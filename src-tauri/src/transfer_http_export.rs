//! HTTP REST 类数据源的表级导出（独立实现）。
//!
//! 由 `HttpDb::export_page` 提供的「不透明游标」分页驱动，逐页读取、逐条写出，绝不整表进内存。
//!
//! 语义与边界：
//! - CSV / JSON 支持；SQL 明确拒绝（HTTP 适配器没有可靠的列类型与真实视图 DDL，无法安全生成 INSERT）。
//! - 自定义 `where_clause` 明确拒绝（分页接口不支持在服务端应用筛选，静默忽略会导出错误数据）。
//! - 结束只由 `next == None` 判定，**短页不是 EOF**；空页但仍有游标也继续。
//! - 游标回环防护：记录有界 token 历史（最多 1024 个），拒绝 `token == 当前游标` 与历史重复；
//!   **不以「行内容相同」判重复**——重复行是合法数据。
//! - JSON：每页用该页自己的列序列化 `{"列":"值"}`（`null` 保留），列集可随页变化；空结果输出 `[]`。
//! - CSV：第一遍把每行 `{columns, values}` 以「u32 小端长度 + JSON」写入同目录独立随机
//!   `StagedOutput` spool（不 publish，Drop 清理），同时用 `HashSet` 累积全部列并即时核算
//!   表头字节上限（含结构开销，列数 ≤ 16384）；第二遍重放 spool，按完整列集映射后逐行写
//!   `ExportSink`。空列集写 0 字节文件。
//! - 分页快照不是强一致快照：并发写入期间各页可能不一致，本实现不对此作任何承诺。
//!
//! 单条记录/表头字节受 `TransferLimits::default().max_record_bytes` 约束，任何序列化均
//! 「先限后分配」，绝不先构造无界字符串再判断。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http_db::{HttpDb, EXPORT_MAX_BYTES, EXPORT_PAGE_MAX_BYTES};
use crate::models::{DbKind, ExportOptions};
use crate::transfer::transfer_output::StagedOutput;
use crate::transfer::{ExportSink, TaskControl};
use crate::transfer_stream::TransferLimits;

/// 导出列数上限：避免恶意/异常适配器返回海量列撑爆内存与表头。
const MAX_COLUMNS: usize = 16_384;
/// 每新增一列在表头预算中计入的结构开销（引号、分隔符、JSON 键的 `"":,` 等）。
const COLUMN_STRUCTURAL_OVERHEAD: u64 = 8;
/// 游标历史长度上限：超过后按 FIFO 淘汰，只保留最近 1024 个已见游标。
const MAX_TOKEN_HISTORY: usize = 1024;
/// 单个分页游标字符串上限（64 KiB）：超长游标直接拒绝，避免历史记录被巨串撑爆。
const MAX_TOKEN_BYTES: usize = 64 * 1024;

// =============================================================================
// 分页游标回环防护
// =============================================================================

/// 有界游标历史。只按游标字符串判重，绝不比较行内容。
struct TokenChain {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl TokenChain {
    fn new() -> Self {
        Self { seen: HashSet::new(), order: VecDeque::new() }
    }

    /// 登记一个游标；已经出现过即判定回环。历史超过上限时按 FIFO 淘汰最旧的。
    fn observe(&mut self, token: &str) -> Result<()> {
        if self.seen.contains(token) {
            bail!("HTTP 导出分页游标出现回环（重复游标: {token}）");
        }
        if self.order.len() >= MAX_TOKEN_HISTORY {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        self.seen.insert(token.to_string());
        self.order.push_back(token.to_string());
        Ok(())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.order.len()
    }
}

/// 校验并推进游标：拒绝空游标、拒绝与当前游标相同、拒绝历史回环，返回下一游标。
fn advance_token(current: Option<&str>, next: &str, chain: &mut TokenChain) -> Result<String> {
    if next.is_empty() {
        bail!("HTTP 导出返回了空的分页游标");
    }
    if next.len() > MAX_TOKEN_BYTES {
        bail!(
            "HTTP 导出分页游标超过上限（{} 字节，上限 {MAX_TOKEN_BYTES} 字节）",
            next.len()
        );
    }
    if current == Some(next) {
        bail!("HTTP 导出分页游标未前进（与当前游标相同: {next}）");
    }
    chain.observe(next)?;
    Ok(next.to_string())
}

// =============================================================================
// 列集合（CSV 跨页并集）
// =============================================================================

/// 跨页累积的列集合：按首次出现顺序稳定编号，`HashSet` 去重。
///
/// 发现新列时立即累加表头字节预算（列名 + 结构开销），超限或列数超限立刻报错，
/// 避免列极多时无界增长。大小写不同的同名列视为冲突，明确报错而不是静默丢弃。
#[derive(Default)]
struct ColumnUnion {
    cols: Vec<String>,
    seen_ci: HashMap<String, String>,
    header_bytes: u64,
}

impl ColumnUnion {
    /// 把一页的列并入集合；重复列跳过，新列即时计入预算。
    fn add_columns(&mut self, columns: &[String], max_record_bytes: u64) -> Result<()> {
        for name in columns {
            let lower = name.to_ascii_lowercase();
            if let Some(existing) = self.seen_ci.get(&lower) {
                if existing != name {
                    bail!("HTTP 导出列名大小写冲突: 已有 {existing}，又出现 {name}");
                }
                continue;
            }
            if self.cols.len() >= MAX_COLUMNS {
                bail!("HTTP 导出列数超过上限（{MAX_COLUMNS} 列）");
            }
            let cost = (name.len() as u64).saturating_add(COLUMN_STRUCTURAL_OVERHEAD);
            let next = self
                .header_bytes
                .checked_add(cost)
                .ok_or_else(|| anyhow!("HTTP 导出表头字节数溢出"))?;
            if next > max_record_bytes {
                bail!(
                    "HTTP 导出表头超过单条记录上限（已发现 {} 列，累计 {next} 字节，上限 {max_record_bytes} 字节）",
                    self.cols.len() + 1
                );
            }
            self.header_bytes = next;
            self.seen_ci.insert(lower, name.clone());
            self.cols.push(name.clone());
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    fn columns(&self) -> &[String] {
        &self.cols
    }
}

/// 严格校验一页的列名（非空、无 NUL、≤256 字节、页内不区分大小写不重复）。
fn validate_page_columns(columns: &[String]) -> Result<()> {
    crate::transfer::validate_names(columns)
}

/// 严格校验行宽与列数一致，绝不允许不对齐。
fn validate_row_width(columns_len: usize, row: &[Option<String>]) -> Result<()> {
    if row.len() != columns_len {
        bail!(
            "HTTP 导出行列数不一致：列元数据 {columns_len} 列，数据行 {} 列",
            row.len()
        );
    }
    Ok(())
}

/// 按列名把一条记录映射到完整列集；记录缺失的列填 `None`（NULL）。
fn map_row_to_union(
    record_columns: &[String],
    values: &[Option<String>],
    index: &HashMap<&str, usize>,
    width: usize,
) -> Result<Vec<Option<String>>> {
    if record_columns.len() != values.len() {
        bail!(
            "HTTP 导出 spool 记录列数不一致：{} 列名，{} 个值",
            record_columns.len(),
            values.len()
        );
    }
    let mut out = vec![None; width];
    for (name, value) in record_columns.iter().zip(values.iter()) {
        if let Some(&position) = index.get(name.as_str()) {
            out[position] = value.clone();
        }
    }
    Ok(out)
}

// =============================================================================
// 有界序列化与 JSON 记录
// =============================================================================

/// 有界字节缓冲：`write` 先核对剩余额度再 `extend`，超限立即报错，绝不无界分配。
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
                "HTTP 导出单条记录超过上限",
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

/// 用该页的列序列化一行为 JSON 对象（值为字符串或 null），超过单条记录上限即拒绝。
fn json_record_bytes(columns: &[String], values: &[Option<String>], max_record_bytes: u64) -> Result<Vec<u8>> {
    if columns.len() != values.len() {
        bail!("HTTP 导出 JSON 行列数不一致");
    }
    let mut object = serde_json::Map::new();
    for (name, value) in columns.iter().zip(values.iter()) {
        let cell = match value {
            Some(text) => serde_json::Value::String(text.clone()),
            None => serde_json::Value::Null,
        };
        object.insert(name.clone(), cell);
    }
    let mut bytes = Vec::new();
    let mut writer = BoundedVecWriter { buf: &mut bytes, written: 0, max_bytes: max_record_bytes };
    serde_json::to_writer(&mut writer, &serde_json::Value::Object(object))
        .map_err(|e| anyhow!("HTTP 导出 JSON 序列化失败（或超过单条记录上限）: {e}"))?;
    drop(writer);
    Ok(bytes)
}

// =============================================================================
// CSV 磁盘 spool（长度前缀 JSON：{columns, values}）
// =============================================================================

#[derive(Debug, Serialize, Deserialize)]
struct SpoolRecord {
    columns: Vec<String>,
    values: Vec<Option<String>>,
}

/// 追加一条 spool 记录：有界序列化后写「u32 小端长度 + JSON 字节」。
fn write_spool_record<W: Write>(writer: &mut W, record: &SpoolRecord, max_record_bytes: u64) -> Result<()> {
    let mut bytes = Vec::new();
    {
        let mut bounded = BoundedVecWriter { buf: &mut bytes, written: 0, max_bytes: max_record_bytes };
        serde_json::to_writer(&mut bounded, record)
            .map_err(|e| anyhow!("HTTP 导出 spool 记录序列化失败（或超过单条记录上限）: {e}"))?;
    }
    if u64::try_from(bytes.len()).map_or(true, |len| len > max_record_bytes) {
        bail!(
            "HTTP 导出单条记录超过上限（{} 字节，上限 {max_record_bytes} 字节）",
            bytes.len()
        );
    }
    let length = u32::try_from(bytes.len()).map_err(|_| anyhow!("HTTP 导出单条 spool 记录过大"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

/// 读取一条 spool 记录；到 EOF 返回 `None`。长度与宽度都重新校验。
fn read_spool_record<R: Read>(reader: &mut R, max_record_bytes: u64) -> Result<Option<SpoolRecord>> {
    let Some(length) = read_record_len(reader)? else { return Ok(None) };
    let length = usize::try_from(length).map_err(|_| anyhow!("HTTP 导出 spool 记录长度超出平台范围"))?;
    if u64::try_from(length).map_or(true, |len| len > max_record_bytes) {
        bail!("HTTP 导出 spool 记录超过单条记录上限");
    }
    let mut buf = vec![0u8; length];
    reader.read_exact(&mut buf).context("HTTP 导出 spool 记录被截断")?;
    let record: SpoolRecord =
        serde_json::from_slice(&buf).map_err(|e| anyhow!("HTTP 导出 spool 记录解析失败: {e}"))?;
    if record.columns.len() != record.values.len() {
        bail!(
            "HTTP 导出 spool 记录列数不一致：{} 列名，{} 个值",
            record.columns.len(),
            record.values.len()
        );
    }
    Ok(Some(record))
}

/// 读取 u32 小端长度前缀；干净 EOF 返回 `None`，半截前缀报错。
fn read_record_len<R: Read>(reader: &mut R) -> Result<Option<u32>> {
    let mut buf = [0u8; 4];
    let mut read = 0usize;
    while read < 4 {
        match reader.read(&mut buf[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => bail!("HTTP 导出 spool 记录长度被截断"),
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).context("读取 HTTP 导出 spool 失败"),
        }
    }
    Ok(Some(u32::from_le_bytes(buf)))
}

/// CSV 导出用的磁盘 spool：先落盘全部记录并发现列，再重放成 CSV。
struct CsvSpool {
    spool: StagedOutput,
    writer: Option<BufWriter<std::fs::File>>,
    columns: ColumnUnion,
    records: u64,
}

impl Drop for CsvSpool {
    fn drop(&mut self) {
        // Windows 的 stage 不共享删除权限：必须先关闭写端克隆，再让 spool 清理。
        self.writer.take();
    }
}

impl CsvSpool {
    fn new_in(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("无法创建 HTTP 导出 spool 目录: {}", dir.display()))?;
        let target = dir.join(format!(".navkit-http-spool-{}.bin", uuid::Uuid::new_v4()));
        let target = target
            .to_str()
            .ok_or_else(|| anyhow!("HTTP 导出 spool 路径不是合法 UTF-8"))?;
        let spool = StagedOutput::new(target)?;
        let file = spool.file_clone()?;
        Ok(Self { spool, writer: Some(BufWriter::new(file)), columns: ColumnUnion::default(), records: 0 })
    }

    /// 仅登记一页的列（不写记录）：即使该页无数据行，也能从列元数据生成表头。
    fn register_columns(&mut self, columns: &[String], max_record_bytes: u64) -> Result<()> {
        self.columns.add_columns(columns, max_record_bytes)
    }

    /// 追加一条记录（列集 + 值），同时把列并入并集。
    fn push(&mut self, columns: &[String], values: &[Option<String>], max_record_bytes: u64) -> Result<()> {
        let record = SpoolRecord { columns: columns.to_vec(), values: values.to_vec() };
        let writer = self.writer.as_mut().ok_or_else(|| anyhow!("HTTP 导出 spool 已关闭"))?;
        write_spool_record(writer, &record, max_record_bytes)?;
        self.columns.add_columns(columns, max_record_bytes)?;
        self.records = self.records.checked_add(1).ok_or_else(|| anyhow!("HTTP 导出行计数溢出"))?;
        Ok(())
    }

    /// 冲刷并关闭写端（幂等），使后续读端可见全部数据。
    fn flush(&mut self) -> Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush().context("刷新 HTTP 导出 spool 失败")?;
        }
        Ok(())
    }

    /// 重放 spool 到 CSV sink：空列集写 0 字节文件；否则按完整列集写表头与全部行。
    fn replay_into(
        mut self,
        kind: &DbKind,
        options: &ExportOptions,
        task: &TaskControl,
        stage: &StagedOutput,
        max_record_bytes: u64,
    ) -> Result<u64> {
        self.flush()?;
        let expected = self.records;
        if self.columns.is_empty() {
            if expected == 0 {
                // 空结果：写出 0 字节目标文件（CSV 无表头、无行）。
                let out = stage.encoder(&options.encoding)?;
                out.finish()?.get_ref().sync_all()?;
                return Ok(0);
            }
            bail!("HTTP 导出有 {expected} 条记录却没有任何列，无法确定表头");
        }
        let columns = self.columns.columns().to_vec();
        let index: HashMap<&str, usize> =
            columns.iter().enumerate().map(|(i, name)| (name.as_str(), i)).collect();

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
            let Some(record) = read_spool_record(&mut reader, max_record_bytes)? else { break };
            let row = map_row_to_union(&record.columns, &record.values, &index, columns.len())?;
            let written = sink.write_rows(std::slice::from_ref(&row))?;
            produced = produced
                .checked_add(written)
                .ok_or_else(|| anyhow!("HTTP 导出行计数溢出"))?;
            if produced % 100 == 0 {
                task.update("export", produced, 0, "正在写入 HTTP 导出数据");
            }
        }
        if produced != expected {
            bail!("HTTP 导出重放行数不一致：spool {expected} 条，写入 {produced} 条");
        }
        sink.finish()?;
        Ok(produced)
    }
}

// =============================================================================
// 分页参数
// =============================================================================

/// 本页请求行数：`min(batch_rows, 剩余 limit)`；无 limit 时整批。
fn page_limit(limit: Option<u64>, produced: u64, batch_rows: u64) -> u64 {
    match limit {
        Some(limit) => limit.saturating_sub(produced).min(batch_rows),
        None => batch_rows,
    }
}

/// spool 目录：与目标导出文件同目录，保证同盘且随目标目录权限约束。
fn spool_directory(options: &ExportOptions) -> PathBuf {
    match Path::new(&options.file).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

// =============================================================================
// 入口
// =============================================================================

/// HTTP 数据源表级导出入口（CSV / JSON）。
pub(super) async fn export(
    http: &HttpDb,
    kind: &DbKind,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
) -> Result<u64> {
    if options
        .where_clause
        .as_deref()
        .map(|clause| !clause.trim().is_empty())
        .unwrap_or(false)
    {
        bail!("HTTP 数据源导出不支持自定义筛选条件（where_clause）；请清空后重试");
    }
    let format = options.format.trim().to_ascii_lowercase();
    let format = if format.is_empty() { "csv".to_string() } else { format };
    match format.as_str() {
        "csv" | "json" => {}
        "sql" => bail!(
            "HTTP 数据源没有可靠的列类型信息与真实视图 DDL，无法安全生成 SQL；请选择 CSV 或 JSON"
        ),
        other => bail!("不支持的导出格式: {other}"),
    }

    let max_record_bytes = TransferLimits::default().max_record_bytes;
    let container = options.schema.as_str();
    let object = options.table.as_str();

    // limit == 0：只探测能力与列结构，不抓任何行。必须走 export_page 以让不支持导出的适配器报错，
    // 不能用 rows/table_columns 绕过。
    if options.limit == Some(0) {
        let page = http.export_page(container, object, None, 0).await?;
        let mut columns = page.data.columns;
        if columns.is_empty() {
            // 适配器在 limit=0 时可能不返回列；在导出页字节作用域内退回真实列元数据。
            // 真实 metadata 错误必须上抛，只有 Ok(None)（适配器无列信息）才保持空列。
            let meta = EXPORT_MAX_BYTES
                .scope(EXPORT_PAGE_MAX_BYTES, http.table_columns(container, object))
                .await?;
            if let Some(meta) = meta {
                columns = meta.into_iter().map(|column| column.name).collect();
            }
        }
        validate_page_columns(&columns)?;
        for row in &page.data.rows {
            validate_row_width(columns.len(), row)?;
        }
        return write_zero_row_output(&format, kind, options, &columns, stage);
    }

    match format.as_str() {
        "json" => export_json(http, options, task, stage, max_record_bytes, container, object).await,
        _ => export_csv(http, kind, options, task, stage, max_record_bytes, container, object).await,
    }
}

/// 无数据行时按格式产出：CSV 有列则写表头、无列则写空文件；JSON 写 `[]`。
fn write_zero_row_output(
    format: &str,
    kind: &DbKind,
    options: &ExportOptions,
    columns: &[String],
    stage: &StagedOutput,
) -> Result<u64> {
    if format == "json" {
        let mut out = stage.encoder(&options.encoding)?;
        out.write_all(b"[]")?;
        out.finish()?.get_ref().sync_all()?;
        return Ok(0);
    }
    if columns.is_empty() {
        let out = stage.encoder(&options.encoding)?;
        out.finish()?.get_ref().sync_all()?;
        return Ok(0);
    }
    let sink = ExportSink::create(
        stage,
        "csv",
        kind,
        &options.schema,
        &options.table,
        columns,
        &[],
        &options.delimiter,
        options.has_header,
        &options.encoding,
    )?;
    sink.finish()?;
    Ok(0)
}

async fn export_json(
    http: &HttpDb,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
    max_record_bytes: u64,
    container: &str,
    object: &str,
) -> Result<u64> {
    let mut out = stage.encoder(&options.encoding)?;
    out.write_all(b"[")?;
    let mut first = true;
    let mut produced = 0u64;
    let mut token: Option<String> = None;
    let mut chain = TokenChain::new();
    let mut cleanup_token: Option<String> = None;
    let result: Result<u64> = async {
    loop {
        task.check()?;
        let limit = page_limit(options.limit, produced, TransferLimits::default().batch_rows);
        if limit == 0 {
            break;
        }
        let page = http.export_page(container, object, token.as_deref(), limit).await?;
        // 保存最新资源游标，再执行可能失败的取消、验证与写盘。
        cleanup_token = page.next.clone();
        task.check()?;
        validate_page_columns(&page.data.columns)?;
        let take = match options.limit {
            Some(total) => usize::try_from(total.saturating_sub(produced))
                .map_err(|_| anyhow!("HTTP 导出分页行数超出平台范围"))?,
            None => page.data.rows.len(),
        };
        for row in page.data.rows.iter().take(take) {
            task.check()?;
            validate_row_width(page.data.columns.len(), row)?;
            let bytes = json_record_bytes(&page.data.columns, row, max_record_bytes)?;
            if first {
                first = false;
            } else {
                out.write_all(b",\n")?;
            }
            out.write_all(&bytes)?;
            produced = produced.checked_add(1).ok_or_else(|| anyhow!("HTTP 导出行计数溢出"))?;
        }
        task.update("export", produced, options.limit.unwrap_or(0), "正在导出 HTTP 数据");
        if options.limit.map_or(false, |limit| produced >= limit) {
            break;
        }
        match page.next {
            None => break,
            Some(next) => token = Some(advance_token(token.as_deref(), &next, &mut chain)?),
        }
    }
    task.check()?;
    out.write_all(b"]")?;
    out.finish()?.get_ref().sync_all()?;
    Ok(produced)
    }.await;
    let cleanup = http.finish_export(cleanup_token.as_deref()).await;
    match result {
        Err(error) => match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(error.context(format!("导出失败且游标清理失败: {cleanup_error:#}"))),
        },
        Ok(count) => { cleanup?; Ok(count) }
    }
}

async fn export_csv(
    http: &HttpDb,
    kind: &DbKind,
    options: &ExportOptions,
    task: &TaskControl,
    stage: &StagedOutput,
    max_record_bytes: u64,
    container: &str,
    object: &str,
) -> Result<u64> {
    let mut spool = CsvSpool::new_in(&spool_directory(options))?;
    let mut produced = 0u64;
    let mut token: Option<String> = None;
    let mut chain = TokenChain::new();
    let mut cleanup_token: Option<String> = None;
    let result: Result<u64> = async {
    loop {
        task.check()?;
        let limit = page_limit(options.limit, produced, TransferLimits::default().batch_rows);
        if limit == 0 {
            break;
        }
        let page = http.export_page(container, object, token.as_deref(), limit).await?;
        cleanup_token = page.next.clone();
        task.check()?;
        validate_page_columns(&page.data.columns)?;
        spool.register_columns(&page.data.columns, max_record_bytes)?;
        let take = match options.limit {
            Some(total) => usize::try_from(total.saturating_sub(produced))
                .map_err(|_| anyhow!("HTTP 导出分页行数超出平台范围"))?,
            None => page.data.rows.len(),
        };
        for row in page.data.rows.iter().take(take) {
            task.check()?;
            validate_row_width(page.data.columns.len(), row)?;
            spool.push(&page.data.columns, row, max_record_bytes)?;
            produced = produced.checked_add(1).ok_or_else(|| anyhow!("HTTP 导出行计数溢出"))?;
        }
        task.update("export", produced, options.limit.unwrap_or(0), "正在读取 HTTP 数据");
        if options.limit.map_or(false, |limit| produced >= limit) {
            break;
        }
        match page.next {
            None => break,
            Some(next) => token = Some(advance_token(token.as_deref(), &next, &mut chain)?),
        }
    }
    spool.replay_into(kind, options, task, stage, max_record_bytes)
    }.await;
    let cleanup = http.finish_export(cleanup_token.as_deref()).await;
    match result {
        Err(error) => match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(error.context(format!("导出失败且游标清理失败: {cleanup_error:#}"))),
        },
        Ok(count) => { cleanup?; Ok(count) }
    }
}

// =============================================================================
// 测试：纯 helper 与 spool（不依赖真实 HttpDb）
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Seek, SeekFrom};
    use std::path::PathBuf;

    fn test_options() -> ExportOptions {
        ExportOptions {
            connection_id: "t".into(),
            schema: "db".into(),
            table: "obj".into(),
            file: "out.csv".into(),
            format: "csv".into(),
            has_header: true,
            delimiter: ",".into(),
            encoding: "utf-8".into(),
            where_clause: None,
            limit: None,
        }
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("navkit-http-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
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
    fn column_union_includes_columns_from_later_page() {
        let mut union = ColumnUnion::default();
        union.add_columns(&["a".to_string(), "b".to_string()], 1 << 20).unwrap();
        union.add_columns(&["a".to_string(), "c".to_string(), "d".to_string()], 1 << 20).unwrap();
        assert_eq!(union.columns().to_vec(), vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()]);
    }

    #[test]
    fn column_union_allows_repeated_columns_and_duplicate_rows() {
        let mut union = ColumnUnion::default();
        union.add_columns(&["a".to_string(), "b".to_string()], 1 << 20).unwrap();
        union.add_columns(&["a".to_string(), "b".to_string()], 1 << 20).unwrap();
        assert_eq!(union.columns().to_vec(), vec!["a".to_string(), "b".to_string()]);
        let index: HashMap<&str, usize> =
            union.columns().iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let row = vec![Some("x".to_string()), None];
        // 完全相同的行重复映射都成功——重复行是合法数据，不按行内容判重。
        assert_eq!(map_row_to_union(&["a".to_string(), "b".to_string()], &row, &index, 2).unwrap(), row);
        assert_eq!(map_row_to_union(&["a".to_string(), "b".to_string()], &row, &index, 2).unwrap(), row);
    }

    #[test]
    fn row_width_mismatch_rejected() {
        let error = validate_row_width(2, &[Some("a".to_string())]).unwrap_err();
        assert!(format!("{error}").contains("列数不一致"));
        assert!(validate_row_width(1, &[Some("a".to_string())]).is_ok());
    }

    #[test]
    fn empty_csv_spool_writes_empty_file() {
        let dir = TempDir::new();
        let spool = CsvSpool::new_in(&dir.path).unwrap();
        let stage = StagedOutput::new(dir.path.join("out.csv").to_str().unwrap()).unwrap();
        let task = TaskControl::begin(&format!("http-empty-{}", uuid::Uuid::new_v4().simple())).unwrap();
        let options = test_options();
        let written = spool
            .replay_into(&DbKind::of("elasticsearch"), &options, &task, &stage, 1 << 20)
            .unwrap();
        assert_eq!(written, 0);
        assert!(read_stage(&stage).is_empty());
        task.finish(&Ok::<u64, anyhow::Error>(written));
    }

    #[test]
    fn token_chain_rejects_repeat_and_cycle() {
        let mut chain = TokenChain::new();
        advance_token(None, "a", &mut chain).unwrap();
        // 与当前游标相同
        assert!(advance_token(Some("a"), "a", &mut chain).is_err());
        advance_token(Some("a"), "b", &mut chain).unwrap();
        // 回环 a -> b -> a
        let error = advance_token(Some("b"), "a", &mut chain).unwrap_err();
        assert!(format!("{error}").contains("回环"));
        // 空游标拒绝
        assert!(advance_token(Some("b"), "", &mut chain).is_err());
        // 历史不超过上限
        let mut bounded = TokenChain::new();
        for index in 0..(MAX_TOKEN_HISTORY + 16) {
            bounded.observe(&format!("t{index}")).unwrap();
        }
        assert_eq!(bounded.len(), MAX_TOKEN_HISTORY);
    }

    #[test]
    fn column_union_enforces_header_limit() {
        let mut union = ColumnUnion::default();
        let error = union
            .add_columns(&["field_00".to_string(), "field_01".to_string()], 8)
            .unwrap_err();
        assert!(format!("{error}").contains("表头超过"));
    }

    #[test]
    fn spool_truncated_record_rejected() {
        // 半截长度前缀
        let mut reader = Cursor::new(vec![1u8, 0u8]);
        let error = read_spool_record(&mut reader, 1 << 20).unwrap_err();
        assert!(format!("{error}").contains("长度被截断"));

        // 完整长度前缀但记录体被截断
        let record = SpoolRecord {
            columns: vec!["a".into(), "b".into()],
            values: vec![Some("value".into()), None],
        };
        let mut bytes = Vec::new();
        write_spool_record(&mut bytes, &record, 1 << 20).unwrap();
        bytes.truncate(bytes.len() - 2);
        let mut reader = Cursor::new(bytes);
        let error = read_spool_record(&mut reader, 1 << 20).unwrap_err();
        assert!(format!("{error}").contains("截断"));
    }

    #[test]
    fn spool_record_limit_enforced() {
        let record = SpoolRecord {
            columns: vec!["a".into()],
            values: vec![Some("x".repeat(64))],
        };
        let mut bytes = Vec::new();
        let error = write_spool_record(&mut bytes, &record, 16).unwrap_err();
        assert!(format!("{error}").contains("超过上限"));

        // 读取时长度前缀超过上限也必须拒绝，且不为其分配内存
        let mut reader = Cursor::new((64u32).to_le_bytes().to_vec());
        let error = read_spool_record(&mut reader, 16).unwrap_err();
        assert!(format!("{error}").contains("超过单条记录上限"));
    }

    #[test]
    fn json_record_uses_current_page_columns() {
        let bytes = json_record_bytes(
            &["a".to_string(), "b".to_string()],
            &[Some("x".to_string()), None],
            1 << 20,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["a"], serde_json::json!("x"));
        assert!(value["b"].is_null());

        let other = json_record_bytes(&["c".to_string()], &[Some("y".to_string())], 1 << 20).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&other).unwrap();
        assert_eq!(value["c"], serde_json::json!("y"));
        assert!(value.get("a").is_none());
    }

    #[test]
    fn map_row_to_union_fills_missing_columns() {
        let union = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let index: HashMap<&str, usize> =
            union.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let row = map_row_to_union(
            &["a".to_string(), "c".to_string()],
            &[Some("1".into()), Some("3".into())],
            &index,
            union.len(),
        )
        .unwrap();
        assert_eq!(row, vec![Some("1".to_string()), None, Some("3".to_string())]);
    }
}

#[cfg(test)]
#[path = "transfer_http_export_tests.rs"]
mod http_export_tests;
