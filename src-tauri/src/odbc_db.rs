//! ODBC 通用驱动：达梦 / Hive / Oracle / DB2 等通过系统 ODBC 驱动真实连接。
//!
//! 需要用户机器上安装对应厂商的 ODBC 驱动。默认连接串由 catalog.rs 的
//! driver 字段生成；字段命名不一致的数据源（如达梦的 SERVER=）单独给出模板；
//! 连接参数 params 提供完整 ODBC 连接串时优先使用（驱动名不一致的逃生通道）。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use odbc_api::{ColumnDescription, Connection, Cursor, DataType, Environment, ResultSetMetadata};

use crate::models::{ColumnInfo, ConnectionRequest, DbKind};

static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();

pub(crate) fn environment() -> Result<&'static Environment> {
    ENV.get_or_init(|| Environment::new().map_err(|e| e.to_string()))
        .as_ref().map_err(|e| anyhow!("初始化 ODBC 环境失败：{e}"))
}

/// 按数据库类型构造默认 ODBC 连接串。
pub fn default_connection_string(kind: &DbKind, req: &ConnectionRequest) -> String {
    let port = req.port;
    let file = req.file.trim();
    let quote = crate::driver_manager::escape_value;
    let host = quote(&req.host);
    let user = quote(&req.user);
    let password = quote(&req.password);
    match kind.id() {
        "dameng" => format!(
            "DRIVER={{DM8 ODBC DRIVER}};SERVER={};PORT={};UID={};PWD={}",
            host, port, user, password
        ),
        "hive" | "inceptor" | "argodb" => format!(
            "DRIVER={{Hive ODBC Driver}};HOST={};PORT={};UID={};PWD={};AuthMech=3",
            host, port, user, password
        ),
        "access" => format!(
            "DRIVER={{Microsoft Access Driver (*.mdb, *.accdb)}};DBQ={}",
            quote(if file.is_empty() { req.database.trim() } else { file })
        ),
        "duckdb" => format!(
            "DRIVER={{DuckDB Driver}};Database={}",
            quote(if file.is_empty() {
                if req.database.trim().is_empty() { ":memory:" } else { req.database.trim() }
            } else {
                file
            })
        ),
        "oracle" => {
            let dsn = if req.database.trim().is_empty() {
                format!("{}:{}", req.host, port)
            } else {
                format!("{}:{}/{}", req.host, port, req.database.trim())
            };
            format!(
                "DRIVER={{Oracle ODBC Driver}};DBQ={};UID={};PWD={}",
                quote(&dsn), user, password
            )
        }
        _ => {
            // 通用模板：使用目录中的驱动名；无驱动名时按 DSN 连接
            let driver = kind.meta().map(|m| m.driver).unwrap_or("");
            if !driver.is_empty() {
                format!(
                    "DRIVER={};SERVER={};PORT={};DATABASE={};UID={};PWD={}",
                    quote(driver), host, port, quote(&req.database), user, password
                )
            } else {
                format!("DSN={};UID={};PWD={}", quote(&req.database), user, password)
            }
        }
    }
}

pub struct OdbcDb {
    conn: Arc<Mutex<Connection<'static>>>,
    pub kind: DbKind,
    pub database: String,
}

/// ODBC 操作是阻塞的，统一丢到 blocking 线程执行。
macro_rules! blocking {
    ($conn:expr, $body:expr) => {{
        let conn = $conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().map_err(|_| anyhow!("ODBC 连接锁不可用"))?;
            ($body)(&guard)
        })
        .await
        .map_err(|e| anyhow!("ODBC 任务失败: {e}"))?
    }};
}

/// 查询结果：列名 + 数据行（全部文本化，与 sqlx 行为一致）。
pub struct OdbcResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// SQLDescribeCol 的真实类型；不能在同步源结构中把 BLOB 降成未知文本。
    pub column_info: Vec<ColumnInfo>,
}

impl OdbcDb {
    pub fn connect(kind: DbKind, req: &ConnectionRequest) -> Result<Self> {
        let conn_str = if let Some(custom) = crate::driver_manager::resolve_connection(req)? {
            custom
        } else if req.params.trim().contains('=') {
            let mut s = req.params.trim().to_string();
            if !req.password.is_empty() && !s.split(';').any(|part| part.split_once('=')
                .is_some_and(|(key, _)| ["pwd", "password"].contains(&key.trim().to_ascii_lowercase().as_str()))) {
                s.push_str(&format!(";PWD={}", crate::driver_manager::escape_value(&req.password)));
            }
            s
        } else {
            default_connection_string(&kind, req)
        };
        Self::connect_string(kind, req, &conn_str)
    }

    /// Connect through the OS ODBC driver manager, including an absolute native library path.
    pub(crate) fn connect_string(kind: DbKind, req: &ConnectionRequest, conn_str: &str) -> Result<Self> {
        #[cfg(target_os = "windows")]
        let (conn_str, _temporary_dsn) = crate::driver_manager::prepare_file_dsn(conn_str)?;
        // Some native file drivers (including DuckDB) reject SQL_ATTR_LOGIN_TIMEOUT before
        // SQLDriverConnect with 08003. Let those drivers apply their own connection timeout.
        #[cfg(target_os = "windows")]
        let login_timeout = if _temporary_dsn.is_some() { None } else { Some(req.timeout_secs.clamp(1, 120) as u32) };
        #[cfg(not(target_os = "windows"))]
        let login_timeout = Some(req.timeout_secs.clamp(1, 120) as u32);
        let options = odbc_api::ConnectionOptions {
            login_timeout_sec: login_timeout,
            packet_size: None,
        };
        let conn = environment()?
            .connect_with_connection_string(&conn_str, options)
            .map_err(|e| anyhow!("ODBC 连接失败：{}", crate::driver_manager::redact_error(&e.to_string(), &req.password)))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            kind,
            database: req.database.clone(),
        })
    }

    /// 执行查询，返回列名 + 文本化数据行。
    pub async fn query_rows(&self, sql: &str) -> Result<OdbcResult> {
        let sql = sql.to_string();
        blocking!(self.conn, |conn: &Connection<'static>| query_blocking(conn, &sql))
    }

    /// 执行语句，返回影响行数（部分驱动不提供精确行数，返回 0）。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let sql = sql.to_string();
        blocking!(self.conn, |conn: &Connection<'static>| exec_blocking(conn, &sql))
    }

    /// 查询单值。
    pub async fn scalar(&self, sql: &str) -> Result<Option<String>> {
        let res = self.query_rows(sql).await?;
        Ok(res.rows.first().and_then(|r| r.first().cloned()).flatten())
    }
}

fn described_column(name: String, description: &ColumnDescription) -> ColumnInfo {
    let data_type = match description.data_type {
        DataType::Unknown | DataType::Other { .. } => String::new(),
        DataType::Binary { .. } => "BINARY".into(),
        DataType::Varbinary { .. } => "VARBINARY".into(),
        DataType::LongVarbinary { .. } => "BLOB".into(),
        DataType::Char { .. } | DataType::WChar { .. } => "CHAR".into(),
        DataType::Varchar { .. } | DataType::WVarchar { .. } => "VARCHAR".into(),
        DataType::LongVarchar { .. } | DataType::WLongVarchar { .. } => "TEXT".into(),
        DataType::Numeric { precision, scale } => format!("NUMERIC({precision},{scale})"),
        DataType::Decimal { precision, scale } => format!("DECIMAL({precision},{scale})"),
        DataType::Integer => "INTEGER".into(),
        DataType::SmallInt => "SMALLINT".into(),
        DataType::BigInt => "BIGINT".into(),
        DataType::TinyInt => "TINYINT".into(),
        DataType::Float { .. } => "FLOAT".into(),
        DataType::Real => "REAL".into(),
        DataType::Double => "DOUBLE PRECISION".into(),
        DataType::Bit => "BIT".into(),
        DataType::Date => "DATE".into(),
        DataType::Time { .. } => "TIME".into(),
        DataType::Timestamp { .. } => "TIMESTAMP".into(),
    };
    let length = match description.data_type {
        DataType::Binary { length } | DataType::Varbinary { length } | DataType::LongVarbinary { length }
        | DataType::Char { length } | DataType::WChar { length } | DataType::Varchar { length }
        | DataType::WVarchar { length } | DataType::LongVarchar { length } | DataType::WLongVarchar { length } => length.and_then(|n| u32::try_from(n.get()).ok()),
        _ => None,
    };
    let precision = match description.data_type {
        DataType::Numeric { precision, .. } | DataType::Decimal { precision, .. } | DataType::Float { precision } => u32::try_from(precision).ok(),
        _ => None,
    };
    let scale = match description.data_type {
        DataType::Numeric { scale, .. } | DataType::Decimal { scale, .. } => u32::try_from(scale).ok(),
        _ => None,
    };
    ColumnInfo { name, data_type, nullable: description.could_be_nullable(), default: None,
        primary_key: false, auto_increment: false, comment: None, length, precision, scale }
}

fn query_blocking(conn: &Connection<'static>, sql: &str) -> Result<OdbcResult> {
    let mut cursor = conn
        .execute(sql, (), Some(30))
        .map_err(|e| anyhow!("ODBC 查询失败: {e}"))?
        .ok_or_else(|| anyhow!("该语句未返回结果集"))?;
    let ncols = cursor.num_result_cols()? as usize;
    let columns: Vec<String> = cursor
        .column_names()?
        .map(|r| r.map(|s| s.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    // 依赖 SQLDescribeCol 的 SQL 类型码；部分驱动的 type-name 会把 BLOB 写成 VARCHAR。
    let mut binary_columns = Vec::with_capacity(ncols);
    let mut column_info = Vec::with_capacity(ncols);
    for i in 1..=ncols as u16 {
        let mut description = ColumnDescription::default();
        cursor.describe_col(i, &mut description)?;
        binary_columns.push(matches!(description.data_type,
            DataType::Binary { .. } | DataType::Varbinary { .. } | DataType::LongVarbinary { .. }));
        column_info.push(described_column(columns[(i - 1) as usize].clone(), &description));
    }
    let mut rows = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    while let Some(mut row) = cursor.next_row()? {
        let mut vals = Vec::with_capacity(ncols);
        for i in 1..=ncols as u16 {
            // 两种读取都以 Ok(false) 表示 NULL；空 BLOB 必须保留为非 NULL 的 0x。
            let is_binary = binary_columns[(i - 1) as usize];
            let has_value = if is_binary { row.get_binary(i, &mut buf)? } else { row.get_text(i, &mut buf)? };
            if has_value {
                vals.push(Some(
                    if is_binary {
                        format!("0x{}", buf.iter().map(|byte| format!("{:02X}", byte)).collect::<String>())
                    } else { String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string() },
                ));
            } else {
                vals.push(None);
            }
            buf.clear();
        }
        rows.push(vals);
    }
    Ok(OdbcResult { columns, rows, column_info })
}

#[cfg(test)]
mod description_tests {
    use super::*;
    use odbc_api::Nullability;
    #[test]
    fn binary_source_metadata_is_not_unknown_text() {
        let description = ColumnDescription::new("PAYLOAD", DataType::Varbinary { length: std::num::NonZeroUsize::new(8000) }, Nullability::Nullable);
        let column = described_column("PAYLOAD".into(), &description);
        assert_eq!(column.data_type, "VARBINARY");
        assert_eq!(column.length, Some(8000));
        assert!(column.nullable && !column.primary_key);
        assert_eq!(crate::sync::map_type(&column.data_type, &DbKind::of("sqlite")), "BLOB");
    }
    #[test]
    fn numeric_description_retains_precision_scale_and_nullable_status() {
        let description = ColumnDescription::new("AMOUNT", DataType::Decimal { precision: 38, scale: 20 }, Nullability::NoNulls);
        let column = described_column("AMOUNT".into(), &description);
        assert_eq!(column.data_type, "DECIMAL(38,20)");
        assert_eq!((column.precision, column.scale, column.nullable), (Some(38), Some(20), false));
    }
}

fn exec_blocking(conn: &Connection<'static>, sql: &str) -> Result<u64> {
    match conn.execute(sql, (), Some(30)) {
        Ok(_) => Ok(0),
        Err(e) => Err(anyhow!("ODBC 执行失败: {e}")),
    }
}

/// 连接超时（供连接串选项引用）。
#[allow(dead_code)]
fn login_timeout() -> Duration {
    Duration::from_secs(15)
}
