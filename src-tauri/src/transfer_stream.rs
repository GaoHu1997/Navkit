//! 大数据流式传输辅助：有界分批、增量解码、分页读取。
//!
//! 设计要点（见 plan/13-大数据流式传输.md）：
//! - 读取文件时按块增量解码，不把整个文件读进内存；
//! - CSV/JSON 逐条产生记录，按行数与字节双限组成批次；
//! - 数据库导出/同步按 LIMIT/OFFSET 分页读取，绝不再 `fetch_all` 全量；
//! - 分页读取只用于只读导出与快照读取，不作为镜像删除的完整性依据。
//! - 单条记录超过 max_record_bytes 时明确失败，而不是静默截断。

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::{qualified, Db};
use crate::models::DbKind;

pub const DEFAULT_BATCH_ROWS: u64 = 1000;
pub const DEFAULT_BATCH_BYTES: u64 = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_QUEUE_BATCHES: u32 = 2;
const READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransferLimits {
    #[serde(default = "default_batch_rows")]
    pub batch_rows: u64,
    #[serde(default = "default_batch_bytes")]
    pub batch_bytes: u64,
    #[serde(default = "default_max_record_bytes")]
    pub max_record_bytes: u64,
    #[serde(default = "default_queue_batches")]
    pub queue_batches: u32,
    #[serde(default)]
    pub spill_budget_bytes: Option<String>,
}

fn default_batch_rows() -> u64 { DEFAULT_BATCH_ROWS }
fn default_batch_bytes() -> u64 { DEFAULT_BATCH_BYTES }
fn default_max_record_bytes() -> u64 { DEFAULT_MAX_RECORD_BYTES }
fn default_queue_batches() -> u32 { DEFAULT_QUEUE_BATCHES }

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            batch_rows: DEFAULT_BATCH_ROWS,
            batch_bytes: DEFAULT_BATCH_BYTES,
            max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
            queue_batches: DEFAULT_QUEUE_BATCHES,
            spill_budget_bytes: None,
        }
    }
}

impl TransferLimits {
    /// 限制值收敛到安全范围，避免前端传入 0 或过大值导致无界内存。
    pub fn normalized(&self) -> Self {
        Self {
            batch_rows: self.batch_rows.clamp(1, 100_000),
            batch_bytes: self.batch_bytes.clamp(64 * 1024, 256 * 1024 * 1024),
            max_record_bytes: self.max_record_bytes.clamp(1024 * 1024, 512 * 1024 * 1024),
            queue_batches: self.queue_batches.clamp(1, 16),
            spill_budget_bytes: self.spill_budget_bytes.clone(),
        }
    }
}

/// 将字节流按指定编码增量解码为 UTF-8 的 `Read`（去除开头的 UTF-8 BOM）。
/// 输入、输出各复用 64 KiB；非法序列前的有效前缀先交付，之后持续报错。
pub struct DecodeReader<R: Read> {
    inner: R,
    // None 表示严格 ISO-8859-1；不能用 WHATWG 的 Windows-1252 别名替代。
    decoder: Option<encoding_rs::Decoder>,
    inbuf: Vec<u8>,
    inpos: usize,
    inlen: usize,
    out: Vec<u8>,
    outpos: usize,
    outlen: usize,
    needs_input: bool,
    eof: bool,
    decoder_finished: bool,
    decode_failed: bool,
}

impl<R: Read> DecodeReader<R> {
    pub fn new(inner: R, encoding: &str) -> Result<Self> {
        let lower = encoding.to_ascii_lowercase();
        let decoder = match lower.as_str() {
            "" | "utf-8" | "utf8" => Some(encoding_rs::UTF_8.new_decoder_with_bom_removal()),
            "gbk" | "gb2312" | "gb18030" => Some(encoding_rs::GB18030.new_decoder_without_bom_handling()),
            "big5" => Some(encoding_rs::BIG5.new_decoder_without_bom_handling()),
            "latin1" | "iso-8859-1" => None,
            _ => bail!("不支持的编码: {}", encoding),
        };
        Ok(Self {
            inner, decoder, inbuf: vec![0; READ_CHUNK], inpos: 0, inlen: 0,
            out: vec![0; READ_CHUNK], outpos: 0, outlen: 0,
            needs_input: true, eof: false, decoder_finished: false, decode_failed: false,
        })
    }

    fn fill_more(&mut self) -> io::Result<()> {
        debug_assert_eq!(self.inpos, self.inlen);
        loop {
            match self.inner.read(&mut self.inbuf) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
                Ok(n) => {
                    self.inpos = 0;
                    self.inlen = n;
                    self.eof = n == 0;
                    self.needs_input = false;
                    return Ok(());
                }
            }
        }
    }

    fn invalid_data() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "文件包含无效或截断的多字节字节序列，请选择正确编码")
    }
}

impl<R: Read> Read for DecodeReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Read 的空缓冲调用不能触发底层 I/O、消费输出或推进解码器。
        if buf.is_empty() { return Ok(0); }
        loop {
            if self.outpos < self.outlen {
                let n = (self.outlen - self.outpos).min(buf.len());
                buf[..n].copy_from_slice(&self.out[self.outpos..self.outpos + n]);
                self.outpos += n;
                return Ok(n);
            }
            if self.decode_failed { return Err(Self::invalid_data()); }
            if self.decoder_finished { return Ok(0); }
            self.outpos = 0;
            self.outlen = 0;
            if self.needs_input && !self.eof { self.fill_more()?; }
            if let Some(decoder) = self.decoder.as_mut() {
                let (result, read, written) = decoder.decode_to_utf8_without_replacement(
                    &self.inbuf[self.inpos..self.inlen], &mut self.out, self.eof,
                );
                self.inpos += read;
                self.outlen = written;
                match result {
                    encoding_rs::DecoderResult::InputEmpty => {
                        // EOF 仍须以 last=true 调用一次；只有成功 flush 才可返回 0。
                        if self.eof { self.decoder_finished = true; }
                        else { self.needs_input = true; }
                    }
                    encoding_rs::DecoderResult::OutputFull => {
                        // 先排空输出，再将未消费输入和解码器内部待输出内容继续解码。
                        self.needs_input = false;
                        if read == 0 && written == 0 { self.decode_failed = true; }
                    }
                    encoding_rs::DecoderResult::Malformed(_, _) => self.decode_failed = true,
                }
            } else {
                // ISO-8859-1 每个字节映射同值 Unicode 码点（包括 NUL/C1 控制字符）。
                while self.inpos < self.inlen {
                    let byte = self.inbuf[self.inpos];
                    let needed = if byte < 0x80 { 1 } else { 2 };
                    if self.outlen + needed > self.out.len() { break; }
                    if needed == 1 {
                        self.out[self.outlen] = byte;
                    } else {
                        self.out[self.outlen] = 0xC0 | (byte >> 6);
                        self.out[self.outlen + 1] = 0x80 | (byte & 0x3F);
                    }
                    self.outlen += needed;
                    self.inpos += 1;
                }
                self.needs_input = self.inpos == self.inlen;
                self.decoder_finished = self.eof && self.needs_input;
            }
        }
    }
}

/// CSV 批：一批记录与估算字节数。
pub struct CsvBatch {
    pub rows: Vec<Vec<String>>,
    pub bytes: u64,
}

/// 增量 CSV 读取器：首条记录用于表头，之后按批产生数据行。
pub struct CsvStream {
    rdr: csv::Reader<DecodeReader<BufReader<File>>>,
    pub names: Vec<String>,
    first: Option<csv::StringRecord>,
    done: bool,
}

impl CsvStream {
    pub fn open(path: &str, encoding: &str, delimiter: char, has_header: bool) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("无法打开文件: {path}"))?;
        let meta = file.metadata()?;
        if !meta.is_file() { bail!("导入文件必须是普通文件"); }
        let reader = DecodeReader::new(BufReader::new(file), encoding)?;
        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delimiter as u8)
            .has_headers(false)
            .flexible(false)
            .from_reader(reader);
        let mut first = None;
        let mut names = Vec::new();
        let mut iter = rdr.records();
        match iter.next() {
            None => {}
            Some(rec) => {
                let rec = rec.context("CSV 第 1 条记录无效")?;
                if has_header {
                    names = rec.iter().map(str::to_string).collect();
                } else {
                    names = (0..rec.len()).map(|i| format!("column{}", i + 1)).collect();
                    first = Some(rec);
                }
            }
        }
        Ok(Self { rdr: rdr, names, first, done: false })
    }

    pub fn next_batch(&mut self, limits: &TransferLimits) -> Result<Option<CsvBatch>> {
        if self.done { return Ok(None); }
        let mut rows = Vec::new();
        let mut bytes = 0u64;
        let mut record = self.first.take();
        loop {
            if record.is_none() {
                match self.rdr.records().next() {
                    None => { self.done = true; break; }
                    Some(rec) => record = Some(rec.context("CSV 记录无效")?),
                }
            }
            let rec = record.take().unwrap();
            let row: Vec<String> = rec.iter().map(str::to_string).collect();
            let row_bytes: u64 = row.iter().map(|c| c.len() as u64 + 1).sum();
            if row_bytes > limits.max_record_bytes {
                bail!("CSV 单条记录超过上限 {} 字节，已停止导入（未修改目标）", limits.max_record_bytes);
            }
            bytes += row_bytes;
            rows.push(row);
            if rows.len() as u64 >= limits.batch_rows || bytes >= limits.batch_bytes {
                break;
            }
        }
        if rows.is_empty() { Ok(None) } else { Ok(Some(CsvBatch { rows, bytes })) }
    }
}

/// 增量 JSON 数组读取器：逐个对象解码，不解析整个顶层数组。
pub struct JsonStream {
    rdr: BufReader<DecodeReader<BufReader<File>>>,
    pub names: Vec<String>,
    started: bool,
    finished: bool,
    pending: Vec<u8>,
    max_record_bytes: u64,
    had_value: bool,
}

impl JsonStream {
    pub fn open(path: &str, encoding: &str, max_record_bytes: u64) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("无法打开文件: {path}"))?;
        let reader = DecodeReader::new(BufReader::new(file), encoding)?;
        Ok(Self { rdr: BufReader::new(reader), names: Vec::new(), started: false, finished: false, pending: Vec::new(), max_record_bytes, had_value: false })
    }

    fn read_byte(&mut self) -> Result<Option<u8>> {
        if let Some(b) = self.pending.pop() { return Ok(Some(b)); }
        let mut one = [0u8; 1];
        match self.rdr.read(&mut one)? {
            0 => Ok(None),
            _ => Ok(Some(one[0])),
        }
    }

    fn push_back(&mut self, b: u8) { self.pending.push(b); }

    fn skip_ws(&mut self) -> Result<Option<u8>> {
        loop {
            match self.read_byte()? {
                None => return Ok(None),
                Some(b) if b.is_ascii_whitespace() => continue,
                Some(b) => return Ok(Some(b)),
            }
        }
    }

    /// 读取下一个 JSON 值；数组结束后返回 None。
    pub fn next_value(&mut self) -> Result<Option<serde_json::Value>> {
        if self.finished { return Ok(None); }
        if !self.started {
            match self.skip_ws()? {
                Some(b'[') => self.started = true,
                Some(_) => bail!("JSON 导入要求顶层为对象数组（以 [ 开始）"),
                None => bail!("JSON 文件为空"),
            }
        }
        let mut next = self.skip_ws()?;
        if next == Some(b',') {
            if !self.had_value { bail!("JSON 数组出现多余逗号"); }
            next = self.skip_ws()?;
        }
        match next {
            None => bail!("JSON 数组未闭合"),
            Some(b']') => { self.finished = true; return Ok(None); }
            Some(b',') => bail!("JSON 数组出现多余逗号"),
            Some(b) => self.push_back(b),
        }
        let mut buf = Vec::new();
        let mut depth_obj = 0i32;
        let mut depth_arr = 0i32;
        let mut in_string = false;
        let mut escape = false;
        loop {
            let Some(b) = self.read_byte()? else { bail!("JSON 值未结束（文件提前结束）") };
            if in_string {
                buf.push(b);
                if escape { escape = false; }
                else if b == b'\\' { escape = true; }
                else if b == b'"' { in_string = false; }
                continue;
            }
            match b {
                b'"' => { in_string = true; buf.push(b); }
                b'{' => { depth_obj += 1; buf.push(b); }
                b'}' => {
                    depth_obj -= 1;
                    buf.push(b);
                    if depth_obj == 0 && depth_arr == 0 { break; }
                }
                b'[' => { depth_arr += 1; buf.push(b); }
                b']' => {
                    if depth_arr == 0 {
                        // 当前值直接结束（如原始数字或字符串后紧跟数组结束）。
                        if buf.is_empty() { self.finished = true; return Ok(None); }
                        self.push_back(b);
                        break;
                    }
                    depth_arr -= 1;
                    buf.push(b);
                    if depth_obj == 0 && depth_arr == 0 { break; }
                }
                b',' if depth_obj == 0 && depth_arr == 0 => { self.push_back(b); break; }
                _ if b.is_ascii_whitespace() && depth_obj == 0 && depth_arr == 0 => {
                    if !buf.is_empty() {
                        // 数字/布尔/null 后的分隔符。
                        self.push_back(b);
                        break;
                    }
                }
                _ => buf.push(b),
            }
            if buf.len() as u64 > self.max_record_bytes {
                bail!("JSON 单条对象超过上限 {} 字节，已停止导入（未修改目标）", self.max_record_bytes);
            }
        }
        let text = String::from_utf8(buf).context("JSON 不是有效的 UTF-8")?;
        let value: serde_json::Value = serde_json::from_str(text.trim()).context("JSON 对象解析失败")?;
        self.had_value = true;
        Ok(Some(value))
    }

    /// 预扫描整个数组的键并集（流式，有界内存），用于后续逐批写入。
    pub fn scan_names(path: &str, encoding: &str, max_record_bytes: u64) -> Result<Vec<String>> {
        let mut stream = Self::open(path, encoding, max_record_bytes)?;
        let mut names = Vec::new();
        while let Some(value) = stream.next_value()? {
            let map = value.as_object().ok_or_else(|| anyhow::anyhow!("JSON 每一行都必须是对象"))?;
            for key in map.keys() {
                if !names.contains(key) { names.push(key.clone()); }
            }
        }
        Ok(names)
    }

    pub fn next_batch(&mut self, limits: &TransferLimits) -> Result<Option<Vec<Vec<Option<String>>>>> {
        let mut rows = Vec::new();
        let mut bytes = 0u64;
        while (rows.len() as u64) < limits.batch_rows && bytes < limits.batch_bytes {
            let Some(value) = self.next_value()? else { break };
            let map = value.as_object().ok_or_else(|| anyhow::anyhow!("JSON 每一行都必须是对象"))?;
            for key in map.keys() {
                if !self.names.contains(key) { self.names.push(key.clone()); }
            }
            let row: Vec<Option<String>> = self.names.iter().map(|n| match value.get(n) {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(v) => Some(v.to_string()),
            }).collect();
            bytes += row.iter().map(|c| c.as_ref().map_or(1, |s| s.len() as u64 + 1)).sum::<u64>();
            rows.push(row);
        }
        if rows.is_empty() { Ok(None) } else { Ok(Some(rows)) }
    }
}

/// 分页读取一张表（只读导出/快照）。`order_cols` 为空时按驱动默认顺序。
pub async fn fetch_page(
    db: &Db,
    kind: &DbKind,
    schema: &str,
    table: &str,
    clause: Option<&str>,
    order_cols: &[String],
    offset: u64,
    limit: u64,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    crate::transfer::fetch_transfer_page(db, kind, schema, table, clause, order_cols, offset, limit).await

    }).await
}

/// 计算分页顺序：优先使用主键列，没有主键时返回空（按驱动顺序）。
pub async fn order_columns(db: &Db, kind: &DbKind, schema: &str, table: &str) -> Result<Vec<String>> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let meta = crate::schema::table_schema(db, kind.clone(), schema, table).await?;
    Ok(meta.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect())

    }).await
}

pub fn table_ref(kind: &DbKind, schema: &str, table: &str) -> String { qualified(kind, schema, table) }

pub fn exists(path: &str) -> bool { Path::new(path).is_file() }

// 说明：`queue_batches` 作为下游（前端一次只消费若干批）的预算上限保留在限制结构中；
// 后端当前以同步批处理 + 事务提交实现背压，不额外起后台预取任务。

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("navkit-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(content).unwrap();
        path
    }

    #[test]
    fn csv_stream_respects_batch_limits_and_quotes() {
        let path = temp_file("a.csv", b"id,name\r\n1,\"a,b\"\r\n2,\"line\nbreak\"\r\n");
        let mut s = CsvStream::open(path.to_str().unwrap(), "utf-8", ',', true).unwrap();
        assert_eq!(s.names, vec!["id", "name"]);
        let mut limits = TransferLimits::default();
        limits.batch_rows = 1;
        let b1 = s.next_batch(&limits).unwrap().unwrap();
        assert_eq!(b1.rows, vec![vec!["1".to_string(), "a,b".to_string()]]);
        let b2 = s.next_batch(&limits).unwrap().unwrap();
        assert_eq!(b2.rows[0][1], "line\nbreak");
        assert!(s.next_batch(&limits).unwrap().is_none());
    }

    #[test]
    fn csv_stream_without_header_names_columns() {
        let path = temp_file("b.csv", b"1,a\n2,b\n");
        let mut s = CsvStream::open(path.to_str().unwrap(), "utf-8", ',', false).unwrap();
        assert_eq!(s.names, vec!["column1", "column2"]);
        let batch = s.next_batch(&TransferLimits::default()).unwrap().unwrap();
        assert_eq!(batch.rows.len(), 2);
        assert_eq!(batch.rows[0], vec!["1".to_string(), "a".to_string()]);
    }

    #[test]
    fn csv_record_over_limit_fails() {
        let path = temp_file("c.csv", b"id\n1234567890\n");
        let mut s = CsvStream::open(path.to_str().unwrap(), "utf-8", ',', true).unwrap();
        let mut limits = TransferLimits::default();
        limits.max_record_bytes = 1;
        assert!(s.next_batch(&limits).is_err());
    }

    #[test]
    fn json_stream_reads_array_elements_incrementally() {
        let path = temp_file("d.json", br#"[{"id":1,"name":"a"}, {"id":2,"name":"b"}]"#);
        let names = JsonStream::scan_names(path.to_str().unwrap(), "utf-8", DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(names, vec!["id", "name"]);
        let mut s = JsonStream::open(path.to_str().unwrap(), "utf-8", DEFAULT_MAX_RECORD_BYTES).unwrap();
        s.names = names;
        let mut limits = TransferLimits::default();
        limits.batch_rows = 1;
        let b1 = s.next_batch(&limits).unwrap().unwrap();
        assert_eq!(b1[0][0], Some("1".to_string()));
        let b2 = s.next_batch(&limits).unwrap().unwrap();
        assert_eq!(b2[0][1], Some("b".to_string()));
        assert!(s.next_batch(&limits).unwrap().is_none());
    }

    #[test]
    fn json_stream_rejects_non_array() {
        let path = temp_file("e.json", br#"{"id":1}"#);
        let mut s = JsonStream::open(path.to_str().unwrap(), "utf-8", DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert!(s.next_value().is_err());
    }

    #[test]
    fn limits_are_clamped() {
        let l = TransferLimits { batch_rows: 0, batch_bytes: 1, max_record_bytes: 1, queue_batches: 0, spill_budget_bytes: None }.normalized();
        assert_eq!(l.batch_rows, 1);
        assert_eq!(l.batch_bytes, 64 * 1024);
        assert_eq!(l.max_record_bytes, 1024 * 1024);
        assert_eq!(l.queue_batches, 1);
    }

    #[test]
    fn decode_reader_handles_gbk() {
        // "中文" 的 GBK 字节
        let path = temp_file("f.csv", &[0xD6u8, 0xD0, 0xCE, 0xC4]);
        let mut r = DecodeReader::new(BufReader::new(File::open(&path).unwrap()), "gbk").unwrap();
        let mut out = String::new();
        r.read_to_string(&mut out).unwrap();
        assert_eq!(out, "中文");
    }

    struct ChunkReader {
        bytes: Vec<u8>,
        pos: usize,
        chunk: usize,
        calls: usize,
        interrupt_every_other: bool,
    }

    impl ChunkReader {
        fn new(bytes: &[u8], chunk: usize) -> Self {
            assert!(chunk > 0);
            Self { bytes: bytes.to_vec(), pos: 0, chunk, calls: 0, interrupt_every_other: false }
        }
    }

    impl Read for ChunkReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            assert!(!buf.is_empty(), "DecodeReader must not forward empty reads");
            assert!(buf.len() <= READ_CHUNK, "input requests must remain bounded");
            if self.interrupt_every_other && self.calls % 2 == 1 {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "test interruption"));
            }
            let n = buf.len().min(self.chunk).min(self.bytes.len() - self.pos);
            buf[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    fn decode_in_chunks(bytes: &[u8], encoding: &str, input_chunk: usize, output_chunk: usize) -> Vec<u8> {
        let mut reader = DecodeReader::new(ChunkReader::new(bytes, input_chunk), encoding).unwrap();
        let mut buf = vec![0; output_chunk];
        assert!(!buf.is_empty());
        let mut output = Vec::new();
        // A bounded number of reads detects premature EOF and accidental no-progress loops.
        for _ in 0..bytes.len().saturating_mul(4) + 16 {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                assert!(reader.decoder_finished);
                assert!(!reader.decode_failed);
                let calls = reader.inner.calls;
                assert_eq!(reader.read(&mut buf).unwrap(), 0);
                assert_eq!(reader.inner.calls, calls, "finished readers must not poll the source again");
                return output;
            }
            output.extend_from_slice(&buf[..n]);
            assert!(output.len() <= bytes.len().saturating_mul(4) + 4);
        }
        panic!("decoder did not finish within its bounded read budget");
    }

    fn assert_decode_failure(bytes: &[u8], encoding: &str, input_chunk: usize, prefix: &[u8]) {
        let mut reader = DecodeReader::new(ChunkReader::new(bytes, input_chunk), encoding).unwrap();
        let mut buf = [0; 3];
        let mut output = Vec::new();
        for _ in 0..bytes.len().saturating_mul(4) + 16 {
            match reader.read(&mut buf) {
                Ok(0) => panic!("malformed or truncated input was silently treated as EOF"),
                Ok(n) => output.extend_from_slice(&buf[..n]),
                Err(error) => {
                    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                    assert_eq!(output, prefix, "valid prefix must be delivered, but bytes after the error must not");
                    assert!(reader.decode_failed);
                    let calls = reader.inner.calls;
                    for _ in 0..3 {
                        assert_eq!(reader.read(&mut buf).unwrap_err().kind(), io::ErrorKind::InvalidData);
                    }
                    assert_eq!(reader.read(&mut []).unwrap(), 0);
                    assert_eq!(reader.inner.calls, calls, "decode failure must remain sticky without reading more input");
                    return;
                }
            }
        }
        panic!("malformed input did not fail within its bounded read budget");
    }

    #[test]
    fn decode_reader_utf8_short_chunks_preserve_nul_and_multibyte() {
        let text = "A\0中🙂\r\néZ";
        for input_chunk in 1..=5 {
            for output_chunk in 1..=7 {
                assert_eq!(decode_in_chunks(text.as_bytes(), "utf-8", input_chunk, output_chunk), text.as_bytes());
            }
        }
    }

    #[test]
    fn decode_reader_utf8_bom_split_across_short_chunks() {
        let text = "标题\0\u{feff}末尾";
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(text.as_bytes());
        for encoding in ["", "utf8", "UTF-8"] {
            for input_chunk in 1..=3 {
                for output_chunk in 1..=4 {
                    assert_eq!(decode_in_chunks(&bytes, encoding, input_chunk, output_chunk), text.as_bytes());
                }
            }
            assert!(decode_in_chunks(&[], encoding, 1, 1).is_empty());
            assert!(decode_in_chunks(&[0xEF, 0xBB, 0xBF], encoding, 1, 1).is_empty());
        }
    }

    #[test]
    fn decode_reader_does_not_bom_switch_selected_encoding() {
        for encoding in ["utf-8", "gbk", "gb18030", "big5"] {
            assert_decode_failure(&[0xFF, 0xFE, b'A', 0], encoding, 1, b"");
        }
        assert_eq!(decode_in_chunks(&[0xEF, 0xBB, 0xBF, b'A'], "latin1", 1, 1), "ï»¿A".as_bytes());
    }

    #[test]
    fn decode_reader_gb18030_four_byte_sequence_split_at_every_boundary() {
        // 81 30 81 30 is GB18030 U+0080; the other pairs are GBK 中文.
        let bytes = [b'A', 0x81, 0x30, 0x81, 0x30, 0xD6, 0xD0, 0xCE, 0xC4, 0, b'Z'];
        let expected = "A\u{80}中文\0Z".as_bytes();
        for encoding in ["gb18030", "gbk", "gb2312", "GB18030"] {
            for input_chunk in 1..=5 {
                for output_chunk in 1..=5 {
                    assert_eq!(decode_in_chunks(&bytes, encoding, input_chunk, output_chunk), expected);
                }
            }
        }
    }

    #[test]
    fn decode_reader_big5_short_chunks_preserve_combining_mapping() {
        // Big5 88 62 maps to two Unicode scalars, not one replacement character.
        let bytes = [0xA4, 0xA4, 0xA4, 0xE5, 0x88, 0x62, 0];
        let expected = "中文Ê\u{304}\0".as_bytes();
        for input_chunk in 1..=4 {
            for output_chunk in 1..=5 {
                assert_eq!(decode_in_chunks(&bytes, "big5", input_chunk, output_chunk), expected);
            }
        }
    }

    #[test]
    fn decode_reader_latin1_converts_all_bytes_to_utf8() {
        let bytes: Vec<u8> = (0..=255).collect();
        let expected: String = bytes.iter().map(|byte| char::from(*byte)).collect();
        for encoding in ["latin1", "iso-8859-1", "ISO-8859-1"] {
            for input_chunk in [1, 2, 17, 256] {
                for output_chunk in [1, 7, 65] {
                    assert_eq!(decode_in_chunks(&bytes, encoding, input_chunk, output_chunk), expected.as_bytes());
                }
            }
        }
        // The C1 range is ISO-8859-1, not WHATWG Windows-1252 (0x80 must not become €).
        assert_eq!(decode_in_chunks(&[0, 0x80, 0x9F, 0xFF], "latin1", 1, 1), "\0\u{80}\u{9f}ÿ".as_bytes());
    }

    #[test]
    fn decode_reader_empty_read_does_not_read_or_advance() {
        let mut reader = DecodeReader::new(ChunkReader::new(b"abcd", 4), "utf-8").unwrap();
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        assert_eq!(reader.inner.calls, 0);
        assert!(!reader.eof);
        let mut byte = [0; 1];
        assert_eq!(reader.read(&mut byte).unwrap(), 1);
        assert_eq!(byte, [b'a']);
        let state = (reader.inpos, reader.inlen, reader.outpos, reader.outlen, reader.needs_input, reader.eof);
        let calls = reader.inner.calls;
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        assert_eq!((reader.inpos, reader.inlen, reader.outpos, reader.outlen, reader.needs_input, reader.eof), state);
        assert_eq!(reader.inner.calls, calls);
        let mut rest = [0; 8];
        assert_eq!(reader.read(&mut rest).unwrap(), 3);
        assert_eq!(&rest[..3], b"bcd");
        assert_eq!(reader.read(&mut rest).unwrap(), 0);
        let calls = reader.inner.calls;
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        assert_eq!(reader.inner.calls, calls);
    }

    #[test]
    fn decode_reader_retries_interrupted_reads_including_eof() {
        let text = "é中\0";
        let mut source = ChunkReader::new(text.as_bytes(), 1);
        source.interrupt_every_other = true;
        let mut reader = DecodeReader::new(source, "utf-8").unwrap();
        let mut output = Vec::new();
        let mut buf = [0; 2];
        for _ in 0..64 {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 { break; }
            output.extend_from_slice(&buf[..n]);
        }
        assert!(reader.decoder_finished);
        assert_eq!(output, text.as_bytes());
        assert_eq!(reader.inner.calls, 2 * (text.len() + 1));
    }

    #[test]
    fn decode_reader_flushes_truncated_utf8_and_partial_bom() {
        for suffix in [&[0xC2][..], &[0xE4, 0xB8][..], &[0xF0, 0x9F, 0x92][..]] {
            let mut bytes = b"ok:".to_vec();
            bytes.extend_from_slice(suffix);
            for chunk in [1, 2, 8] { assert_decode_failure(&bytes, "utf-8", chunk, b"ok:"); }
        }
        assert_decode_failure(&[0xEF], "utf-8", 1, b"");
        assert_decode_failure(&[0xEF, 0xBB], "utf-8", 1, b"");
    }

    #[test]
    fn decode_reader_flushes_truncated_gbk_big5_and_gb18030() {
        for (encoding, suffix) in [
            ("gbk", &[0xD6][..]), ("gb2312", &[0xCE][..]),
            ("big5", &[0xA4][..]), ("gb18030", &[0x81, 0x30][..]),
            ("gb18030", &[0x81, 0x30, 0x81][..]),
        ] {
            let mut bytes = b"ok:".to_vec();
            bytes.extend_from_slice(suffix);
            for chunk in [1, 2, 8] { assert_decode_failure(&bytes, encoding, chunk, b"ok:"); }
        }
    }

    #[test]
    fn decode_reader_delivers_valid_prefix_then_sticky_invalid_data() {
        for (encoding, suffix) in [
            ("utf-8", &[0xFF][..]), ("utf-8", &[0xE4, b'X'][..]),
            ("utf-8", &[0xC0, 0xAF][..]), ("utf-8", &[0xED, 0xA0, 0x80][..]),
            ("utf-8", &[0xF4, 0x90, 0x80, 0x80][..]),
            ("gbk", &[0xFF][..]), ("big5", &[0xFF][..]),
        ] {
            let mut bytes = b"ok:".to_vec();
            bytes.extend_from_slice(suffix);
            bytes.extend_from_slice(b"must-not-be-delivered");
            for chunk in [1, 2, 64] { assert_decode_failure(&bytes, encoding, chunk, b"ok:"); }
        }
    }

    #[test]
    fn decode_reader_output_full_keeps_unconsumed_big5_input() {
        let mut bytes = vec![b'a'; READ_CHUNK - 3];
        bytes.extend_from_slice(&[0x88, 0x62, 0xA4, 0xA4]);
        let mut expected = vec![b'a'; READ_CHUNK - 3];
        expected.extend_from_slice("Ê\u{304}中".as_bytes());
        assert_eq!(decode_in_chunks(&bytes, "big5", READ_CHUNK, 4097), expected);
    }

    // Generates many MiB without materializing the source or the decoded result.
    struct RepeatingReader {
        pattern: Vec<u8>,
        remaining: usize,
        offset: usize,
        calls: usize,
        max_requested: usize,
    }

    impl Read for RepeatingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            assert!(!buf.is_empty());
            self.max_requested = self.max_requested.max(buf.len());
            let n = self.remaining.min(buf.len());
            for (i, byte) in buf[..n].iter_mut().enumerate() {
                *byte = self.pattern[(self.offset + i) % self.pattern.len()];
            }
            self.offset += n;
            self.remaining -= n;
            Ok(n)
        }
    }

    fn assert_large_decode_bounded(encoding: &str, input_pattern: &[u8], output_pattern: &[u8], repeats: usize) {
        let source = RepeatingReader {
            pattern: input_pattern.to_vec(), remaining: input_pattern.len() * repeats,
            offset: 0, calls: 0, max_requested: 0,
        };
        let mut reader = DecodeReader::new(source, encoding).unwrap();
        let input_ptr = reader.inbuf.as_ptr();
        let output_ptr = reader.out.as_ptr();
        let mut buf = [0; 8191];
        let mut total = 0;
        let expected_bytes = output_pattern.len() * repeats;
        for _ in 0..expected_bytes / buf.len() + 1024 {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 { break; }
            if total == 0 {
                assert!(reader.inner.offset <= READ_CHUNK, "the first read must not drain the source");
                assert!(reader.inner.remaining > READ_CHUNK);
            }
            for (i, byte) in buf[..n].iter().enumerate() {
                assert_eq!(*byte, output_pattern[(total + i) % output_pattern.len()]);
            }
            total += n;
            assert!(total <= expected_bytes);
            assert_eq!(reader.inbuf.len(), READ_CHUNK);
            assert_eq!(reader.inbuf.capacity(), READ_CHUNK);
            assert_eq!(reader.out.len(), READ_CHUNK);
            assert_eq!(reader.out.capacity(), READ_CHUNK);
            assert_eq!(reader.inbuf.as_ptr(), input_ptr, "input buffer must be reused");
            assert_eq!(reader.out.as_ptr(), output_ptr, "output buffer must be reused");
            assert!(reader.inpos <= reader.inlen && reader.inlen <= READ_CHUNK);
            assert!(reader.outpos <= reader.outlen && reader.outlen <= READ_CHUNK);
        }
        assert_eq!(total, expected_bytes);
        assert!(reader.decoder_finished);
        assert!(!reader.decode_failed);
        assert_eq!(reader.inner.remaining, 0);
        assert!(reader.inner.max_requested <= READ_CHUNK);
        assert_eq!(reader.inner.calls, (input_pattern.len() * repeats + READ_CHUNK - 1) / READ_CHUNK + 1);
    }

    #[test]
    fn decode_reader_large_latin1_stream_uses_fixed_buffers() {
        // 8 MiB of source expands to 12 MiB of UTF-8 without accumulating either.
        assert_large_decode_bounded("latin1", &[0, 0x80, 0xFF, b'\n'], &[0, 0xC2, 0x80, 0xC3, 0xBF, b'\n'], 2 * 1024 * 1024);
    }

    #[test]
    fn decode_reader_large_big5_stream_uses_fixed_buffers() {
        assert_large_decode_bounded("big5", &[0x88, 0x62, 0xA4, 0xA4, 0], "Ê\u{304}中\0".as_bytes(), 512 * 1024);
    }

    #[test]
    fn decode_reader_large_gb18030_stream_uses_fixed_buffers() {
        assert_large_decode_bounded("gb18030", &[0x81, 0x30, 0x81, 0x30, 0xD6, 0xD0, 0], "\u{80}中\0".as_bytes(), 512 * 1024);
    }
}
