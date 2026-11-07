# -*- coding: utf-8 -*-
"""第一批修复：Cargo features + mssql/redis/http/mongo 的 API 版本适配。"""
import io

def patch(path, pairs):
    s = io.open(path, encoding='utf-8').read()
    for old, new in pairs:
        assert old in s, path + ' NOT FOUND: ' + old[:90]
        s = s.replace(old, new, 1)
    io.open(path, 'w', encoding='utf-8', newline='\n').write(s)
    print('patched', path)

# ============ Cargo.toml：reqwest json/form/gzip ============
patch('Cargo.toml', [
    ('reqwest = "0.13.3"',
     'reqwest = { version = "0.13.3", features = ["json", "form", "gzip"] }'),
])

# ============ mssql.rs：compat_write + Required + execute API ============
patch('src/mssql.rs', [
    # Client 泛型参数：Compat<TcpStream>
    ('''    /// tiberius 查询接口需要 &mut，而连接以 Arc 共享，用 tokio Mutex 包裹。
    client: tokio::sync::Mutex<Client<tokio_util::net::TcpStreamCompat>>,''',
     '''    /// tiberius 查询接口需要 &mut，而连接以 Arc 共享，用 tokio Mutex 包裹。
    client: tokio::sync::Mutex<Client<tokio_util::compat::Compat<tokio::net::TcpStream>>>,'''),
    # TLS 包装：官方示例使用 compat_write
    ('''use anyhow::{anyhow, Context, Result};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel};''',
     '''use anyhow::{anyhow, Context, Result};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio_util::compat::TokioAsyncWriteCompatExt;'''),
    ('''        cfg.encryption(if ssl_required {
            EncryptionLevel::Require
        } else {
            EncryptionLevel::NotSupported
        });''',
     '''        cfg.encryption(if ssl_required {
            EncryptionLevel::Required
        } else {
            EncryptionLevel::NotSupported
        });'''),
    ('''            // TLS 握手需要 TcpStreamCompat（tiberius rustls 下要求可取出底层流）
            Client::connect(cfg, tokio_util::net::TcpStreamCompat::from(tcp))
                .await
                .map_err(|e| anyhow!("SQL Server 认证失败: {e}"))''',
     '''            Client::connect(cfg, tcp.compat_write())
                .await
                .map_err(|e| anyhow!("SQL Server 认证失败: {e}"))'''),
    # execute：tiberius 0.12 用 execute() + ExecuteResult::total()
    ('''    /// 执行语句，返回影响行数（simple_query 不返回精确行数时为 0）。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        use futures::StreamExt;
        let mut client = self.client.lock().await;
        let mut stream = client
            .simple_query(sql)
            .await
            .map_err(|e| anyhow!("SQL Server 执行失败: {e}"))?;
        let mut affected = 0u64;
        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("SQL Server 执行失败: {e}"))?
        {
            use tiberius::QueryItem;
            if let QueryItem::Done(done) = item {
                affected += done.rows_affected() as u64;
            }
        }
        Ok(affected)
    }''',
     '''    /// 执行语句，返回影响行数。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let mut client = self.client.lock().await;
        let result = client
            .execute(sql, &[])
            .await
            .map_err(|e| anyhow!("SQL Server 执行失败: {e}"))?;
        Ok(result.total())
    }'''),
    # QueryItem::Row + Metadata（0.12 无 Done 变体）
    ('''        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("SQL Server 查询失败: {e}"))?
        {
            use tiberius::QueryItem;
            match item {
                QueryItem::Row(row) => {
                    if columns.is_empty() {
                        columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    let n = row.columns().len();
                    let mut vals = Vec::with_capacity(n);
                    for i in 0..n {
                        vals.push(cell_text(&row, i));
                    }
                    rows.push(vals);
                }
                QueryItem::Done(_) => {}
            }
        }''',
     '''        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("SQL Server 查询失败: {e}"))?
        {
            use tiberius::QueryItem;
            if let QueryItem::Row(row) = item {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                let n = row.columns().len();
                let mut vals = Vec::with_capacity(n);
                for i in 0..n {
                    vals.push(cell_text(&row, i));
                }
                rows.push(vals);
            }
        }'''),
    # numeric 路径
    ('    try_type!(tiberius::numeric::Decimal);\n    try_type!(tiberius::numeric::Numeric);',
     '    try_type!(tiberius::tds::numeric::Decimal);\n    try_type!(tiberius::tds::numeric::Numeric);'),
])

# ============ redis_db.rs：Value 新变体 + 按库连接（SELECT） ============
patch('src/redis_db.rs', [
    # Value → 文本（redis 0.27 变体重命名）
    ('''fn value_to_table(v: redis::Value) -> Vec<Vec<Option<String>>> {
    fn cell(v: &redis::Value) -> Option<String> {
        match v {
            redis::Value::Nil => None,
            redis::Value::Int(i) => Some(i.to_string()),
            redis::Value::Data(b) => Some(String::from_utf8_lossy(b).to_string()),
            redis::Value::Status(s) => Some(s.clone()),
            redis::Value::Ok => Some("OK".into()),
            redis::Value::Bulk(items) => items.iter().map(|x| vec![cell(x)]).collect(),
            redis::Value::Map(pairs) => pairs
                .iter()
                .map(|(k, val)| vec![cell(k), cell(val)])
                .collect(),
            redis::Value::Set(items) => items.iter().map(|x| vec![cell(x)]).collect(),
            single => vec![vec![cell(single)]],
        }
    }
    match &v {
        redis::Value::Bulk(items) => items.iter().map(|x| vec![cell(x)]).collect(),
        redis::Value::Map(pairs) => pairs
            .iter()
            .map(|(k, val)| vec![cell(k), cell(val)])
            .collect(),
        redis::Value::Set(items) => items.iter().map(|x| vec![cell(x)]).collect(),
        single => vec![vec![cell(single)]],
    }
}''',
'''fn value_to_table(v: redis::Value) -> Vec<Vec<Option<String>>> {
    fn cell(v: &redis::Value) -> Option<String> {
        match v {
            redis::Value::Nil => None,
            redis::Value::Int(i) => Some(i.to_string()),
            redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).to_string()),
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
            redis::Value::Array(items) => Some(items
                .iter()
                .map(|x| cell(x).unwrap_or_else(|| "NULL".into()))
                .collect::<Vec<_>>()
                .join(", ")),
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
}'''),
    # 按库连接：0.27 无 with_db，连接后执行一次 SELECT
    ('''        let c = self
            .client
            .get_multiplexed_async_connection_with_db(db)
            .await
            .with_context(|| format!("Redis SELECT {} 失败", db))?;
        self.conns''',
     '''        let mut c = self
            .client
            .get_multiplexed_async_connection()
            .await
            .with_context(|| format!("无法连接到 Redis（db{}）", db))?;
        if db != 0 {
            redis::cmd("SELECT")
                .arg(db)
                .query_async::<String>(&mut c)
                .await
                .with_context(|| format!("Redis SELECT {} 失败", db))?;
        }
        self.conns'''),
])

# ============ http_db.rs：Ctx 初始化 + mut 绑定 ============
patch('src/http_db.rs', [
    ('''        let ctx = Ctx {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .danger_accept_invalid_certs(!verify)
                .build()?,
            base,
            user: req.user.clone(),
            pass: req.password.clone(),
            params,
        };''',
     '''        let ctx = Ctx {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .danger_accept_invalid_certs(!verify)
                .build()?,
            base,
            user: req.user.clone(),
            pass: req.password.clone(),
            params,
            headers: reqwest::header::HeaderMap::new(),
        };'''),
    ('''fn with_header_auth(ctx: Ctx) -> Ctx {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut headers = HeaderMap::new();''',
     '''fn with_header_auth(mut ctx: Ctx) -> Ctx {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut headers = HeaderMap::new();'''),
    ('''fn with_bearer(ctx: Ctx) -> Ctx {
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
    let mut headers = HeaderMap::new();''',
     '''fn with_bearer(mut ctx: Ctx) -> Ctx {
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
    let mut headers = HeaderMap::new();'''),
    # Milvus code 判断放在 Value 上更稳（非 JSON 响应已是 String）
    ('''        let code = j.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code != 0 {
            bail!("Milvus 查询失败: {}", jbody(&j));
        }
        let items = j.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(obj_cols_rows(&items))''',
     '''        if let Some(code) = j.get("code").and_then(Value::as_i64) {
            if code != 0 {
                bail!("Milvus 查询失败: {}", jbody(&j));
            }
        }
        let items = j.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(obj_cols_rows(&items))'''),
])

# ============ mongo_db.rs：bson 2.15 API ============
patch('src/mongo_db.rs', [
    ('''        Bson::DateTime(d) => d
            .try_to_chrono()
            .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|_| d.to_string()),''',
     '''        Bson::DateTime(d) => d
            .to_chrono()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),'''),
    ('''fn bson_owned_to_text(v: Bson) -> String {
    use mongodb::bson::Bson as B;
    match v {
        B::Document(d) => d.into_relaxed_extjson_string(),
        B::Array(items) => format!(
            "[{}]",
            items
                .into_iter()
                .map(bson_owned_to_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.into_relaxed_extjson_string(),
    }
}''',
     '''fn bson_owned_to_text(v: Bson) -> String {
    use mongodb::bson::Bson as B;
    fn json_text(v: serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        }
    }
    match v {
        B::Array(items) => format!(
            "[{}]",
            items
                .into_iter()
                .map(bson_owned_to_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => json_text(other.into_relaxed_extjson()),
    }
}'''),
])

print('ALL OK')
