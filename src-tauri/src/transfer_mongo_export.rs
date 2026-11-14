//! MongoDB 表级导出（独立实现，父模块以 `#[path = "transfer_mongo_export.rs"] mod mongo;` 嵌入）。
//!
//! 语义：
//! - JSON：逐文档写出 canonical Extended JSON（`{"$oid":…}`、`{"$numberInt":…}` 等），
//!   保留嵌套对象/数组、BSON 类型、以及对「字段缺失」和「字段为 null」的区分；
//!   不把文档压平成文本。
//! - CSV：先单遍读取文档，把每个文档以 BSON 长度前缀写入磁盘 spool，同时发现全部顶层字段；
//!   第二遍重放 spool，按完整列集写 CSV（后来的文档出现的新字段也会成为一列）。
//!   空集合写出 0 字节文件，而不是报错。
//! - SQL：MongoDB 没有 SQL 恢复语义，明确拒绝。
//!
//! spool 复用 `StagedOutput` 的临时 stage 文件（目标同目录的隐藏文件），只通过 `file_clone`
//! 写入/读取，永不 publish，Drop 自动清理；单条记录与表头受 `TransferLimits::default()` 的
//! `max_record_bytes` 限制，绝不为整个集合分配内存。

use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use mongodb::bson::{Bson, Document};

use crate::models::{DbKind, ExportOptions};
use crate::transfer::transfer_output::StagedOutput;
use crate::transfer::{ExportSink, TaskControl};
use crate::transfer_stream::TransferLimits;

/// 找到的列集合：按首次出现顺序记录顶层字段名，序号稳定；`_id` 始终排到第一列。
///
/// 字段发现阶段就地记账：每新增一列立即累加表头字节数并对照单条记录上限，
/// 用 `HashSet` 做去重，避免字段极多时退化成 O(n²) 且不会无界增长。
#[derive(Default)]
struct ColumnSet {
    cols: Vec<String>,
    seen: std::collections::HashSet<String>,
    header_bytes: u64,
}

impl ColumnSet {
    fn add(&mut self, doc: &Document, max_record_bytes: u64) -> Result<()> {
        for key in doc.keys() {
            if self.seen.contains(key) {
                continue;
            }
            if key.trim().is_empty() {
                bail!("MongoDB 文档包含空字段名，无法作为 CSV 表头");
            }
            // 每个新增列至少占用「字段名 + 一个分隔符」字节，据此就地限制表头总量。
            let cost = (key.len() as u64).saturating_add(1);
            let next = self
                .header_bytes
                .checked_add(cost)
                .ok_or_else(|| anyhow!("MongoDB 导出表头字节数溢出"))?;
            if next > max_record_bytes {
                bail!(
                    "MongoDB 导出表头超过上限（已发现 {} 列，累计 {next} 字节，上限 {max_record_bytes} 字节）",
                    self.cols.len() + 1
                );
            }
            self.header_bytes = next;
            self.seen.insert(key.clone());
            self.cols.push(key.clone());
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    /// 输出顺序：`_id` 优先（若存在），其余保持首次出现顺序。
    fn ordered(&self) -> Vec<String> {
        let mut cols = self.cols.clone();
        if let Some(pos) = cols.iter().position(|c| c == "_id") {
            if pos != 0 {
                cols.swap(0, pos);
            }
        }
        cols
    }
}
/// 把一个 BSON 文档映射成 CSV 一行：缺失字段与 `null` 都按 NULL（`\N`）写出。
fn csv_row(doc: &Document, cols: &[String]) -> Vec<Option<String>> {
    cols.iter()
        .map(|col| match doc.get(col) {
            Some(Bson::Null) | None => None,
            Some(value) => Some(crate::mongo_db::bson_to_text(value)),
        })
        .collect()
}

/// 有界字节缓冲：序列化边写边计数，超过单条记录上限立即报错，避免先 `to_vec`
/// 出一整个无界字符串再判断。
struct BoundedVecWriter<'a> {
    buf: &'a mut Vec<u8>,
    written: u64,
    max_bytes: u64,
}

impl Write for BoundedVecWriter<'_> {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if u64::try_from(input.len()).map_or(true, |len| len > self.max_bytes.saturating_sub(self.written))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MongoDB 导出单条记录超过上限",
            ));
        }
        self.buf.extend_from_slice(input);
        self.written += input.len() as u64;
        Ok(input.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 单文档的 canonical Extended JSON 字节，超过单条记录上限即拒绝。
fn json_record_bytes(doc: &Document, max_record_bytes: u64) -> Result<Vec<u8>> {
    let value = Bson::Document(doc.clone()).into_canonical_extjson();
    let mut bytes = Vec::new();
    let mut writer = BoundedVecWriter {
        buf: &mut bytes,
        written: 0,
        max_bytes: max_record_bytes,
    };
    serde_json::to_writer(&mut writer, &value).map_err(|e| {
        anyhow!("MongoDB canonical Extended JSON 序列化失败（或超过单条记录上限）: {e}")
    })?;
    drop(writer);
    Ok(bytes)
}

/// 写入一条 JSON 记录（首条前不加逗号，后续以 `,\n` 分隔）。
fn write_json_document<W: Write>(
    out: &mut W,
    doc: &Document,
    first: &mut bool,
    max_record_bytes: u64,
) -> Result<()> {
    let bytes = json_record_bytes(doc, max_record_bytes)?;
    if *first {
        *first = false;
    } else {
        out.write_all(b",\n")?;
    }
    out.write_all(&bytes)?;
    Ok(())
}

/// 从 spool 读取一条记录的长度前缀；正常到达 EOF 返回 `None`。
fn read_record_len<R: Read>(reader: &mut R) -> Result<Option<u32>> {
    let mut buf = [0u8; 4];
    let mut read = 0usize;
    while read < 4 {
        match reader.read(&mut buf[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => bail!("MongoDB spool 记录长度被截断"),
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).context("读取 MongoDB spool 失败"),
        }
    }
    Ok(Some(u32::from_le_bytes(buf)))
}

/// CSV 导出用的磁盘 spool：先落盘全部文档并发现列，再重放成 CSV。
///
/// spool 本身就是一个 `StagedOutput`（目标同目录隐藏文件），只借 `file_clone` 写读，
/// 从不调用 `publish`；析构时由 `StagedOutput::Drop` 删除临时文件。
struct CsvSpool {
    spool: StagedOutput,
    writer: Option<BufWriter<std::fs::File>>,
    columns: ColumnSet,
    documents: u64,
}

impl Drop for CsvSpool {
    fn drop(&mut self) {
        // Windows 下 cloned handle 未共享 DELETE；先关闭写端，再释放 stage 才能删除文件。
        self.writer.take();
    }
}

impl CsvSpool {
    /// 在指定目录下创建 spool（生产用导出目标文件所在目录；测试用隔离目录）。
    fn new_in(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("无法创建 MongoDB spool 目录: {}", dir.display()))?;
        let target = dir.join(format!("navkit-mongo-spool-{}.bin", uuid::Uuid::new_v4()));
        let target = target
            .to_str()
            .ok_or_else(|| anyhow!("MongoDB 导出 spool 路径不是合法 UTF-8"))?;
        let spool = StagedOutput::new(target)?;
        let file = spool.file_clone()?;
        Ok(Self {
            spool,
            writer: Some(BufWriter::new(file)),
            columns: ColumnSet::default(),
            documents: 0,
        })
    }

    /// 生产路径：spool 建在导出目标文件的同一目录，避免跨卷（尤其是 Windows 临时目录
    /// 可能是 junction/另一分区）时重命名或读取失败；仅使用隐藏临时文件，析构即删除。
    fn new(output_file: &str) -> Result<Self> {
        let path = Path::new(output_file);
        let dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("MongoDB 导出 spool 无法确定输出文件目录: {output_file}"))?;
        Self::new_in(dir)
    }

    /// 追加一个文档：序列化后按「u32 小端长度 + BSON 字节」写入 spool。
    fn push(&mut self, doc: &Document, max_record_bytes: u64) -> Result<()> {
        let bytes =
            mongodb::bson::to_vec(doc).map_err(|e| anyhow!("MongoDB 文档序列化失败: {e}"))?;
        if u64::try_from(bytes.len()).map_or(true, |n| n > max_record_bytes) {
            bail!(
                "MongoDB 导出单条记录超过上限（{} 字节，上限 {} 字节）",
                bytes.len(),
                max_record_bytes
            );
        }
        let len = u32::try_from(bytes.len()).map_err(|_| anyhow!("MongoDB 单条记录过大"))?;
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow!("MongoDB 导出 spool 已关闭"))?;
        writer.write_all(&len.to_le_bytes())?;
        writer.write_all(&bytes)?;
        self.columns.add(doc, max_record_bytes)?;
        self.documents = self
            .documents
            .checked_add(1)
            .ok_or_else(|| anyhow!("MongoDB 导出行计数溢出"))?;
        Ok(())
    }

    fn documents(&self) -> u64 {
        self.documents
    }

    /// 冲刷并关闭写端，使后续读端能看到全部数据。幂等。
    fn flush(&mut self) -> Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush().context("刷新 MongoDB spool 失败")?;
        }
        Ok(())
    }

    /// 重放 spool 到 CSV sink：空集合写 0 字节文件；否则按完整列集写表头与全部行。
    fn replay_into(
        mut self,
        kind: &DbKind,
        options: &ExportOptions,
        task: &TaskControl,
        stage: &StagedOutput,
        max_record_bytes: u64,
    ) -> Result<u64> {
        self.flush()?;
        let expected = self.documents;
        if self.columns.is_empty() {
            if expected == 0 {
                // 空集合：写出 0 字节目标文件（CSV 无表头、无行）。
                let out = stage.encoder(&options.encoding)?;
                out.finish()?.get_ref().sync_all()?;
                return Ok(0);
            }
            // 集合非空却没有任何顶层字段：无法确定表头，明确报错而不是静默丢数据。
            bail!("MongoDB 集合中的 {expected} 条文档都没有任何顶层字段，无法确定导出表头");
        }
        let columns = self.columns.ordered();
        // 表头同样受单条记录字节上限约束，避免超长列名集合撑爆单行。
        let header_bytes: u64 = columns
            .iter()
            .map(|name| name.len() as u64)
            .sum::<u64>()
            .saturating_add(
                (columns.len().saturating_sub(1) as u64)
                    .saturating_mul(options.delimiter.len() as u64),
            );
        if header_bytes > max_record_bytes {
            bail!(
                "MongoDB 导出表头超过上限（{header_bytes} 字节，上限 {max_record_bytes} 字节）"
            );
        }

        let mut sink = ExportSink::create(
            stage,
            "csv",
            kind,
            &options.schema,
            &options.table,
            &columns,
            &[],
            &options.delimiter,
            options.has_header,
            &options.encoding,
        )?;

        let reader_file = self.spool.file_clone()?;
        let mut reader = BufReader::new(reader_file);
        reader.seek(SeekFrom::Start(0))?;

        let mut produced = 0u64;
        loop {
            task.check()?;
            let Some(len) = read_record_len(&mut reader)? else { break };
            let len = usize::try_from(len).map_err(|_| anyhow!("MongoDB spool 记录长度超出平台范围"))?;
            if u64::try_from(len).map_or(true, |n| n > max_record_bytes) {
                bail!("MongoDB spool 记录超过单条记录上限");
            }
            let mut buf = vec![0u8; len];
            reader
                .read_exact(&mut buf)
                .context("MongoDB spool 记录被截断")?;
            let doc: Document = mongodb::bson::from_slice(&buf)
                .map_err(|e| anyhow!("MongoDB spool 文档反序列化失败: {e}"))?;
            let row = csv_row(&doc, &columns);
            let written = sink.write_rows(std::slice::from_ref(&row))?;
            produced = produced
                .checked_add(written)
                .ok_or_else(|| anyhow!("MongoDB 导出行计数溢出"))?;
            if produced % 100 == 0 {
                task.update("export", produced, 0, "正在导出 MongoDB 数据");
            }
        }
        if produced != expected {
            bail!("MongoDB 导出重放行数不一致：spool {expected} 条，写入 {produced} 条");
        }
        sink.finish()?;
        Ok(produced)
    }
}

/// JSON 导出：逐文档 canonical Extended JSON，空集合输出 `[\n\n]\n`。
async fn export_json(
    mongo: &crate::mongo_db::MongoDb,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
    max_record_bytes: u64,
) -> Result<u64> {
    let mut out = stage.encoder(&options.encoding)?;
    out.write_all(b"[\n")?;
    let mut first = true;
    let mut count = 0u64;
    if let Some(mut cursor) = mongo
        .find_export_cursor(
            &options.schema,
            &options.table,
            options.where_clause.as_deref().unwrap_or(""),
            options.limit,
        )
        .await?
    {
        while let Some(next) = cursor.next().await {
            task.check()?;
            let doc = next.map_err(|e| anyhow!("MongoDB 读取文档失败: {e}"))?;
            write_json_document(&mut out, &doc, &mut first, max_record_bytes)?;
            count = count
                .checked_add(1)
                .ok_or_else(|| anyhow!("MongoDB 导出行计数溢出"))?;
            if count % 100 == 0 {
                task.update("export", count, 0, "正在导出 MongoDB 数据");
            }
        }
    }
    out.write_all(b"\n]\n")?;
    out.finish()?.get_ref().sync_all()?;
    Ok(count)
}

/// CSV 导出：先 spool 全量文档并发现列，再重放到 CSV。
async fn export_csv(
    mongo: &crate::mongo_db::MongoDb,
    kind: &DbKind,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
    max_record_bytes: u64,
) -> Result<u64> {
    let mut spool = CsvSpool::new(&options.file)?;
    if let Some(mut cursor) = mongo
        .find_export_cursor(
            &options.schema,
            &options.table,
            options.where_clause.as_deref().unwrap_or(""),
            options.limit,
        )
        .await?
    {
        while let Some(next) = cursor.next().await {
            task.check()?;
            let doc = next.map_err(|e| anyhow!("MongoDB 读取文档失败: {e}"))?;
            spool.push(&doc, max_record_bytes)?;
            if spool.documents() % 100 == 0 {
                task.update("export", spool.documents(), 0, "正在读取 MongoDB 文档");
            }
        }
    }
    spool.replay_into(kind, options, task, stage, max_record_bytes)
}

/// MongoDB 表级导出入口。`kind` 目前只用于 CSV 的通用元数据（MongoDB 无 SQL 方言）。
pub(super) async fn export(
    mongo: &crate::mongo_db::MongoDb,
    kind: &crate::models::DbKind,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
) -> Result<u64> {
    let format = options.format.trim().to_ascii_lowercase();
    let max_record_bytes = TransferLimits::default().max_record_bytes;
    match format.as_str() {
        "sql" => bail!("MongoDB 没有 SQL 恢复语义，不支持导出为 SQL；请选择 CSV 或 JSON"),
        "json" => export_json(mongo, options, task, stage, max_record_bytes).await,
        "csv" | "" => export_csv(mongo, kind, options, task, stage, max_record_bytes).await,
        other => bail!("不支持的导出格式: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;
    use std::io::Read;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("navkit-mongo-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn test_task() -> TaskControl {
        TaskControl::begin(&format!("mongo-test-{}", uuid::Uuid::new_v4().simple())).unwrap()
    }

    fn test_options() -> ExportOptions {
        ExportOptions {
            connection_id: "t".into(),
            schema: "db".into(),
            table: "coll".into(),
            file: "out.csv".into(),
            format: "csv".into(),
            has_header: true,
            delimiter: ",".into(),
            encoding: "utf-8".into(),
            where_clause: None,
            limit: None,
        }
    }

    fn read_stage(stage: &StagedOutput) -> String {
        let mut file = stage.file_clone().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn json_record_preserves_nested_and_bson_types() {
        let oid = mongodb::bson::oid::ObjectId::new();
        let doc = doc! {
            "_id": oid,
            "n": Bson::Int32(7),
            "big": Bson::Int64(9_000_000_000),
            "nested": { "a": Bson::Double(1.5), "arr": [Bson::Int32(1), Bson::String("x".into())] },
            "nil": Bson::Null,
        };
        let bytes = json_record_bytes(&doc, 16 * 1024 * 1024).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["_id"], serde_json::json!({ "$oid": oid.to_hex() }));
        assert_eq!(value["n"], serde_json::json!({ "$numberInt": "7" }));
        assert_eq!(value["big"], serde_json::json!({ "$numberLong": "9000000000" }));
        assert_eq!(value["nested"]["a"], serde_json::json!({ "$numberDouble": "1.5" }));
        assert_eq!(value["nested"]["arr"][0], serde_json::json!({ "$numberInt": "1" }));
        assert_eq!(value["nested"]["arr"][1], serde_json::json!("x"));
        assert!(value["nil"].is_null());
        // 缺失字段不应出现
        assert!(value.get("missing").is_none());
    }

    #[test]
    fn json_stream_keeps_missing_field_absent_from_later_document() {
        let mut buf = Vec::new();
        let mut first = true;
        write_json_document(&mut buf, &doc! { "a": 1, "b": Bson::Null }, &mut first, 1 << 20).unwrap();
        write_json_document(&mut buf, &doc! { "a": 2, "c": "z" }, &mut first, 1 << 20).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let value: serde_json::Value = serde_json::from_str(&format!("[{text}]")).unwrap();
        assert!(value[0]["b"].is_null());
        assert!(value[0].get("c").is_none());
        assert!(value[1].get("b").is_none());
        assert_eq!(value[1]["c"], serde_json::json!("z"));
    }

    #[test]
    fn csv_columns_include_fields_from_later_documents() {
        let d1 = doc! { "_id": 1, "a": "x" };
        let d2 = doc! { "_id": 2, "a": "y", "c": 3 };
        let mut set = ColumnSet::default();
        set.add(&d1, 1 << 20).unwrap();
        set.add(&d2, 1 << 20).unwrap();
        let columns = set.ordered();
        assert_eq!(columns, vec!["_id", "a", "c"]);
        assert_eq!(
            csv_row(&d1, &columns),
            vec![Some("1".to_string()), Some("x".to_string()), None]
        );
        assert_eq!(
            csv_row(&d2, &columns),
            vec![Some("2".to_string()), Some("y".to_string()), Some("3".to_string())]
        );
    }

    #[test]
    fn csv_replay_writes_all_columns_from_later_document() {
        let dir = TempDir::new();
        let mut spool = CsvSpool::new_in(&dir.path).unwrap();
        spool.push(&doc! { "_id": 1, "a": "x" }, 1 << 20).unwrap();
        spool.push(&doc! { "_id": 2, "a": "y", "c": 3 }, 1 << 20).unwrap();
        let stage = StagedOutput::new(dir.path.join("out.csv").to_str().unwrap()).unwrap();
        let task = test_task();
        let options = test_options();
        let written = spool
            .replay_into(&DbKind::of("mongodb"), &options, &task, &stage, 1 << 20)
            .unwrap();
        assert_eq!(written, 2);
        assert_eq!(read_stage(&stage), "_id,a,c\r\n1,x,\\N\r\n2,y,3\r\n");
        task.finish(&Ok::<u64, anyhow::Error>(written));
    }

    #[test]
    fn csv_empty_spool_writes_empty_file() {
        let dir = TempDir::new();
        let spool = CsvSpool::new_in(&dir.path).unwrap();
        let stage = StagedOutput::new(dir.path.join("out.csv").to_str().unwrap()).unwrap();
        let task = test_task();
        let options = test_options();
        let written = spool
            .replay_into(&DbKind::of("mongodb"), &options, &task, &stage, 1 << 20)
            .unwrap();
        assert_eq!(written, 0);
        assert!(read_stage(&stage).is_empty());
        task.finish(&Ok::<u64, anyhow::Error>(written));
    }

    #[test]
    fn csv_rejects_oversized_document() {
        let dir = TempDir::new();
        let mut spool = CsvSpool::new_in(&dir.path).unwrap();
        let big = doc! { "x": "a".repeat(64) };
        let error = spool.push(&big, 16).unwrap_err();
        assert!(format!("{error}").contains("超过上限"));
    }

    #[test]
    fn json_record_rejects_oversized_document() {
        let doc = doc! { "x": "a".repeat(64) };
        let error = json_record_bytes(&doc, 16).unwrap_err();
        assert!(format!("{error}").contains("上限"));
    }

    #[test]
    fn column_set_limits_header_bytes_during_discovery() {
        let mut set = ColumnSet::default();
        let mut doc = Document::new();
        for index in 0..16 {
            let _ = doc.insert(format!("field_{index:02}"), Bson::Int32(index));
        }
        let error = set.add(&doc, 8).unwrap_err();
        assert!(format!("{error}").contains("表头超过上限"));
    }

    #[test]
    fn column_set_rejects_empty_field_name() {
        let mut set = ColumnSet::default();
        let mut doc = Document::new();
        let _ = doc.insert(String::new(), Bson::Int32(1));
        let error = set.add(&doc, 1 << 20).unwrap_err();
        assert!(format!("{error}").contains("空字段名"));
    }

    #[test]
    fn csv_spool_temp_file_is_removed_on_drop() {
        let dir = TempDir::new();
        for flush in [false, true] {
            {
                let mut spool = CsvSpool::new_in(&dir.path).unwrap();
                spool.push(&doc! { "a": 1 }, 1024).unwrap();
                if flush { spool.flush().unwrap(); }
                assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 1);
            }
            // 正常结束和尚未 flush 的错误/取消路径都必须先关闭 writer 再清理 stage。
            assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 0);
        }
        {
            let mut spool = CsvSpool::new_in(&dir.path).unwrap();
            assert!(spool.push(&doc! { "a": "x".repeat(64) }, 16).is_err());
        }
        assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 0);
    }
}
