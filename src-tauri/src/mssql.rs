//! SQL Server 原生客户端：tiberius（TDS 协议），无需安装 ODBC 驱动。
//!
//! 默认不加密；要求 TLS 时默认验证证书，只有显式 trust-server-certificate=true
//! 才允许自签名证书。未知或冲突的 TLS 参数必须拒绝，不能降级到明文。

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use futures::TryStreamExt;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use crate::models::{ConnectionRequest, DbKind};
#[path = "transfer_mssql_export.rs"] mod export_adapter;

pub struct MssqlDb {
    /// tiberius 查询接口需要 &mut，而连接以 Arc 共享，用 tokio Mutex 包裹。
    client: tokio::sync::Mutex<Client<tokio_util::compat::Compat<tokio::net::TcpStream>>>,
    pub kind: DbKind,
    pub database: String,
}

/// 查询结果：列名 + 文本化数据行（与 ODBC/ClickHouse 客户端形态一致）。
pub struct MssqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

fn tls_options(input: &str) -> Result<(bool, bool)> {
    let trimmed = input.trim();
    let pairs = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value: serde_json::Value = serde_json::from_str(trimmed).context("SQL Server 参数 JSON 无效")?;
        value.as_object().ok_or_else(||anyhow!("SQL Server 参数必须是 JSON 对象"))?.iter().map(|(k,v)|(k.clone(),v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string()))).collect::<Vec<_>>()
    } else { crate::db::extra_params(input) };
    let mut requested=None;let mut trust=false;let mut verify_required=false;
    for (key,value) in pairs {
        let key=key.trim().to_ascii_lowercase();let value=value.trim().to_ascii_lowercase();
        if matches!(key.as_str(),"ssl"|"tls"|"sslmode"|"ssl-mode"|"encrypt") {
            let mode=match value.as_str(){"disable"|"disabled"|"off"|"false"|"no"|"0"=>false,"require"|"required"|"on"|"true"|"yes"|"1"=>true,"strict"|"verify-ca"|"verify-full"=>{verify_required=true;true},_=>return Err(anyhow!("SQL Server TLS 参数值无效；不会降级到明文"))};
            if requested.is_some_and(|old|old!=mode){return Err(anyhow!("SQL Server TLS 参数冲突；不会降级到明文"));}requested=Some(mode);
        } else if matches!(key.as_str(),"trustservercertificate"|"trust-server-certificate"|"trust_server_certificate") {
            trust=match value.as_str(){"true"|"yes"|"1"=>true,"false"|"no"|"0"=>false,_=>return Err(anyhow!("SQL Server trust-server-certificate 参数必须为布尔值"))};
        }
    }
    if trust&&verify_required{return Err(anyhow!("严格证书验证不能同时 trust-server-certificate=true"));}
    Ok((requested.unwrap_or(false),trust))
}

impl MssqlDb {
    pub async fn connect(kind: DbKind, req: &ConnectionRequest) -> Result<Self> {
        let port = if req.port == 0 { 1433 } else { req.port };
        if req.user.is_empty() {
            return Err(anyhow!(
                "SQL Server 连接需要用户名（Windows 集成认证暂不支持）"
            ));
        }
        let mut cfg = Config::new();
        cfg.host(&req.host);
        cfg.port(port);
        cfg.authentication(AuthMethod::sql_server(&req.user, &req.password));
        if !req.database.is_empty() {
            cfg.database(&req.database);
        }
        let (ssl_required, trust_server_certificate) = tls_options(&req.params)?;
        if trust_server_certificate { cfg.trust_cert(); }
        cfg.encryption(if ssl_required {
            EncryptionLevel::Required
        } else {
            EncryptionLevel::NotSupported
        });

        let timeout = Duration::from_secs(req.timeout_secs.max(1));
        let addr = cfg.get_addr();
        let client = tokio::time::timeout(timeout, async {
            let tcp = tokio::net::TcpStream::connect(&addr)
                .await
                .with_context(|| format!("无法连接到 SQL Server {}", addr))?;
            Client::connect(cfg, tcp.compat_write())
                .await
                .map_err(|e| anyhow!("SQL Server 认证失败: {e}"))
        })
        .await
        .map_err(|_| anyhow!("连接超时（{}s）", timeout.as_secs()))??;

        Ok(Self {
            client: tokio::sync::Mutex::new(client),
            kind,
            database: req.database.clone(),
        })
    }

    /// 执行查询，返回列名 + 文本化数据行。
    pub async fn query_rows(&self, sql: &str) -> Result<MssqlResult> {
        let mut sql_history=crate::sql_history::Guard::new(sql,Vec::new());
        let result: Result<MssqlResult> = async {
        let mut client = self.client.lock().await;
        let mut stream = client
            .simple_query(sql)
            .await
            .map_err(|e| anyhow!("SQL Server 查询失败: {e}"))?;
        // QueryStream 可能 !Unpin，固定后才能 try_next
        let mut stream = std::pin::pin!(stream);
        let mut columns: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        while let Some(item) = stream
            .try_next()
            .await
            .map_err(|e| anyhow!("SQL Server 查询失败: {e}"))?
        {
            use tiberius::QueryItem;
            if let QueryItem::Metadata(metadata) = &item {
                columns = metadata.columns().iter().map(|c|c.name().to_string()).collect();
            }
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
        }
        Ok(MssqlResult { columns, rows })
    
        }.await;
        sql_history.result(&result,|r|r.rows.len() as i64);
        result
    }

    /// 执行语句，返回影响行数。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let mut sql_history=crate::sql_history::Guard::new(sql,Vec::new());
        let result: Result<u64> = async {
        let mut client = self.client.lock().await;
        let result = client
            .execute(sql, &[])
            .await
            .map_err(|e| anyhow!("SQL Server 执行失败: {e}"))?;
        Ok(result.total())
    
        }.await;
        sql_history.result(&result,|n|*n as i64);
        result
    }

    /// 查询单值。
    pub async fn scalar(&self, sql: &str) -> Result<Option<String>> {
        let res = self.query_rows(sql).await?;
        Ok(res.rows.first().and_then(|r| r.first().cloned()).flatten())
    }
}

/// 将任意类型单元格文本化（按常见类型逐个尝试，与 sqlx 行为保持一致）。
fn cell_text(row: &tiberius::Row, idx: usize) -> Option<String> {
    macro_rules! try_type {
        ($t:ty) => {
            if let Ok(v) = row.try_get::<$t, _>(idx) {
                if let Some(x) = v {
                    return Some(x.to_string());
                }
                return None;
            }
        };
    }
    try_type!(&str);
    try_type!(i16);
    try_type!(i32);
    try_type!(i64);
    try_type!(f32);
    try_type!(f64);
    try_type!(bool);
    try_type!(tiberius::numeric::Numeric);
    try_type!(chrono::NaiveDateTime);
    try_type!(uuid::Uuid);
    // 与其他同步读取通道一致：二进制为 0xHEX，不能经 UTF-8 替换而丢失字节。
    if let Ok(v) = row.try_get::<&[u8], _>(idx) {
        return v.map(|bytes| format!("0x{}", bytes.iter().map(|byte| format!("{:02X}", byte)).collect::<String>()));
    }
    None
}
