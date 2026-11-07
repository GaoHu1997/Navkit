//! ClickHouse 原生 HTTP 接口驱动（默认 8123 端口）。
//!
//! SELECT / SHOW / DESCRIBE 以 `FORMAT JSONCompactEachRowWithNamesAndTypes` 执行，
//! 前两行为列名与列类型，其后为数据行；其余语句按普通 SQL 执行。

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::models::ConnectionRequest;

#[derive(Clone)]
pub struct ClickHouseClient {
    http: reqwest::Client,
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
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(if req.timeout_secs < 5 {
                30
            } else {
                req.timeout_secs
            }))
            .build()
            .context("初始化 HTTP 客户端失败")?;
        Ok(Self {
            http,
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

    /// 执行返回结果集的查询（自动追加 JSON 格式声明）。
    pub async fn query_rows(&self, sql: &str) -> Result<ChResult> {
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
    }

    /// 执行不返回结果集的语句（DDL / INSERT / ALTER），返回影响行数（尽力而为）。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
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
}

/// 解析 JSONCompactEachRowWithNamesAndTypes：第 1 行列名、第 2 行类型、其余数据。
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
