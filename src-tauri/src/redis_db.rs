//! Redis 原生客户端：redis-rs（RESP 协议），支持按库（SELECT n）缓存连接。
//!
//! 树结构：连接 → db0..dbN（依据 INFO keyspace）→ 键；
//! 「表数据」页展示单个键的类型 / TTL / 值；查询标签页可直接执行 Redis 命令。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;

use crate::models::{ConnectionRequest, DbKind};

pub struct RedisDb {
    client: redis::Client,
    timeout: Duration,
    conns: Mutex<HashMap<i64, MultiplexedConnection>>,
    /// 连接配置里指定的默认库（未指定为 0）。
    pub database: i64,
    pub kind: DbKind,
}

impl RedisDb {
    pub async fn connect(kind: DbKind, req: &ConnectionRequest) -> Result<Self> {
        let info = connection_info(req)?;
        let default_db = info.redis.db;
        let timeout = login_timeout(req.timeout_secs);
        // 直接传认证字段，避免 URL 分隔符改变用户名/密码，也不把 secret 写入错误。
        let client = redis::Client::open(info).context("Redis 地址无效")?;
        let config = redis::AsyncConnectionConfig::new()
            .set_connection_timeout(timeout)
            .set_response_timeout(timeout);
        let mut conn = client
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .context("无法连接到 Redis")?;
        tokio::time::timeout(timeout, redis::cmd("PING").query_async::<String>(&mut conn))
            .await
            .map_err(|_| anyhow!("Redis PING 超时"))?
            .context("Redis PING 失败")?;

        let mut conns = HashMap::new();
        conns.insert(default_db, conn);
        Ok(Self {
            client,
            timeout,
            conns: Mutex::new(conns),
            database: default_db,
            kind,
        })
    }

    /// 取指定库的多路复用连接（懒建，避免 SELECT 污染共享连接）。
    async fn conn(&self, db: i64) -> Result<MultiplexedConnection> {
        {
            let guard = self.conns.lock().map_err(|_| anyhow!("Redis 锁不可用"))?;
            if let Some(c) = guard.get(&db) {
                return Ok(c.clone());
            }
        }
        if db < 0 {
            return Err(anyhow!("Redis 数据库编号不能为负数"));
        }
        let config = redis::AsyncConnectionConfig::new()
            .set_connection_timeout(self.timeout)
            .set_response_timeout(self.timeout);
        // Client 自带默认库；新连接必须选择请求的库，包括 db0。
        let mut c = self
            .client
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .with_context(|| format!("无法连接到 Redis（db{}）", db))?;
        redis::cmd("SELECT")
            .arg(db)
            .query_async::<String>(&mut c)
            .await
            .with_context(|| format!("Redis SELECT {} 失败", db))?;
        self.conns
            .lock()
            .map_err(|_| anyhow!("Redis 锁不可用"))?
            .insert(db, c.clone());
        Ok(c)
    }

    /// 列出存在键空间的库（db0..dbN），空库至少返回 db0。
    pub async fn databases(&self) -> Result<Vec<(String, String)>> {
        let mut conn = self.conn(self.database).await?;
        let info: String = redis::cmd("INFO")
            .arg("keyspace")
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow!("Redis INFO 失败: {e}"))?;
        let mut dbs: Vec<(i64, u64)> = Vec::new();
        for line in info.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("db") {
                if let Some((num, keys)) = rest.split_once(':') {
                    if let Ok(n) = num.parse::<i64>() {
                        let keys = keys
                            .split(',')
                            .find_map(|p| p.strip_prefix("keys="))
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(0);
                        dbs.push((n, keys));
                    }
                }
            }
        }
        dbs.sort_unstable();
        let mut out: Vec<(String, String)> = dbs
            .into_iter()
            .map(|(n, keys)| (format!("db{}", n), format!("db{}（{} 键）", n, keys)))
            .collect();
        if out.is_empty() {
            out.push(("db0".into(), "db0".into()));
        }
        Ok(out)
    }

    /// SCAN 浏览某库的键（最多 limit 个）。
    pub async fn keys(&self, db: i64, pattern: &str, limit: usize) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut conn = self.conn(db).await?;
        let mut cursor: u64 = 0;
        let mut out = Vec::new();
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(200)
                .query_async(&mut conn)
                .await
                .map_err(|e| anyhow!("Redis SCAN 失败: {e}"))?;
            for k in batch {
                out.push(k);
                if out.len() >= limit {
                    out.sort();
                    return Ok(out);
                }
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        out.sort();
        Ok(out)
    }

    /// 键详情：[类型, TTL秒, 值预览]。
    pub async fn key_info(&self, db: i64, key: &str) -> Result<(String, i64, String)> {
        let mut conn = self.conn(db).await?;
        let ty: String = redis::cmd("TYPE")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow!("Redis TYPE 失败: {e}"))?;
        let ttl: i64 = conn.ttl(key).await.context("Redis TTL 失败")?;
        let value = self.value_preview(&mut conn, key, &ty).await?;
        Ok((ty, ttl, value))
    }

    async fn value_preview(
        &self,
        conn: &mut MultiplexedConnection,
        key: &str,
        ty: &str,
    ) -> Result<String> {
        let text = match ty {
            "string" => {
                let v: Vec<u8> = redis::cmd("GETRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(MAX_TEXT_BYTES)
                    .query_async(conn)
                    .await
                    .context("Redis GETRANGE 失败")?;
                preview_text(&v)
            }
            "list" => {
                let v: Vec<Vec<u8>> = redis::cmd("LRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(MAX_PREVIEW_ITEMS - 1)
                    .query_async(conn)
                    .await
                    .context("Redis LRANGE 失败")?;
                v.iter()
                    .take(MAX_PREVIEW_ITEMS)
                    .map(|b| preview_text(b))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "set" | "hash" => scan_preview(conn, key, ty, MAX_PREVIEW_ITEMS).await?,
            "zset" => {
                let v: Vec<(Vec<u8>, f64)> = redis::cmd("ZRANGE")
                    .arg(key)
                    .arg(0)
                    .arg(MAX_PREVIEW_ITEMS - 1)
                    .arg("WITHSCORES")
                    .query_async(conn)
                    .await
                    .context("Redis ZRANGE 失败")?;
                v.iter()
                    .take(MAX_PREVIEW_ITEMS)
                    .map(|(m, s)| {
                        format!("{}（{}）", preview_text(m), crate::values::format_float(*s))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            "stream" => {
                // streams feature 未启用，直接解码真实 RESP 嵌套数组，不新增依赖。
                let v: redis::Value = redis::cmd("XRANGE")
                    .arg(key)
                    .arg("-")
                    .arg("+")
                    .arg("COUNT")
                    .arg(MAX_PREVIEW_ITEMS)
                    .query_async(conn)
                    .await
                    .context("Redis XRANGE 失败")?;
                stream_preview(&v, MAX_PREVIEW_ITEMS)?
            }
            "none" => String::new(),
            _ => format!("（{} 类型，请在查询标签页执行命令查看）", ty),
        };
        Ok(text)
    }

    /// 执行一条 Redis 命令（首词为命令名，其余为参数；支持引号包裹的参数）。
    pub async fn run_command(&self, db: i64, input: &str) -> Result<Vec<Vec<Option<String>>>> {
        if input.is_empty() {
            return Err(anyhow!("请输入 Redis 命令，例如 GET key 或 KEYS *"));
        }
        let args = split_command(input)?;
        if args.is_empty() || args[0].is_empty() {
            return Err(anyhow!("空的 Redis 命令"));
        }
        let name = std::str::from_utf8(&args[0]).context("Redis 命令名必须是 UTF-8")?;
        let mut cmd = redis::cmd(name);
        for a in &args[1..] {
            cmd.arg(a.as_slice());
        }
        let mut conn = self.conn(db).await?;
        let val: redis::Value = cmd
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow!("Redis 命令失败: {e}"))?;
        Ok(value_to_table(val))
    }

    /// 删除键。
    #[allow(dead_code)]
    pub async fn del(&self, db: i64, key: &str) -> Result<u64> {
        let mut conn = self.conn(db).await?;
        conn.del(key)
            .await
            .map_err(|e| anyhow!("Redis DEL 失败: {e}"))
    }

    /// 服务器版本（INFO server）。
    pub async fn version(&self) -> Result<String> {
        let mut conn = self.conn(self.database).await?;
        let info: String = redis::cmd("INFO")
            .arg("server")
            .query_async(&mut conn)
            .await
            .map_err(|e| anyhow!("Redis INFO 失败: {e}"))?;
        for line in info.lines() {
            if let Some(v) = line.trim().strip_prefix("redis_version:") {
                return Ok(format!("Redis {}", v.trim()));
            }
        }
        Ok("Redis".into())
    }
}

const MAX_PREVIEW_ITEMS: usize = 200;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_SCAN_ROUNDS: usize = 20;

fn preview_text(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_TEXT_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = MAX_TEXT_BYTES;
    // 前缀仅因截断产生不完整 UTF-8 时退到完整字符；二进制仍按文本 API lossy 展示。
    if let Err(error) = std::str::from_utf8(&bytes[..end]) {
        if error.error_len().is_none() {
            end = error.valid_up_to();
        }
    }
    format!("{}…", String::from_utf8_lossy(&bytes[..end]))
}

async fn scan_preview(
    conn: &mut MultiplexedConnection,
    key: &str,
    ty: &str,
    max: usize,
) -> Result<String> {
    let mut cursor = 0_u64;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    if max == 0 {
        return Ok(String::new());
    }
    // COUNT 是提示而非硬上限（compact encoding 可一次返回整个集合）。
    // 同时限制输出数量和往返次数，并去除 SCAN 允许出现的重复成员/字段。
    for _ in 0..MAX_SCAN_ROUNDS {
        let mut command = redis::cmd(if ty == "set" { "SSCAN" } else { "HSCAN" });
        command.arg(key).arg(cursor).arg("COUNT").arg(max);
        if ty == "set" {
            let (next, batch): (u64, Vec<Vec<u8>>) = command
                .query_async(conn)
                .await
                .context("Redis SSCAN 失败")?;
            cursor = next;
            for member in batch {
                if seen.insert(member.clone()) {
                    out.push(preview_text(&member));
                    if out.len() == max {
                        break;
                    }
                }
            }
        } else {
            let (next, batch): (u64, Vec<(Vec<u8>, Vec<u8>)>) = command
                .query_async(conn)
                .await
                .context("Redis HSCAN 失败")?;
            cursor = next;
            for (field, value) in batch {
                if seen.insert(field.clone()) {
                    out.push(format!(
                        "{}: {}",
                        preview_text(&field),
                        preview_text(&value)
                    ));
                    if out.len() == max {
                        break;
                    }
                }
            }
        }
        if cursor == 0 || out.len() == max {
            break;
        }
    }
    Ok(out.join("\n"))
}

fn stream_preview(reply: &redis::Value, max: usize) -> Result<String> {
    let redis::Value::Array(entries) = reply else {
        return Err(anyhow!("Redis XRANGE 返回值不是数组"));
    };
    let mut lines = Vec::new();
    for entry in entries.iter().take(max) {
        let redis::Value::Array(parts) = entry else {
            return Err(anyhow!("Redis XRANGE 条目不是数组"));
        };
        if parts.len() != 2 {
            return Err(anyhow!("Redis XRANGE 条目必须包含 ID 与字段"));
        }
        let id: String = redis::from_redis_value(&parts[0]).context("Redis XRANGE ID 无效")?;
        let pairs: Vec<(&redis::Value, &redis::Value)> = match &parts[1] {
            redis::Value::Array(fields) if fields.len() % 2 == 0 => fields
                .chunks_exact(2)
                .map(|pair| (&pair[0], &pair[1]))
                .collect(),
            redis::Value::Map(fields) => fields.iter().map(|(f, v)| (f, v)).collect(),
            _ => return Err(anyhow!("Redis XRANGE 字段必须是成对数组或 map")),
        };
        let mut fields = Vec::new();
        for (field, value) in pairs.into_iter().take(MAX_PREVIEW_ITEMS) {
            let field: Vec<u8> =
                redis::from_redis_value(field).context("Redis XRANGE 字段名无效")?;
            let value: Vec<u8> =
                redis::from_redis_value(value).context("Redis XRANGE 字段值无效")?;
            fields.push(format!("{}={}", preview_text(&field), preview_text(&value)));
        }
        lines.push(format!("{} {}", id, fields.join(" ")));
    }
    Ok(lines.join("\n"))
}

fn command_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// redis-cli / sdssplitargs 语义；不按分号拆命令，不把二进制参数转回 String。
fn split_command(input: &str) -> Result<Vec<Vec<u8>>> {
    let input = input.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < input.len() {
        while pos < input.len() && command_space(input[pos]) {
            pos += 1;
        }
        if pos == input.len() {
            break;
        }
        let mut arg = Vec::new();
        let mut quote = None;
        loop {
            if pos == input.len() {
                if quote.is_some() {
                    return Err(anyhow!("Redis 命令引号未闭合"));
                }
                break;
            }
            let byte = input[pos];
            match quote {
                Some(b'"') if byte == b'\\' => {
                    let escaped = *input
                        .get(pos + 1)
                        .ok_or_else(|| anyhow!("Redis 命令引号未闭合"))?;
                    if escaped == b'x' && pos + 3 < input.len() {
                        if let (Some(hi), Some(lo)) =
                            (hex_digit(input[pos + 2]), hex_digit(input[pos + 3]))
                        {
                            arg.push(hi * 16 + lo);
                            pos += 4;
                            continue;
                        }
                    }
                    // Redis 对未知转义保留被转义字符（例如 \\q → q）。
                    arg.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'b' => 0x08,
                        b'a' => 0x07,
                        other => other,
                    });
                    pos += 2;
                }
                Some(b'\'') if byte == b'\\' && input.get(pos + 1) == Some(&b'\'') => {
                    arg.push(b'\'');
                    pos += 2;
                }
                Some(q) if byte == q => {
                    pos += 1;
                    if pos < input.len() && !command_space(input[pos]) {
                        return Err(anyhow!("Redis 命令引号后必须是空白或结尾"));
                    }
                    break;
                }
                Some(_) => {
                    arg.push(byte);
                    pos += 1;
                }
                None if byte == b'\'' || byte == b'"' => {
                    quote = Some(byte);
                    pos += 1;
                }
                None if command_space(byte) => break,
                None => {
                    arg.push(byte);
                    pos += 1;
                }
            }
        }
        out.push(arg); // 即使引号内为空也必须保留该参数。
    }
    Ok(out)
}

/// Redis 返回值 → 列名 + 数据行（与通用结果集形态一致）。
fn value_to_table(v: redis::Value) -> Vec<Vec<Option<String>>> {
    fn cell(v: &redis::Value) -> Option<String> {
        match v {
            redis::Value::Nil => None,
            redis::Value::Int(i) => Some(i.to_string()),
            redis::Value::BulkString(b) => Some(crate::native_db::text_bytes(b)),
            redis::Value::SimpleString(s) => Some(s.clone()),
            redis::Value::Okay => Some("OK".into()),
            redis::Value::Boolean(b) => Some(b.to_string()),
            redis::Value::Double(f) => Some(crate::values::format_float(*f)),
            redis::Value::BigNumber(n) => Some(n.to_string()),
            single => Some(format!("{:?}", single)),
        }
    }
    fn cell_flat(v: &redis::Value) -> Option<String> {
        match v {
            redis::Value::Array(items) => Some(
                items
                    .iter()
                    .map(|x| cell(x).unwrap_or_else(|| "NULL".into()))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            other => cell(other),
        }
    }
    match &v {
        redis::Value::Array(items) => items.iter().map(|x| vec![cell(x)]).collect(),
        redis::Value::Map(pairs) => pairs
            .iter()
            .map(|(k, val)| vec![cell_flat(k), cell_flat(val)])
            .collect(),
        redis::Value::Set(items) => items.iter().map(|x| vec![cell(x)]).collect(),
        single => vec![vec![cell(single)]],
    }
}

fn login_timeout(seconds: u64) -> Duration {
    Duration::from_secs(seconds.clamp(1, 300))
}

fn connection_params(input: &str) -> Result<Vec<(String, String)>> {
    let text = input.trim();
    if text.starts_with('{') || text.starts_with('[') {
        let value: serde_json::Value = serde_json::from_str(text)
            .context("Redis JSON 连接参数格式无效")?;
        let object = value.as_object().ok_or_else(|| anyhow!("Redis JSON 连接参数必须是对象"))?;
        Ok(object.iter().map(|(name, value)| (
            name.trim().to_string(),
            value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()),
        )).collect())
    } else {
        Ok(crate::db::extra_params(text))
    }
}

fn tls_requested(params: &[(String, String)]) -> Result<bool> {
    let mut enabled = None;
    for (name, value) in params {
        let key = name.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let parsed = match key.as_str() {
            "ssl" | "tls" => match value.as_str() {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => return Err(anyhow!("Redis {name} 必须是 true/false/1/0")),
            },
            "sslmode" | "ssl-mode" => match value.as_str() {
                "disable" | "disabled" => false,
                "require" | "required" | "verify-ca" | "verify-full" | "prefer" | "preferred" | "allow" => true,
                _ => return Err(anyhow!("Redis {name} 是无效 TLS 模式")),
            },
            _ => continue,
        };
        if enabled.is_some_and(|previous| previous != parsed) {
            return Err(anyhow!("Redis TLS 参数互相冲突"));
        }
        enabled = Some(parsed);
    }
    Ok(enabled.unwrap_or(false))
}

fn connection_info(req: &ConnectionRequest) -> Result<redis::ConnectionInfo> {
    let params = connection_params(&req.params)?;
    if tls_requested(&params)? {
        // 当前 Cargo 只启用 tokio-comp，无 redis TLS feature；绝不能降级到明文。
        return Err(anyhow!(
            "当前 Redis 客户端未启用 TLS 支持，无法使用 ssl/tls=true"
        ));
    }
    let database = params
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("db"))
        .map(|(_, value)| value.trim())
        .unwrap_or_else(|| req.database.trim());
    let database = database.strip_prefix("db").unwrap_or(database);
    let db = if database.is_empty() {
        0
    } else {
        database.parse::<i64>().context("Redis 数据库编号无效")?
    };
    if db < 0 {
        return Err(anyhow!("Redis 数据库编号不能为负数"));
    }
    let host = req.host.trim();
    if host.is_empty() {
        return Err(anyhow!("Redis 主机不能为空"));
    }
    // bracketed IPv6 的方括号属于 URL 语法，不属于 TCP hostname。
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Ok(redis::ConnectionInfo {
        addr: redis::ConnectionAddr::Tcp(
            host.to_string(),
            if req.port == 0 { 6379 } else { req.port },
        ),
        redis: redis::RedisConnectionInfo {
            db,
            username: (!req.user.is_empty()).then(|| req.user.clone()),
            // ACL username 即使密码为空也要 AUTH username ""。
            password: (!req.password.is_empty() || !req.user.is_empty())
                .then(|| req.password.clone()),
            ..Default::default()
        },
    })
}

#[cfg(test)]
#[path = "redis_db_regression_tests.rs"]
mod regression_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ConnectionRequest {
        ConnectionRequest {
            kind: DbKind::of("redis"),
            host: "127.0.0.1".into(),
            port: 6379,
            user: String::new(),
            password: String::new(),
            database: String::new(),
            file: String::new(),
            params: String::new(),
            timeout_secs: 2,
        }
    }

    fn bytes(text: &str) -> redis::Value {
        redis::Value::BulkString(text.as_bytes().to_vec())
    }

    fn stream_entry(id: &str, fields: redis::Value) -> redis::Value {
        redis::Value::Array(vec![bytes(id), fields])
    }

    #[test]
    fn cli_json_stringify_unicode_sentinel_roundtrip() {
        let expected = "navkit-UUID-中文;O'Reilly\\尾";
        let json = serde_json::to_string(expected).unwrap();
        let args = split_command(&format!("SET sentinel {json}")).unwrap();
        assert_eq!(
            args,
            vec![
                b"SET".to_vec(),
                b"sentinel".to_vec(),
                expected.as_bytes().to_vec()
            ]
        );
    }

    #[test]
    fn cli_double_quote_escapes_and_binary() {
        let args = split_command(r#"ECHO "\n\r\t\b\a\\\"\x00\xff\xAF""#).unwrap();
        assert_eq!(
            args[1],
            vec![b'\n', b'\r', b'\t', 8, 7, b'\\', b'"', 0, 255, 175]
        );
        for byte in 0..=255_u8 {
            let args = split_command(&format!("ECHO \"\\x{byte:02X}\"")).unwrap();
            assert_eq!(args[1], vec![byte]);
        }
    }

    #[test]
    fn cli_single_quote_only_escapes_apostrophe() {
        let args = split_command(r"SET key '中文;O\'Reilly\尾\n\xFF'").unwrap();
        assert_eq!(args[2], "中文;O'Reilly\\尾\\n\\xFF".as_bytes());
    }

    #[test]
    fn cli_retains_empty_quoted_arguments() {
        let args = split_command("SET \"\" ''").unwrap();
        assert_eq!(args, vec![b"SET".to_vec(), vec![], vec![]]);
        assert_eq!(split_command("\"\"").unwrap(), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn cli_semicolon_is_not_a_command_separator() {
        let args = split_command("SET key 中文;尾;DEL").unwrap();
        assert_eq!(args[2], "中文;尾;DEL".as_bytes());
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn cli_ascii_space_not_unicode_or_bare_backslash_escape() {
        let args = split_command(" \tSET\r\nkey\x0b\x0c中文\u{3000}尾\\n ").unwrap();
        assert_eq!(args[2], "中文\u{3000}尾\\n".as_bytes());
        assert!(split_command(" \t\r\n\x0b\x0c").unwrap().is_empty());
        assert_eq!(
            split_command("ECHO \u{3000}").unwrap()[1],
            "\u{3000}".as_bytes()
        );
    }

    #[test]
    fn cli_unknown_double_escape_matches_redis() {
        // sdssplitargs: 无效 hex 不走二进制分支，转义 x 本身；不替换成 JSON 解析器。
        let args = split_command(r#"ECHO "\q\xG1\x1""#).unwrap();
        assert_eq!(args[1], b"qxG1x1");
    }

    #[test]
    fn cli_quote_can_follow_unquoted_prefix() {
        assert_eq!(
            split_command(r#"ECHO pre"中文""#).unwrap()[1],
            "pre中文".as_bytes()
        );
    }

    #[test]
    fn cli_rejects_unclosed_quotes_and_illegal_suffixes() {
        for input in [
            "SET k \"tail",
            "SET k 'tail",
            "SET k \"tail\\",
            "SET k \"a\"b",
            "SET k 'a'b",
            "SET k \"\";DEL k",
            "SET k \"a\"'b'",
        ] {
            assert!(split_command(input).is_err(), "accepted {input:?}");
        }
    }

    #[tokio::test]
    async fn malformed_commands_fail_before_connecting() {
        let db = RedisDb {
            client: redis::Client::open("redis://127.0.0.1:1").unwrap(),
            timeout: Duration::from_secs(1),
            conns: Mutex::new(HashMap::new()),
            database: 0,
            kind: DbKind::of("redis"),
        };
        for (input, expected) in [
            ("SET k \"unterminated", "引号未闭合"),
            ("SET k \"a\"suffix", "引号后"),
            ("\"\"", "空的"),
            (r#""\xFF" k"#, "命令名必须是 UTF-8"),
        ] {
            let error = db.run_command(0, input).await.unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        assert!(db.conns.lock().unwrap().is_empty());
    }

    #[test]
    fn connection_auth_fields_preserve_url_metacharacters() {
        let mut req = request();
        req.user = "用户@:/#".into();
        req.password = "密码@:/#\\尾".into();
        req.database = "db3".into();
        let info = connection_info(&req).unwrap();
        assert_eq!(info.redis.username.as_deref(), Some(req.user.as_str()));
        assert_eq!(info.redis.password.as_deref(), Some(req.password.as_str()));
        assert_eq!(info.redis.db, 3);
        assert_eq!(
            info.addr,
            redis::ConnectionAddr::Tcp("127.0.0.1".into(), 6379)
        );
        let client = redis::Client::open(info).unwrap();
        assert_eq!(
            client.get_connection_info().redis.password.as_deref(),
            Some(req.password.as_str())
        );
    }

    #[test]
    fn connection_password_only_and_empty_acl_password() {
        let mut req = request();
        req.password = "@:/#".into();
        let info = connection_info(&req).unwrap();
        assert_eq!(info.redis.username, None);
        assert_eq!(info.redis.password.as_deref(), Some("@:/#"));
        req.password.clear();
        req.user = "acl-user".into();
        assert_eq!(
            connection_info(&req).unwrap().redis.password.as_deref(),
            Some("")
        );
        req.user.clear();
        assert_eq!(connection_info(&req).unwrap().redis.password, None);
    }

    #[test]
    fn connection_db_parameter_ipv6_and_invalid_db() {
        let mut req = request();
        req.host = "[::1]".into();
        req.port = 0;
        req.database = "db4".into();
        req.params = "DB=2".into();
        let info = connection_info(&req).unwrap();
        assert_eq!(info.redis.db, 2);
        assert_eq!(info.addr, redis::ConnectionAddr::Tcp("::1".into(), 6379));
        for params in ["db=-1", "db=invalid"] {
            req.params = params.into();
            assert!(connection_info(&req).is_err());
        }
    }

    #[test]
    fn tls_strict_case_insensitive_boolean_and_conflict() {
        for value in ["true", "TRUE", "TrUe", "1"] {
            assert!(tls_requested(&[("TLS".into(), value.into())]).unwrap());
        }
        for value in ["false", "FALSE", "FaLsE", "0"] {
            assert!(!tls_requested(&[("SsL".into(), value.into())]).unwrap());
        }
        for value in ["", "yes", "2", "truthy"] {
            assert!(tls_requested(&[("tls".into(), value.into())]).is_err());
        }
        assert!(tls_requested(&[
            ("ssl".into(), "false".into()),
            ("tls".into(), "true".into())
        ])
        .is_err());
    }

    #[test]
    fn unsupported_tls_and_invalid_option_fail_without_secret() {
        let mut req = request();
        req.password = "secret-that-must-not-be-printed".into();
        for params in ["SSL=TRUE", "tls=1", "ssl=bad", "ssl=0;tls=bad"] {
            req.params = params.into();
            let error = connection_info(&req).unwrap_err().to_string();
            assert!(!error.contains(&req.password));
        }
        req.params = "tls=0".into();
        assert!(connection_info(&req).is_ok());
    }

    #[test]
    fn timeout_clamps_to_one_through_three_hundred_seconds() {
        for (input, expected) in [
            (0, 1),
            (1, 1),
            (15, 15),
            (300, 300),
            (301, 300),
            (u64::MAX, 300),
        ] {
            assert_eq!(login_timeout(input), Duration::from_secs(expected));
        }
    }

    #[tokio::test]
    async fn connection_timeout_is_really_applied() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut req = request();
        req.port = listener.local_addr().unwrap().port();
        req.timeout_secs = 1;
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_secs(4),
            RedisDb::connect(DbKind::of("redis"), &req),
        )
        .await;
        server.abort();
        assert!(result.is_ok(), "连接未应用 1 秒 timeout");
        assert!(result.unwrap().is_err(), "未响应的服务不应连接成功");
    }

    #[test]
    fn stream_preview_real_nested_ids_fields_and_binary() {
        let reply = redis::Value::Array(vec![
            stream_entry(
                "1700000000000-0",
                redis::Value::Array(vec![
                    bytes("名"),
                    bytes("中文;O'Reilly\\尾"),
                    bytes("bin"),
                    redis::Value::BulkString(vec![255]),
                ]),
            ),
            stream_entry(
                "1700000000000-1",
                redis::Value::Map(vec![(bytes("f"), bytes("v"))]),
            ),
        ]);
        assert_eq!(
            stream_preview(&reply, 2).unwrap(),
            "1700000000000-0 名=中文;O'Reilly\\尾 bin=�\n1700000000000-1 f=v"
        );
        assert_eq!(stream_preview(&reply, 1).unwrap().lines().count(), 1);
        assert_eq!(stream_preview(&reply, 0).unwrap(), "");
    }

    #[test]
    fn stream_preview_rejects_invalid_shapes_instead_of_empty_success() {
        for reply in [
            redis::Value::Nil,
            redis::Value::Array(vec![bytes("not-entry")]),
            redis::Value::Array(vec![redis::Value::Array(vec![bytes("id")])]),
            redis::Value::Array(vec![stream_entry("1-0", bytes("not-fields"))]),
            redis::Value::Array(vec![stream_entry(
                "1-0",
                redis::Value::Array(vec![bytes("unpaired")]),
            )]),
        ] {
            assert!(stream_preview(&reply, 200).is_err());
        }
        assert_eq!(
            stream_preview(&redis::Value::Array(vec![]), 200).unwrap(),
            ""
        );
    }

    #[test]
    fn preview_text_is_bounded_and_unicode_safe() {
        let short = "中文;O'Reilly\\尾";
        assert_eq!(preview_text(short.as_bytes()), short);
        let long = "中".repeat(MAX_TEXT_BYTES);
        let preview = preview_text(long.as_bytes());
        assert!(preview.ends_with('…'));
        assert!(!preview.contains('�'));
        assert!(preview.len() <= MAX_TEXT_BYTES + '…'.len_utf8());
    }

    #[tokio::test]
    #[ignore = "requires real Redis on 127.0.0.1:6379; parent runs provisioned matrix"]
    async fn real_redis_unicode_binary_all_types_bounds_and_errors() -> Result<()> {
        let db = RedisDb::connect(DbKind::of("redis"), &request()).await?;
        let key = format!("navkit-unit-{}", uuid::Uuid::new_v4());
        let expected = "navkit-UUID-中文;O'Reilly\\尾";
        db.run_command(
            0,
            &format!("SET {key} {}", serde_json::to_string(expected)?),
        )
        .await?;
        assert_eq!(db.key_info(0, &key).await?.2, expected);
        assert!(db
            .run_command(0, &format!("SET {key} \"bad\"suffix"))
            .await
            .is_err());
        assert_eq!(db.key_info(0, &key).await?.2, expected);
        let mut conn = db.conn(0).await?;
        for wrong_type in ["list", "set", "hash", "zset", "stream"] {
            let error = db
                .value_preview(&mut conn, &key, wrong_type)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("WRONGTYPE"));
        }
        db.run_command(0, &format!(r#"SET {key} "\xFF\x00""#))
            .await?;
        let binary: Vec<u8> = redis::cmd("GET").arg(&key).query_async(&mut conn).await?;
        assert_eq!(binary, vec![255, 0]);
        db.del(0, &key).await?;
        for ty in ["list", "set", "hash", "zset", "stream"] {
            for index in 0..=MAX_PREVIEW_ITEMS {
                let value = if index == 0 {
                    expected.to_string()
                } else {
                    format!("v{index}")
                };
                let mut cmd = redis::cmd(match ty {
                    "list" => "RPUSH",
                    "set" => "SADD",
                    "hash" => "HSET",
                    "zset" => "ZADD",
                    _ => "XADD",
                });
                cmd.arg(&key);
                match ty {
                    "hash" => {
                        cmd.arg(format!("f{index}")).arg(value);
                    }
                    "zset" => {
                        cmd.arg(index).arg(value);
                    }
                    "stream" => {
                        cmd.arg("*").arg("f").arg(value);
                    }
                    _ => {
                        cmd.arg(value);
                    }
                }
                let _: redis::Value = cmd.query_async(&mut conn).await?;
            }
            let (actual, ttl, value) = db.key_info(0, &key).await?;
            assert_eq!(actual, ty);
            assert_eq!(ttl, -1);
            assert!(!value.is_empty());
            assert_eq!(value.lines().count(), MAX_PREVIEW_ITEMS);
            if ty == "list" || ty == "zset" || ty == "stream" {
                assert!(value.contains(expected));
            }
            db.del(0, &key).await?;
        }
        Ok(())
    }
}
