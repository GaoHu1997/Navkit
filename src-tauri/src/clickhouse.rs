//! ClickHouse 原生 HTTP 接口驱动（默认 8123 端口）。
//!
//! SELECT / SHOW / DESCRIBE 以 `FORMAT JSONCompactEachRowWithNamesAndTypes` 执行，
//! 前两行为列名与列类型，其后为数据行；其余语句按普通 SQL 执行。

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use serde_json::Value;

use crate::models::ConnectionRequest;

/// describe 成功响应最多读取的字节数（只含列名/类型两行，超出视为异常）。
const MAX_DESCRIBE_BYTES: usize = 8 * 1024 * 1024;
/// 错误响应体最多读取的字节数（仅用于错误信息，超出部分丢弃）。
const MAX_ERROR_BYTES: usize = 64 * 1024;
/// 流式解析单行绝对上限，防止异常服务端不发换行导致无界增长。
const MAX_STREAM_LINE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct ClickHouseClient {
    /// 普通查询客户端：带整请求总超时（预览/小查询）。
    http: reqwest::Client,
    /// 流式导出专用客户端：只设连接超时与读空闲超时，不设整请求总超时，
    /// 否则大表导出会在读取响应体阶段被总超时中途掐断。
    stream_http: reqwest::Client,
    base_url: String,
    database: String,
    user: String,
    password: String,
}

/// 一次查询的结果：列名 + 全部数据行。
pub struct ChResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl ClickHouseClient {
    pub fn connect(req: &ConnectionRequest) -> Result<Self> {
        let port = if req.port == 0 { 8123 } else { req.port };
        let base_url = format!("http://{}:{}", req.host, port);
        let total_secs = if req.timeout_secs < 5 { 30 } else { req.timeout_secs };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(total_secs))
            .build()
            .context("初始化 HTTP 客户端失败")?;
        // 流式客户端不设总超时：read_timeout 是"两次读到数据之间的最长空闲"，
        // 只要服务器持续产出就不会中断，同时仍能回收真正卡死的连接。
        let stream_http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(total_secs.clamp(5, 30)))
            .read_timeout(Duration::from_secs(total_secs.max(5)))
            .build()
            .context("初始化流式 HTTP 客户端失败")?;
        Ok(Self {
            http,
            stream_http,
            base_url,
            database: req.database.clone(),
            user: req.user.clone(),
            password: req.password.clone(),
        })
    }

    fn post(&self, body: &str) -> reqwest::RequestBuilder {
        self.http
            .post(&self.base_url)
            .basic_auth(if self.user.is_empty() { "default" } else { &self.user }, Some(&self.password))
            .header("X-ClickHouse-Database", &self.database)
            .body(body.to_string())
    }

    /// 流式导出使用专用客户端（无整请求总超时）。
    fn post_stream(&self, body: &str) -> reqwest::RequestBuilder {
        self.stream_http
            .post(&self.base_url)
            .basic_auth(if self.user.is_empty() { "default" } else { &self.user }, Some(&self.password))
            .header("X-ClickHouse-Database", &self.database)
            .body(body.to_string())
    }

    /// 执行返回结果集的查询（自动追加 JSON 格式声明）。
    pub async fn query_rows(&self, sql: &str) -> Result<ChResult> {
        let mut sql_history=crate::sql_history::Guard::new(&format!("{}\nFORMAT JSONCompactEachRowWithNamesAndTypes", sql.trim_end_matches(';').trim()),Vec::new());
        let result: Result<ChResult> = async {
        let body = format!("{}\nFORMAT JSONCompactEachRowWithNamesAndTypes", sql.trim_end_matches(';').trim());
        let resp = self
            .post(&body)
            .send()
            .await
            .context("无法连接到 ClickHouse 服务器")?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("读取 ClickHouse 响应失败")?;
        if !status.is_success() {
            return Err(anyhow!(
                "ClickHouse 错误: {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        parse_json_compact(&bytes)
    
        }.await;
        sql_history.result(&result,|r|r.rows.len() as i64);
        result
    }

    /// 导出用真实定义读取：限制元数据响应大小，不吞掉权限/对象不存在错误。
    pub(crate) async fn export_definition(&self, sql: &str) -> Result<String> {
        let body = format!("{}\nFORMAT JSONCompactEachRowWithNamesAndTypes", sql.trim_end_matches(';').trim());
        let mut guard = crate::sql_history::Guard::new(&body, Vec::new());
        let result = async {
            let response = self.post(&body).send().await.context("读取 ClickHouse 对象定义失败")?;
            if !response.status().is_success() {
                let bytes = read_error_prefix(response, MAX_ERROR_BYTES).await?;
                bail!("ClickHouse 对象定义读取失败: {}", String::from_utf8_lossy(&bytes));
            }
            let bytes = read_bounded(response, MAX_DESCRIBE_BYTES).await?;
            validate_export_definition(&bytes)?;
            let result = parse_json_compact(&bytes)?;
            if result.rows.len() != 1 || result.rows[0].len() != 1 { bail!("ClickHouse 对象定义返回形状异常"); }
            result.rows[0][0].clone().filter(|value| !value.trim().is_empty()).ok_or_else(|| anyhow!("ClickHouse 对象定义不可见"))
        }.await;
        guard.result(&result, |_| 1);
        result
    }

    /// 执行不返回结果集的语句（DDL / INSERT / ALTER），返回影响行数（尽力而为）。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let mut sql_history=crate::sql_history::Guard::new(sql.trim_end_matches(';').trim(),Vec::new());
        let result: Result<u64> = async {
        let resp = self
            .post(sql.trim_end_matches(';').trim())
            .send()
            .await
            .context("无法连接到 ClickHouse 服务器")?;
        let status = resp.status();
        let summary = resp
            .headers()
            .get("X-ClickHouse-Summary")
            .map(|v| v.as_bytes().to_vec());
        let bytes = resp.bytes().await.context("读取 ClickHouse 响应失败")?;
        if !status.is_success() {
            return Err(anyhow!(
                "ClickHouse 错误: {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        // X-ClickHouse-Summary 头里带 written_rows（JSON 形式）
        if let Some(summary) = summary {
            if let Ok(map) = serde_json::from_slice::<Value>(&summary) {
                if let Some(n) = map.get("written_rows").and_then(|v| v.as_str()) {
                    return Ok(n.parse().unwrap_or(0));
                }
            }
        }
        Ok(0)
    
        }.await;
        sql_history.result(&result,|n|*n as i64);
        result
    }

    /// 查询单值。
    pub async fn scalar(&self, sql: &str) -> Result<Option<String>> {
        let res = self.query_rows(sql).await?;
        Ok(res
            .rows
            .first()
            .and_then(|r| r.first().cloned())
            .flatten())
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    /// 只取列名与列类型，不读取任何数据行（用于建立有界导出 sink）。
    pub async fn describe(&self, sql: &str) -> Result<(Vec<String>, Vec<String>)> {
        let body = format!(
            "{}\nFORMAT JSONCompactEachRowWithNamesAndTypes",
            sql.trim_end_matches(';').trim()
        );
        let resp = self
            .post(&body)
            .send()
            .await
            .context("无法连接到 ClickHouse 服务器")?;
        let status = resp.status();
        if !status.is_success() {
            let bytes = read_error_prefix(resp, MAX_ERROR_BYTES).await?;
            return Err(anyhow!(
                "ClickHouse 错误: {}",
                String::from_utf8_lossy(&bytes).trim()
            ));
        }
        let bytes = read_bounded(resp, MAX_DESCRIBE_BYTES).await?;
        let text = std::str::from_utf8(&bytes).context("ClickHouse describe 响应不是合法 UTF-8")?;
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let columns = parse_name_row(lines.next(), "列名行")?;
        let types = parse_name_row(lines.next(), "类型行")?;
        if columns.is_empty() {
            bail!("ClickHouse describe 未返回列名");
        }
        if types.len() != columns.len() {
            bail!("ClickHouse describe 列名与类型数量不一致");
        }
        if lines.next().is_some() {
            bail!("ClickHouse describe 响应包含多余数据行");
        }
        Ok((columns, types))
    }

    /// 以流式方式执行查询，逐行把数据送进有界通道；不把整表读进内存。
    /// `types` 与 `names` 一起用于严格校验响应表头，`max_record_bytes` 为单行记录上限。
    /// Guard 从调用作用域（sql_history::scope 内）创建后 move 进任务，保持 SQL 日志上下文。
    pub fn spawn_row_stream(
        &self,
        sql: String,
        names: Vec<String>,
        types: Vec<String>,
        max_record_bytes: u64,
        tx: tokio::sync::mpsc::Sender<Vec<Option<String>>>,
    ) -> tokio::task::JoinHandle<Result<u64>> {
        let body = format!(
            "{}\nFORMAT JSONCompactEachRowWithNamesAndTypes",
            sql.trim_end_matches(';').trim()
        );
        let mut guard = crate::sql_history::Guard::new(&body, Vec::new());
        let client = self.clone();
        tokio::spawn(async move {
            let result: Result<u64> =
                stream_rows(client, body, names, types, max_record_bytes, tx).await;
            guard.result(&result, |count| *count as i64);
            result
        })
    }
}


/// 流式读取核心：请求、逐段拆行、校验、写入有界通道；接收端关闭时在
/// 请求 / stream.next / send 三处都可及时退出。
async fn stream_rows(
    client: ClickHouseClient,
    body: String,
    names: Vec<String>,
    types: Vec<String>,
    max_record_bytes: u64,
    tx: tokio::sync::mpsc::Sender<Vec<Option<String>>>,
) -> Result<u64> {
    let resp = tokio::select! {
        result = client.post_stream(&body).send() => result.context("无法连接到 ClickHouse 服务器")?,
        _ = tx.closed() => bail!("ClickHouse 导出接收端已关闭"),
    };
    let status = resp.status();
    if !status.is_success() {
        let bytes = read_error_prefix(resp, MAX_ERROR_BYTES).await?;
        return Err(anyhow!(
            "ClickHouse 错误: {}",
            String::from_utf8_lossy(&bytes).trim()
        ));
    }
    let mut stream = resp.bytes_stream();
    let mut parser = CompactStreamParser::new(names, types, max_record_bytes);
    loop {
        let next = tokio::select! {
            chunk = stream.next() => chunk,
            _ = tx.closed() => bail!("ClickHouse 导出接收端已关闭"),
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.context("读取 ClickHouse 响应流失败")?;
        for values in parser.push(&chunk)? {
            send_row(&tx, values).await?;
        }
    }
    for values in parser.finish()? {
        send_row(&tx, values).await?;
    }
    Ok(parser.data_rows())
}

/// 向接收端发送一行，接收端关闭时立即退出。
async fn send_row(
    tx: &tokio::sync::mpsc::Sender<Vec<Option<String>>>,
    values: Vec<Option<String>>,
) -> Result<()> {
    tokio::select! {
        result = tx.send(values) => result.map_err(|_| anyhow!("ClickHouse 导出接收端已关闭")),
        _ = tx.closed() => Err(anyhow!("ClickHouse 导出接收端已关闭")),
    }
}

/// 读取成功响应体，最多 cap 字节；超过立即报错，避免异常响应占满内存。
async fn read_bounded(resp: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let mut stream = resp.bytes_stream();
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取 ClickHouse 响应失败")?;
        if out.len().saturating_add(chunk.len()) > cap {
            bail!("ClickHouse 响应超过上限 {cap} 字节");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// 读取错误响应前缀（最多 cap 字节），超出部分丢弃，仅用于拼装错误信息。
async fn read_error_prefix(resp: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let mut stream = resp.bytes_stream();
    let mut out: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取 ClickHouse 响应失败")?;
        if out.len() >= cap {
            break;
        }
        let take = (cap - out.len()).min(chunk.len());
        out.extend_from_slice(&chunk[..take]);
        if out.len() >= cap {
            break;
        }
    }
    Ok(out)
}

/// 解析 describe 的一行（列名或类型），必须是字符串数组。
fn parse_name_row(line: Option<&str>, label: &str) -> Result<Vec<String>> {
    let line = line.ok_or_else(|| anyhow!("ClickHouse describe 响应缺少{label}"))?;
    let arr: Vec<Value> = serde_json::from_str(line)
        .with_context(|| format!("ClickHouse describe 解析失败（{label}）"))?;
    string_row(arr, label)
}

fn string_row(arr: Vec<Value>, label: &str) -> Result<Vec<String>> {
    arr.into_iter()
        .map(|value| match value {
            Value::String(text) => Ok(text),
            _ => Err(anyhow!("ClickHouse {label}含非字符串项")),
        })
        .collect()
}

/// `JSONCompactEachRowWithNamesAndTypes` 的增量解析器，与网络解耦。
/// `push` 可接收任意切分的字节段，返回本次解析出的数据行；`finish` 处理结尾
/// 无换行的最后一行并校验 header/类型行齐全。错误一律立即返回（超长行、
/// 非法 UTF-8、列名/类型不一致、行宽不符、记录超限、计数溢出）。
pub(crate) struct CompactStreamParser {
    names: Vec<String>,
    types: Vec<String>,
    max_line_bytes: usize,
    max_record_bytes: u64,
    buf: Vec<u8>,
    line_no: u64,
    data_rows: u64,
    saw_names: bool,
    saw_types: bool,
}

impl CompactStreamParser {
    pub(crate) fn new(names: Vec<String>, types: Vec<String>, max_record_bytes: u64) -> Self {
        let max_line_bytes = max_record_bytes
            .saturating_mul(2)
            .saturating_add(1024 * 1024)
            .min(MAX_STREAM_LINE_BYTES as u64) as usize;
        Self::with_limits(names, types, max_record_bytes, max_line_bytes)
    }

    fn with_limits(
        names: Vec<String>,
        types: Vec<String>,
        max_record_bytes: u64,
        max_line_bytes: usize,
    ) -> Self {
        Self {
            names,
            types,
            max_line_bytes,
            max_record_bytes,
            buf: Vec::with_capacity(256),
            line_no: 0,
            data_rows: 0,
            saw_names: false,
            saw_types: false,
        }
    }

    pub(crate) fn data_rows(&self) -> u64 {
        self.data_rows
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<Option<String>>>> {
        self.buf.extend_from_slice(chunk);
        let mut rows = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Some(row) = self.consume_line(&raw)? {
                rows.push(row);
            }
        }
        if self.buf.len() > self.max_line_bytes {
            bail!("ClickHouse 导出单行超过上限");
        }
        Ok(rows)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Vec<Option<String>>>> {
        let mut rows = Vec::new();
        if !self.buf.is_empty() {
            let raw = std::mem::take(&mut self.buf);
            if let Some(row) = self.consume_line(&raw)? {
                rows.push(row);
            }
        }
        if !self.saw_names {
            bail!("ClickHouse 响应缺少列名(header)行");
        }
        if !self.saw_types {
            bail!("ClickHouse 响应缺少类型行");
        }
        Ok(rows)
    }

    fn consume_line(&mut self, raw: &[u8]) -> Result<Option<Vec<Option<String>>>> {
        if raw.len() > self.max_line_bytes {
            bail!("ClickHouse 导出单行超过上限");
        }
        let mut end = raw.len();
        while end > 0 && matches!(raw[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        let line = &raw[..end];
        let text = std::str::from_utf8(line)
            .map_err(|e| anyhow!("ClickHouse 响应不是合法 UTF-8: {e}"))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let arr: Vec<Value> = serde_json::from_str(trimmed).context("ClickHouse 响应解析失败")?;
        match self.line_no {
            0 => {
                let got = string_row(arr, "列名行")?;
                if got.len() != self.names.len() || got != self.names {
                    bail!("ClickHouse 列名与预期不一致，拒绝错位导出");
                }
                self.line_no = 1;
                self.saw_names = true;
                Ok(None)
            }
            1 => {
                let got = string_row(arr, "类型行")?;
                if got.len() != self.types.len() || got != self.types {
                    bail!("ClickHouse 列类型与预期不一致，拒绝错位导出");
                }
                self.line_no = 2;
                self.saw_types = true;
                Ok(None)
            }
            _ => {
                let mut values: Vec<Option<String>> = Vec::with_capacity(arr.len());
                let mut record_bytes: u64 = 0;
                for value in arr.into_iter() {
                    let cell = json_cell(value);
                    record_bytes = record_bytes
                        .checked_add(cell.as_ref().map_or(0, |text| text.len()) as u64)
                        .ok_or_else(|| anyhow!("ClickHouse 导出记录长度溢出"))?;
                    values.push(cell);
                }
                if values.len() != self.names.len() {
                    bail!("ClickHouse 数据行列数与列名不一致");
                }
                if record_bytes > self.max_record_bytes {
                    bail!("ClickHouse 导出单条记录超过上限");
                }
                self.data_rows = self
                    .data_rows
                    .checked_add(1)
                    .ok_or_else(|| anyhow!("ClickHouse 导出行数溢出"))?;
                Ok(Some(values))
            }
        }
    }
}

#[cfg(test)]
mod stream_parser_tests {
    use super::CompactStreamParser;

    fn schema() -> (Vec<String>, Vec<String>) {
        (
            vec!["a".to_string(), "b".to_string()],
            vec!["UInt64".to_string(), "String".to_string()],
        )
    }

    fn parser(max_record_bytes: u64) -> CompactStreamParser {
        let (names, types) = schema();
        CompactStreamParser::new(names, types, max_record_bytes)
    }

    const HEADER: &str = "[\"a\",\"b\"]\n[\"UInt64\",\"String\"]\n";

    #[test]
    fn parses_chunks_split_at_arbitrary_boundaries() {
        let payload = format!("{HEADER}[1,\"x\"]\n[2,\"y\"]\n");
        let mut p = parser(1024);
        let mut rows = Vec::new();
        for byte in payload.bytes() {
            rows.extend(p.push(&[byte]).unwrap());
        }
        rows.extend(p.finish().unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec![Some("1".to_string()), Some("x".to_string())]);
        assert_eq!(p.data_rows(), 2);
    }

    #[test]
    fn parses_last_row_without_trailing_newline() {
        let payload = format!("{HEADER}[7,\"z\"]");
        let mut p = parser(1024);
        let mut rows = p.push(payload.as_bytes()).unwrap();
        rows.extend(p.finish().unwrap());
        assert_eq!(rows, vec![vec![Some("7".to_string()), Some("z".to_string())]]);
    }

    #[test]
    fn skips_blank_lines_around_header() {
        let payload = format!("\n\n{HEADER}\n[3,\"v\"]\n\n");
        let mut p = parser(1024);
        let mut rows = p.push(payload.as_bytes()).unwrap();
        rows.extend(p.finish().unwrap());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Some("3".to_string()));
    }

    #[test]
    fn errors_on_late_bad_json() {
        let payload = format!("{HEADER}[1,\"x\"]\nnot-json\n[2,\"y\"]\n");
        let mut p = parser(1024);
        assert!(p.push(payload.as_bytes()).is_err());
    }

    #[test]
    fn errors_on_overlong_line() {
        let (names, types) = schema();
        let mut p = CompactStreamParser::with_limits(names, types, 1024, 16);
        let long = vec![b'x'; 64];
        assert!(p.push(&long).is_err());
    }

    #[test]
    fn errors_on_type_row_mismatch() {
        let payload = "[\"a\",\"b\"]\n[\"UInt64\",\"Int32\"]\n";
        let mut p = parser(1024);
        assert!(p.push(payload.as_bytes()).is_err());
    }

    #[test]
    fn errors_on_column_name_mismatch() {
        let payload = "[\"a\",\"c\"]\n[\"UInt64\",\"String\"]\n";
        let mut p = parser(1024);
        assert!(p.push(payload.as_bytes()).is_err());
    }

    #[test]
    fn errors_when_header_missing() {
        let mut p = parser(1024);
        assert!(p.finish().is_err());
    }

    #[test]
    fn errors_when_type_row_missing() {
        let mut p = parser(1024);
        p.push(b"[\"a\",\"b\"]\n").unwrap();
        assert!(p.finish().is_err());
    }

    #[test]
    fn errors_on_invalid_utf8() {
        let mut p = parser(1024);
        let mut payload = HEADER.as_bytes().to_vec();
        payload.extend_from_slice(b"[1,\"\xff\"]\n");
        assert!(p.push(&payload).is_err());
    }

    #[test]
    fn enforces_record_byte_limit() {
        let mut p = parser(4);
        let payload = format!("{HEADER}[12345,\"x\"]\n");
        assert!(p.push(payload.as_bytes()).is_err());
    }
}

/// 解析 JSONCompactEachRowWithNamesAndTypes：第 1 行列名、第 2 行类型、其余数据。
/// 严格验证导出定义，禁止浏览解析器的 UTF-8 替换或非字符串转型。
fn validate_export_definition(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("ClickHouse 对象定义响应不是合法 UTF-8")?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let names: Vec<String> = serde_json::from_str(lines.next().ok_or_else(|| anyhow!("ClickHouse 对象定义缺少列名"))?)?;
    let types: Vec<String> = serde_json::from_str(lines.next().ok_or_else(|| anyhow!("ClickHouse 对象定义缺少类型"))?)?;
    let values: Vec<String> = serde_json::from_str(lines.next().ok_or_else(|| anyhow!("ClickHouse 对象定义不可见"))?)?;
    if names.len() != 1 || names[0].trim().is_empty() || types != ["String"] || values.len() != 1 || lines.next().is_some() {
        bail!("ClickHouse 对象定义返回形状异常");
    }
    if values[0].trim().is_empty() { bail!("ClickHouse 对象定义不可见"); }
    Ok(())
}

#[cfg(test)]
mod definition_parser_tests {
    use super::validate_export_definition;
    fn payload(names: serde_json::Value, types: serde_json::Value, rows: &[serde_json::Value]) -> Vec<u8> {
        let mut lines = vec![names.to_string(), types.to_string()];
        lines.extend(rows.iter().map(ToString::to_string));
        lines.join("\n").into_bytes()
    }
    #[test]
    fn definition_preserves_unicode_and_escaped_sql_exactly() {
        let ddl = "CREATE TABLE `中文` (note String DEFAULT 'a\n💾') ENGINE=Memory";
        let bytes = payload(serde_json::json!(["statement"]), serde_json::json!(["String"]), &[serde_json::json!([ddl])]);
        validate_export_definition(&bytes).unwrap();
        assert_eq!(super::parse_json_compact(&bytes).unwrap().rows[0][0].as_deref(), Some(ddl));
    }
    #[test]
    fn definition_rejects_lossy_utf8_non_strings_and_extra_rows() {
        let names = serde_json::json!(["statement"]);
        let types = serde_json::json!(["String"]);
        for rows in [vec![], vec![serde_json::json!([null])], vec![serde_json::json!([42])], vec![serde_json::json!([""])], vec![serde_json::json!(["DDL", "extra"])], vec![serde_json::json!(["DDL"]), serde_json::json!(["DDL"])]] {
            assert!(validate_export_definition(&payload(names.clone(), types.clone(), &rows)).is_err());
        }
        for (bad_names, bad_types) in [(serde_json::json!([42]), types.clone()), (names.clone(), serde_json::json!(["Int32"])), (serde_json::json!([""]), types.clone())] {
            assert!(validate_export_definition(&payload(bad_names, bad_types, &[serde_json::json!(["DDL"])] )).is_err());
        }
        let mut bytes = payload(names, types, &[serde_json::json!(["DDL"])]);
        bytes.push(255);
        assert!(format!("{:#}", validate_export_definition(&bytes).unwrap_err()).contains("UTF-8"));
    }
}

pub fn parse_json_compact(bytes: &[u8]) -> Result<ChResult> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let columns: Vec<String> = match lines.next() {
        Some(line) => {
            let arr: Vec<Value> = serde_json::from_str(line)
                .context("ClickHouse 响应解析失败（列名行）")?;
            arr.into_iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        }
        None => return Ok(ChResult { columns: Vec::new(), rows: Vec::new() }),
    };
    if let Some(line) = lines.next() {
        // 第二行是类型行，校验为 JSON 数组后丢弃
        serde_json::from_str::<Vec<Value>>(line).context("ClickHouse 响应解析失败（类型行）")?;
    }
    let mut rows = Vec::new();
    for line in lines {
        let arr: Vec<Value> =
            serde_json::from_str(line).context("ClickHouse 响应解析失败（数据行）")?;
        rows.push(arr.into_iter().map(json_cell).collect());
    }
    Ok(ChResult { columns, rows })
}

/// JSON 标量 → 字符串（与 sqlx 文本化行为保持一致）。
fn json_cell(v: Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s),
        Value::Bool(b) => Some(if b { "1".into() } else { "0".into() }),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}
