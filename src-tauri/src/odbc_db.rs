//! ODBC 通用驱动：达梦 / Hive / Oracle / DB2 等通过系统 ODBC 驱动真实连接。
//!
//! 需要用户机器上安装对应厂商的 ODBC 驱动。默认连接串由 catalog.rs 的
//! driver 字段生成；字段命名不一致的数据源（如达梦的 SERVER=）单独给出模板；
//! 连接参数 params 提供完整 ODBC 连接串时优先使用（驱动名不一致的逃生通道）。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use odbc_api::{ColumnDescription, Connection, Cursor, DataType, Environment, ResultSetMetadata, TablesRow};

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
        let mut sql_history=crate::sql_history::Guard::new(sql,Vec::new());
        let result: Result<OdbcResult> = async {
        let sql = sql.to_string();
        blocking!(self.conn, |conn: &Connection<'static>| query_blocking(conn, &sql))
    
        }.await;
        sql_history.result(&result,|r|r.rows.len() as i64);
        result
    }

    /// 执行语句，返回影响行数（部分驱动不提供精确行数，返回 0）。
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let mut sql_history=crate::sql_history::Guard::new(sql,Vec::new());
        let result: Result<u64> = async {
        let sql = sql.to_string();
        blocking!(self.conn, |conn: &Connection<'static>| exec_blocking(conn, &sql))
    
        }.await;
        sql_history.result(&result,|n|*n as i64);
        result
    }

    /// 查询单值。
    pub async fn scalar(&self, sql: &str) -> Result<Option<String>> {
        let res = self.query_rows(sql).await?;
        Ok(res.rows.first().and_then(|r| r.first().cloned()).flatten())
    }

    /// 在独立 blocking 线程上打开只进游标，逐行文本化后经有界通道交给调用方，
    /// 不把整表缓存进内存。连接锁在遍历期间持有；接收端关闭（取消/写入失败）后
    /// producer 会在下一次 blocking_send 立即退出，不会继续无谓拉取。
    /// 通过 ODBC 目录（SQLTables）判定对象类型，返回 "TABLE" 或 "VIEW"。
    /// 仅用于非 Oracle 的 SQL 格式导出：TABLE 允许生成 INSERT，VIEW 必须拒绝。
    pub(crate) async fn export_object_type(&self, schema: &str, table: &str) -> Result<String> {
        const MAX_CATALOG_ROWS: usize = 200_000;
        let conn = self.conn.clone();
        let schema = schema.to_owned();
        let table = table.to_owned();
        tokio::task::spawn_blocking(move || -> Result<String> {
            let guard = conn.lock().map_err(|_| anyhow!("ODBC 连接锁已中毒"))?;
            // 目录模式参数可能把 _/% 当作通配符，故一律取全量目录再精确过滤，并对扫描行数设上限。
            let mut scanned = 0usize;
            let mut exact: Vec<String> = Vec::new();
            let mut casefold: Vec<String> = Vec::new();
            for row in guard.tables("", "", "", "")? {
                let row: TablesRow = row.map_err(|e| anyhow!("ODBC 目录查询失败: {e}"))?;
                scanned += 1;
                if scanned > MAX_CATALOG_ROWS {
                    bail!("ODBC 目录对象过多，无法可靠判定对象类型");
                }
                let row_schema = row.schema.as_str().map_err(|e| anyhow!("ODBC 目录 schema 非合法 UTF-8: {e}"))?.unwrap_or("");
                let row_table = row.table.as_str().map_err(|e| anyhow!("ODBC 目录表名非合法 UTF-8: {e}"))?.unwrap_or("");
                let row_type = row.table_type.as_str().map_err(|e| anyhow!("ODBC 目录类型非合法 UTF-8: {e}"))?.unwrap_or("");
                if row_table == table && row_schema == schema {
                    exact.push(row_type.to_owned());
                } else if row_table.eq_ignore_ascii_case(&table) && row_schema.eq_ignore_ascii_case(&schema) {
                    casefold.push(row_type.to_owned());
                }
            }
            let types = if exact.is_empty() { casefold } else { exact };
            match types.len() {
                0 => Err(anyhow!(
                    "ODBC 目录中未找到对象 {}{}（可能不存在或当前用户权限不可见）",
                    if schema.is_empty() { String::new() } else { format!("{schema}.") },
                    table
                )),
                1 => normalize_object_type(&types[0]),
                _ => bail!("ODBC 目录中对象 {schema}.{table} 存在多个匹配，无法唯一判定类型"),
            }
        })
        .await
        .map_err(|e| anyhow!("ODBC 目录查询任务失败: {e}"))?
    }

    pub(crate) fn spawn_row_stream(
        &self,
        sql: String,
        names: Vec<String>,
        binary: Vec<bool>,
        max_record_bytes: u64,
        tx: tokio::sync::mpsc::Sender<Vec<Option<String>>>,
    ) -> tokio::task::JoinHandle<Result<u64>> {
        let mut sql_log = crate::sql_history::Guard::new(&sql, Vec::new());
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let result: Result<u64> = (|| {
                if tx.is_closed() { return Ok(0); }
                let conn_lock = conn.lock().map_err(|_| anyhow!("ODBC 连接锁已中毒"))?;
                let mut cursor = conn_lock
                    .execute(&sql, (), Some(60))
                    .map_err(|e| anyhow!("ODBC 流式查询失败: {e}"))?
                    .ok_or_else(|| anyhow!("导出语句未返回结果集"))?;
                let ncols = cursor.num_result_cols()? as usize;
                if ncols != names.len() { bail!("ODBC 导出列数与预处理元数据不一致"); }
                if binary.len() != ncols { bail!("ODBC 导出二进制元数据长度不一致"); }
                let colnames: Vec<String> = cursor.column_names()?.map(|r| r.map(|s| s.to_string())).collect::<std::result::Result<_, _>>()?;
                if colnames.len() != ncols || colnames.iter().zip(&names).any(|(actual, expected)| actual != expected) {
                    bail!("ODBC 导出列元数据发生变化，目标未对齐");
                }
                // 依据 SQLDescribeCol 复核二进制标记：类型未知且按文本读取会丢失二进制数据，直接拒绝。
                for i in 1..=ncols as u16 {
                    let mut description = ColumnDescription::default();
                    cursor.describe_col(i, &mut description).map_err(|e| anyhow!("ODBC 导出读取列类型失败: {e}"))?;
                    let expected = binary[(i - 1) as usize];
                    match odbc_binary_kind(&description) {
                        Some(actual) => {
                            if actual != expected {
                                bail!("ODBC 导出列 {} 的二进制类型与预处理元数据不一致", names[(i - 1) as usize]);
                            }
                        }
                        None => {
                            if !expected {
                                bail!("ODBC 导出列 {} 类型未知（SQLDescribeCol 未给出可用类型），拒绝按文本读取以避免二进制数据丢失", names[(i - 1) as usize]);
                            }
                        }
                    }
                }
                let mut buf: Vec<u8> = Vec::with_capacity(256);
                let mut count = 0u64;
                loop {
                    if tx.is_closed() { return Ok(count); }
                    let Some(mut row) = cursor.next_row()? else { break; };
                    let mut values: Vec<Option<String>> = Vec::with_capacity(ncols);
                    let mut record_bytes = 0u64;
                    for i in 1..=ncols as u16 {
                        if tx.is_closed() { return Ok(count); }
                        let is_binary = binary[(i - 1) as usize];
                        let has_value = if is_binary { row.get_binary(i, &mut buf) } else { row.get_text(i, &mut buf) }?;
                        let value = if has_value {
                            if is_binary { Some(odbc_hex(&buf, max_record_bytes)?) } else { Some(odbc_text(&buf)?) }
                        } else { None };
                        buf.clear();
                        record_bytes = record_bytes
                            .checked_add(value.as_ref().map_or(0, |text| text.len()) as u64)
                            .ok_or_else(|| anyhow!("ODBC 导出单条记录大小溢出"))?;
                        if record_bytes > max_record_bytes { bail!("ODBC 导出单条记录超过上限"); }
                        values.push(value);
                    }
                    if tx.blocking_send(values).is_err() { return Ok(count); }
                    count = count.checked_add(1).ok_or_else(|| anyhow!("ODBC 导出行计数溢出"))?;
                }
                Ok(count)
            })();
            sql_log.result(&result, |n| *n as i64);
            result
        })
    }
}

/// 严格 UTF-8 解码 ODBC 文本列：不做 lossy、不裁剪 NUL，非法字节直接报错。
fn odbc_text(buf: &[u8]) -> Result<String> {
    std::str::from_utf8(buf)
        .map(str::to_owned)
        .map_err(|e| anyhow!("ODBC 文本列不是合法 UTF-8: {e}"))
}

/// 二进制列转 0x 大写十六进制；分配十六进制串前先按上限校验长度，避免超大缓冲放大分配。
fn odbc_hex(buf: &[u8], max_record_bytes: u64) -> Result<String> {
    if buf.len() as u64 > max_record_bytes { bail!("ODBC 导出单条记录超过上限"); }
    let capacity = buf.len().checked_mul(2).and_then(|n| n.checked_add(2))
        .ok_or_else(|| anyhow!("ODBC 二进制列长度溢出"))?;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut hex = String::with_capacity(capacity);
    hex.push_str("0x");
    for byte in buf { hex.push(HEX[(byte >> 4) as usize] as char); hex.push(HEX[(byte & 15) as usize] as char); }
    Ok(hex)
}

/// 依据 SQLDescribeCol 判定列是否二进制；None 表示类型未知（Unknown/Other）。
fn odbc_binary_kind(description: &ColumnDescription) -> Option<bool> {
    match description.data_type {
        DataType::Binary { .. } | DataType::Varbinary { .. } | DataType::LongVarbinary { .. } => Some(true),
        DataType::Unknown | DataType::Other { .. } => None,
        _ => Some(false),
    }
}

/// 将 SQLTables 的 TABLE_TYPE 归一化为 "TABLE"/"VIEW"；其余类型明确报错，绝不臆断为表。
fn normalize_object_type(raw: &str) -> Result<String> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "TABLE" => Ok("TABLE".to_string()),
        "VIEW" => Ok("VIEW".to_string()),
        "" => bail!("ODBC 目录未返回对象类型"),
        other => bail!("不支持的 ODBC 对象类型 {other}（仅 TABLE/VIEW 允许导出 SQL）"),
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

    #[test]
    fn text_decoding_is_strict_and_preserves_nul() {
        assert_eq!(odbc_text(b"abc").unwrap(), "abc");
        // 不裁剪 NUL：内嵌/尾部 NUL 都原样保留
        assert_eq!(odbc_text(b"a\0b").unwrap(), "a\0b");
        assert_eq!(odbc_text(b"trail\0").unwrap(), "trail\0");
        // 非法 UTF-8 直接报错，绝不 lossy 替换
        assert!(odbc_text(&[0xFF, 0xFE]).is_err());
    }

    #[test]
    fn hex_encoding_bounds_and_format() {
        assert_eq!(odbc_hex(&[0xDE, 0xAD], 16).unwrap(), "0xDEAD");
        // 超过单条记录上限时，在分配十六进制串前即拒绝
        assert!(odbc_hex(&[0u8; 5], 4).is_err());
    }

    #[test]
    fn binary_kind_classification() {
        let bin = ColumnDescription::new("P", DataType::Varbinary { length: std::num::NonZeroUsize::new(8000) }, Nullability::Nullable);
        assert_eq!(odbc_binary_kind(&bin), Some(true));
        let text = ColumnDescription::new("T", DataType::Varchar { length: std::num::NonZeroUsize::new(255) }, Nullability::Nullable);
        assert_eq!(odbc_binary_kind(&text), Some(false));
        let unknown = ColumnDescription::new("U", DataType::Unknown, Nullability::Nullable);
        assert_eq!(odbc_binary_kind(&unknown), None);
    }

    #[test]
    fn object_type_normalization() {
        assert_eq!(normalize_object_type("TABLE").unwrap(), "TABLE");
        assert_eq!(normalize_object_type(" view ").unwrap(), "VIEW");
        // 系统表等非 TABLE/VIEW 类型必须明确拒绝，绝不当作可写表
        assert!(normalize_object_type("SYSTEM TABLE").is_err());
        assert!(normalize_object_type("").is_err());
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
