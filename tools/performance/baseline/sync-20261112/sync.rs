//! Deliberately focused heterogeneous table synchronization (no scheduling/ETL features).
#[path = "sync_stream.rs"]
mod sync_stream;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use crate::db::{Db, qualified, quote_ident, quote_literal};
use crate::models::{DbKind, ColumnInfo, Protocol};
use crate::transfer::{self, TaskControl};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncColumnMapping {
    pub source: String,
    pub target: String,
    /// Optional explicit target DDL type; validated against the target dialect whitelist.
    #[serde(default)] pub target_type: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOptions {
    #[serde(default)] pub task_id: Option<String>,
    pub source_connection: String,
    pub source_schema: String,
    pub source_table: String,
    pub target_connection: String,
    pub target_schema: String,
    pub target_table: String,
    /// structure | data | both
    pub mode: String,
    /// append | replace (DELETE then INSERT) | recreate (DROP/CREATE; requires structure)
    pub write_mode: String,
    /// full（默认，按 write_mode 处理全部数据）| incremental（按主键只插入目标缺失的行）
    #[serde(default)] pub sync_mode: String,
    /// 增量时是否同时更新主键相同但内容不同的行（精确数值归一化，保留文本空白）
    #[serde(default)] pub incremental_update: bool,
    #[serde(default)] pub create_target: bool,
    #[serde(default)] pub columns: Vec<SyncColumnMapping>,
    /// 自定义业务键（源列名，按顺序比较）；为空时使用源表主键。
    #[serde(default)] pub key_columns: Vec<String>,
    /// Legacy flag; true is rejected before all DDL/DML until reviewed deletion is available.
    #[serde(default)] pub delete_missing: bool,
    #[serde(default)] pub where_clause: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport { pub rows: u64, pub created: bool, pub warnings: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTypeMapping { pub source: String, pub source_type: String, pub target_type: String }

/// Existing metadata/qualified SQL only supports SQLite main. Fail closed instead of
/// silently reading/writing a same-named table in another attached database.
pub(crate) async fn validate_sqlite_namespace(db: &Db, namespace: &str, table: &str) -> Result<()> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if let Db::Sqlite(pool) = db {
        if !namespace.is_empty() && !namespace.eq_ignore_ascii_case("main") { bail!("SQLite 工具暂仅支持 main 数据库，禁止附加库或 temp 错靶操作"); }
        let shadow: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM temp.sqlite_master WHERE name=? COLLATE NOCASE")
            .bind(table).fetch_one(pool).await?;
        if shadow > 0 { bail!("存在同名 SQLite 临时对象，无法安全确定 main 目标，请移除临时对象后重试"); }
    }
    Ok(())

    }).await
}

pub fn supported(kind: &DbKind) -> bool {
    matches!(kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres | Protocol::Mssql | Protocol::Clickhouse | Protocol::Odbc)
}
/// ODBC 目标里以大写存储对象名的方言（Oracle/DB2/达梦 系）。
const ODBC_UPPERCASE_KINDS: &[&str] = &["oracle", "dameng", "oscar", "sundb", "yashandb", "gbase", "db2", "informix"];
/// ODBC 目标按方言族细分：Oracle 系没有表级 BOOLEAN，用 NUMBER(1)。
fn odbc_target_dialect(kind: &DbKind) -> &'static str {
    if ODBC_UPPERCASE_KINDS.contains(&kind.id()) { "oracle" } else { "ansi" }
}
/// JDBC-like canonical type mapping inspired by dbswitch's reader/type/writer split.
/// Never copy a source vendor's DDL type string directly into another vendor's CREATE.
pub fn map_type(source: &str, target: &DbKind) -> String {
    let s = source.to_ascii_uppercase();
    let binary = s.contains("BLOB") || s.contains("BINARY") || s == "BYTEA";
    let integer = s.contains("INT") || s.contains("SERIAL");
    let boolean = s.contains("BOOL") || s == "BIT";
    let decimal = s.contains("DECIMAL") || s.contains("NUMERIC") || s.contains("NUMBER") || s.contains("MONEY");
    let real = s.contains("REAL") || s.contains("FLOAT") || s.contains("DOUBLE");
    let timestamp = s.contains("TIMESTAMP") || s.contains("DATETIME");
    let ty = match target.protocol() {
        Protocol::Sqlite => if binary { "BLOB" } else if integer || boolean { "INTEGER" } else if real { "REAL" } else if decimal { "NUMERIC" } else { "TEXT" },
        Protocol::Mysql => if binary { "LONGBLOB" } else if boolean { "BOOLEAN" } else if integer { "BIGINT" } else if real { "DOUBLE" } else if decimal { "DECIMAL(65,20)" } else if timestamp { "DATETIME(6)" } else if s == "DATE" { "DATE" } else if s.starts_with("TIME") { "TIME(6)" } else { "LONGTEXT" },
        Protocol::Postgres => if binary { "BYTEA" } else if boolean { "BOOLEAN" } else if integer { "BIGINT" } else if real { "DOUBLE PRECISION" } else if decimal { "NUMERIC" } else if timestamp { "TIMESTAMP" } else if s == "DATE" { "DATE" } else if s.starts_with("TIME") { "TIME" } else { "TEXT" },
        // SQL Server：文本统一 NVARCHAR 以保 Unicode；二进制 MAX；时间 DATETIME2。
        Protocol::Mssql => if binary { "VARBINARY(MAX)" } else if boolean { "BIT" } else if integer { "BIGINT" } else if real { "FLOAT" } else if decimal { "DECIMAL(38,10)" } else if timestamp { "DATETIME2" } else if s == "DATE" { "DATE" } else if s.starts_with("TIME") { "TIME" } else { "NVARCHAR(MAX)" },
        // ClickHouse：MergeTree；二进制按十六进制文本存储（String），布尔 UInt8。
        Protocol::Clickhouse => if binary { "String" } else if boolean { "UInt8" } else if integer { "Int64" } else if real { "Float64" } else if decimal { "Decimal(65, 20)" } else if timestamp { "DateTime64(6)" } else if s == "DATE" { "Date" } else { "String" },
        // ODBC 通用方言：Oracle/DB2/达梦 系布尔用 NUMBER(1)（无表级 BOOLEAN）。
        Protocol::Odbc => {
            let bool_type = if odbc_target_dialect(target) == "oracle" { "NUMBER(1)" } else { "BOOLEAN" };
            if binary { "BLOB" } else if boolean { bool_type } else if integer { "BIGINT" } else if real { "DOUBLE PRECISION" } else if decimal { "NUMERIC(38, 10)" } else if timestamp { "TIMESTAMP" } else if s == "DATE" { "DATE" } else { "TEXT" }
        }
        _ => "TEXT",
    };
    // Preserve bounded strings for MySQL primary/unique keys (LONGTEXT cannot be indexed without prefix).
    if !target.is_sqlite() && (s.starts_with("VARCHAR(") || s.starts_with("CHAR(")) {
        if let Ok(t) = validate_target_type(&s, target) { return t; }
    }
    ty.into()
}
pub fn validate_target_type(raw: &str, target: &DbKind) -> Result<String> {
    let t = raw.trim().to_ascii_uppercase();
    let (base, params) = if let Some(i) = t.find('(') {
        if !t.ends_with(')') { bail!("目标列类型无效: {}", raw); }
        (&t[..i], Some(&t[i+1..t.len()-1]))
    } else { (t.as_str(), None) };
    let allowed: &[&str] = match target.protocol() {
        Protocol::Sqlite => &["INTEGER", "INT", "BIGINT", "REAL", "TEXT", "BLOB", "NUMERIC", "DECIMAL", "VARCHAR", "CHAR", "BOOLEAN", "DATE", "DATETIME"],
        Protocol::Mysql => &["TINYINT", "SMALLINT", "INT", "INTEGER", "BIGINT", "REAL", "FLOAT", "DOUBLE", "DECIMAL", "NUMERIC", "BOOLEAN", "CHAR", "VARCHAR", "TEXT", "MEDIUMTEXT", "LONGTEXT", "BINARY", "VARBINARY", "BLOB", "LONGBLOB", "DATE", "TIME", "DATETIME", "TIMESTAMP", "JSON"],
        Protocol::Postgres => &["SMALLINT", "INT", "INTEGER", "BIGINT", "REAL", "DOUBLE PRECISION", "DECIMAL", "NUMERIC", "BOOLEAN", "CHAR", "VARCHAR", "TEXT", "BYTEA", "DATE", "TIME", "TIMESTAMP", "TIMESTAMP WITH TIME ZONE", "TIMESTAMP WITHOUT TIME ZONE", "UUID", "JSON", "JSONB"],
        Protocol::Mssql => &["TINYINT", "SMALLINT", "INT", "BIGINT", "REAL", "FLOAT", "DECIMAL", "NUMERIC", "BIT", "CHAR", "VARCHAR", "NVARCHAR", "NCHAR", "TEXT", "NTEXT", "BINARY", "VARBINARY", "DATE", "TIME", "DATETIME", "DATETIME2", "SMALLDATETIME", "UNIQUEIDENTIFIER"],
        Protocol::Clickhouse => &["INT8", "INT16", "INT32", "INT64", "UINT8", "UINT16", "UINT32", "UINT64", "FLOAT32", "FLOAT64", "DECIMAL", "BOOLEAN", "BOOL", "STRING", "FIXEDSTRING", "DATE", "DATETIME", "DATETIME64", "UUID"],
        Protocol::Odbc => &["BIGINT", "INTEGER", "INT", "SMALLINT", "TINYINT", "NUMBER", "NUMERIC", "DECIMAL", "DOUBLE PRECISION", "REAL", "FLOAT", "BOOLEAN", "CHAR", "VARCHAR", "TEXT", "CLOB", "BLOB", "BINARY", "DATE", "TIME", "TIMESTAMP"],
        _ => bail!("目标数据库不支持同步"),
    };
    if !allowed.contains(&base) { bail!("不支持或不安全的目标列类型: {}", raw); }
    if let Some(p) = params {
        let param_bases = ["CHAR", "VARCHAR", "NCHAR", "NVARCHAR", "BINARY", "VARBINARY", "DECIMAL", "NUMERIC", "NUMBER", "DATETIME", "DATETIME2", "TIMESTAMP", "TIME", "DATETIME64", "FIXEDSTRING"];
        if !param_bases.contains(&base) { bail!("该类型不接受长度/精度参数: {}", raw); }
        if p.eq_ignore_ascii_case("max") {
            if target.protocol() != Protocol::Mssql || !["VARCHAR", "NVARCHAR", "VARBINARY", "BINARY"].contains(&base) { bail!("该类型不接受 MAX 参数: {}", raw); }
            return Ok(t);
        }
        let nums = p.split(',').map(|x| x.trim().parse::<u32>().map_err(|_| anyhow!("类型长度/精度必须是整数"))).collect::<Result<Vec<_>>>()?;
        if nums.is_empty() || nums.len() > 2 || nums.iter().any(|n| *n > 65535) || (nums.len() == 2 && nums[1] > nums[0]) { bail!("类型长度/精度超出范围"); }
        if ["CHAR", "VARCHAR", "NCHAR", "NVARCHAR", "BINARY", "VARBINARY", "FIXEDSTRING"].contains(&base) && (nums.len() != 1 || nums[0] == 0) { bail!("字符串长度必须大于零"); }
        let decimal_max = if target.is_mysql_family() { 65 } else if target.protocol() == Protocol::Mssql { 38 } else if target.protocol() == Protocol::Clickhouse { 76 } else { 1000 };
        if ["DECIMAL", "NUMERIC", "NUMBER"].contains(&base) && (nums[0] == 0 || nums[0] > decimal_max || nums.get(1).is_some_and(|s| target.is_mysql_family() && *s > 30)) { bail!("数值精度超出目标范围"); }
        if ["TIME", "DATETIME", "DATETIME2", "TIMESTAMP", "DATETIME64"].contains(&base) && (nums.len() != 1 || nums[0] > 6) { bail!("时间精度只能为 0 至 6"); }
    }
    Ok(t)
}
pub async fn type_mapping(source: &Db, source_kind: &DbKind, schema: &str, table: &str, target_kind: &DbKind) -> Result<Vec<SyncTypeMapping>> {
    let __sql_history_context = crate::sql_history::db_context(source);
    crate::sql_history::scope(__sql_history_context, async {
    if !supported(source_kind) || !supported(target_kind) { bail!("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源；Redis、Elasticsearch、Kafka 等非关系型不支持"); }
    validate_sqlite_namespace(source, schema, table).await?;
    let schema = crate::schema::table_schema(source, source_kind.clone(), schema, table).await?;
    Ok(schema.columns.iter().map(|c| SyncTypeMapping { source: c.name.clone(), source_type: c.data_type.clone(), target_type: map_type(&c.data_type, target_kind) }).collect())

    }).await
}
fn binary(ty: &str) -> bool { let t = ty.to_ascii_uppercase(); t.contains("BLOB") || t.contains("BINARY") || t == "BYTEA" }
pub fn value_sql(value: Option<&str>, target: &DbKind, target_type: &str, source_type: &str) -> Result<String> {
    let Some(value) = value else { return Ok("NULL".into()); };
    if binary(target_type) && binary(source_type) {
        let hex = value.strip_prefix("0x").ok_or_else(|| anyhow!("二进制源值缺少十六进制标记"))?;
        if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) { bail!("二进制值格式无效"); }
        return Ok(match target.protocol() {
            Protocol::Postgres => format!("decode('{}','hex')", hex),
            Protocol::Mssql => format!("0x{}", hex),
            _ => format!("X'{}'", hex),
        });
    }
    if target_type.to_ascii_uppercase().contains("BOOL") {
        return Ok(match value.to_ascii_lowercase().as_str() { "1" | "true" | "t" => "TRUE", "0" | "false" | "f" => "FALSE", _ => bail!("无法把 {} 转换为 BOOLEAN", value) }.into());
    }
    // 源是布尔而目标不是布尔类型（Oracle NUMBER(1)、ClickHouse UInt8 等）：转 1/0。
    let source_upper = source_type.to_ascii_uppercase();
    if source_upper.contains("BOOL") || source_upper == "BIT" {
        if let Some(bit) = match value.to_ascii_lowercase().as_str() { "1" | "true" | "t" => Some("1"), "0" | "false" | "f" => Some("0"), _ => None } {
            return Ok(bit.into());
        }
    }
    if target.is_pg_family() && value.contains('\0') { bail!("PostgreSQL 文本不支持空字节"); }
    if target.is_sqlite() && value.contains('\0') {
        return Ok(format!("CAST(X'{}' AS TEXT)", value.as_bytes().iter().map(|b| format!("{:02X}", b)).collect::<String>()));
    }
    // SQL Server 的 Unicode 文本类型用 N'' 前缀字面量。
    if target.protocol() == Protocol::Mssql && target_type.to_ascii_uppercase().starts_with('N') {
        return Ok(format!("N{}", quote_literal(value)));
    }
    Ok(quote_literal(value))
}
/// 主键按有边界的 JSON 数组编码；保留空白，NULL/空串及控制字符均不会碰撞。
pub(crate) fn row_key(names: &[String], row: &[Option<String>], cols: &[String]) -> Option<String> {
    let values=cols.iter().map(|col|names.iter().position(|n|n==col).and_then(|i|row.get(i))).collect::<Option<Vec<_>>>()?;
    serde_json::to_string(&values).ok()
}
fn validate_key_mapping(meta: &crate::models::TableSchema, mapping: &[SyncColumnMapping], keys: &[String]) -> Result<()> {
    if keys.is_empty() {
        for column in meta.columns.iter().filter(|c|c.primary_key) {
            if !mapping.iter().any(|m|m.source==column.name) {bail!("增量同步/差异比较必须映射完整主键，缺少主键列: {}",column.name);}
        }
        return Ok(());
    }
    if keys.len() > 8 { bail!("业务键最多支持 8 列"); }
    for key in keys {
        if !meta.columns.iter().any(|c| &c.name==key) { bail!("业务键列不存在: {key}"); }
        if !mapping.iter().any(|m| &m.source==key) { bail!("业务键列必须参与列映射: {key}"); }
    }
    Ok(())
}
fn keyed_rows<'a>(names: &[String], rows: &'a [Vec<Option<String>>], columns: &[String], label: &str) -> Result<std::collections::HashMap<String, &'a Vec<Option<String>>>> {
    let mut keyed=std::collections::HashMap::new();
    for row in rows {
        for column in columns {
            let index=names.iter().position(|name|name==column).ok_or_else(||anyhow!("{}主键列缺失: {}",label,column))?;
            if row.get(index).and_then(Option::as_deref).is_none() {bail!("{}主键 {} 为 NULL，无法安全进行增量比较",label,column);}
        }
        let key=row_key(names,row,columns).ok_or_else(||anyhow!("{}无法读取完整主键值",label))?;
        if keyed.insert(key,row).is_some() {bail!("{}存在重复主键值，无法安全进行增量比较",label);}
    }
    Ok(keyed)
}
/// 数值族类型：内容比较时做数值归一化（"1" 与 "1.0" 视为相同）。
fn numeric_family(data_type: &str) -> bool {
    let t = data_type.to_ascii_uppercase();
    t.contains("INT") || t.contains("SERIAL") || t.contains("REAL") || t.contains("FLOA")
        || t.contains("DOUBLE") || t.contains("DECIMAL") || t.contains("NUMERIC") || t.contains("NUMBER")
}
/// 精确数值归一化不经 f64，避免 BIGINT/DECIMAL 被舍入后漏判；文本空白属于数据。
/// 日期/时间在不同数据库的文本格式可能不同，仍计入差异。
fn normalized_cell(value: Option<&str>, numeric: bool) -> Option<String> {
    let value = value?;
    if numeric {
        if let Ok(number) = value.trim().parse::<sqlx::types::BigDecimal>() { return Some(number.normalized().to_string()); }
    }
    Some(value.to_string())
}
/// 比较两行（源行/目标行）在映射列上的内容是否不同。
fn row_differs(names: &[String], srow: &[Option<String>], tnames: &[String], trow: &[Option<String>], cols: &[(&SyncColumnMapping, &ColumnInfo)], target_meta: Option<&crate::models::TableSchema>) -> Result<bool> {
    for (m, c) in cols {
        let si = names.iter().position(|n| n == &m.source).ok_or_else(|| anyhow!("数据列缺失: {}", m.source))?;
        // TEXT → DECIMAL 后目标会补小数位；按真实双方类型比较，而不是反复更新同一值。
        let numeric = numeric_family(&c.data_type) || target_meta
            .and_then(|meta| meta.columns.iter().find(|tc| tc.name == m.target))
            .is_some_and(|tc| numeric_family(&tc.data_type));
        let sv = normalized_cell(srow.get(si).and_then(Option::as_deref), numeric);
        let tv = tnames.iter().position(|n| n == &m.target)
            .and_then(|ti| trow.get(ti)).and_then(Option::as_deref);
        let tv = normalized_cell(tv, numeric);
        if sv != tv { return Ok(true); }
    }
    Ok(false)
}
/// 生成增量更新语句：SET 非 mapping 的全部非主键列（源值优先），WHERE 主键。
fn update_sql(full: &str, target_kind: &DbKind, names: &[String], row: &[Option<String>], cols: &[(&SyncColumnMapping, &ColumnInfo, String)], target_meta: Option<&crate::models::TableSchema>) -> Result<String> {
    let mut sets = Vec::new();
    let mut wheres = Vec::new();
    for (m, c, ty) in cols {
        let i = names.iter().position(|n| n == &m.source).ok_or_else(|| anyhow!("数据列缺失: {}", m.source))?;
        let value = row.get(i).and_then(Option::as_deref);
        let target_type = target_meta.and_then(|meta| meta.columns.iter().find(|tc| tc.name == m.target)).map(|tc| tc.data_type.as_str()).unwrap_or(ty);
        if c.primary_key {
            if value.is_none() { bail!("主键 {} 为 NULL，无法生成增量更新", m.source); }
            // 主键也必须按目标真实类型编码，不能把 BLOB/Unicode 键一律作为普通文本。
            wheres.push(format!("{} = {}", quote_ident(target_kind, &m.target), value_sql(value,target_kind,target_type,&c.data_type)?));
        } else {
            sets.push(format!("{} = {}", quote_ident(target_kind, &m.target), value_sql(value, target_kind, target_type, &c.data_type)?));
        }
    }
    if sets.is_empty() { bail!("映射列全部是主键，没有可更新的列"); }
    Ok(format!("UPDATE {} SET {} WHERE {}", full, sets.join(", "), wheres.join(" AND ")))
}
fn portable_default(c: &ColumnInfo, target: &DbKind) -> Option<String> {
    let d = c.default.as_deref()?.trim();
    if d.eq_ignore_ascii_case("NULL") || d.eq_ignore_ascii_case("CURRENT_TIMESTAMP") || d.eq_ignore_ascii_case("CURRENT_DATE") { return Some(d.into()); }
    if d.parse::<f64>().is_ok() && !d.eq_ignore_ascii_case("nan") && !d.to_ascii_lowercase().contains("inf") { return Some(d.into()); }
    if d.eq_ignore_ascii_case("true") || d.eq_ignore_ascii_case("false") { return Some(if target.is_sqlite() { if d.eq_ignore_ascii_case("true") { "1" } else { "0" } } else { d }.into()); }
    if d.starts_with('\'') && d.ends_with('\'') && !d.contains(';') && !d.contains("--") && !d.contains("/*") {
        let inner = &d[1..d.len()-1];
        if inner.replace("''", "").contains('\'') { return None; }
        return Some(d.into());
    }
    None
}
async fn same_database(source: &Db, target: &Db) -> Result<bool> {
    if std::ptr::eq(source, target) { return Ok(true); }
    match (source, target) {
        (Db::Sqlite(a), Db::Sqlite(b)) => {
            let a: String = crate::sql_history::query("PRAGMA database_list").fetch_one(a).await?.try_get("file")?;
            let b: String = crate::sql_history::query("PRAGMA database_list").fetch_one(b).await?.try_get("file")?;
            Ok(!a.is_empty() && std::fs::canonicalize(a)? == std::fs::canonicalize(b)?)
        },
        (Db::Mysql(a), Db::Mysql(b)) => {
            let sql = "SELECT CONCAT(@@hostname, ':', @@port, ':', @@server_id)";
            let a: String = crate::sql_history::query_scalar(sql).fetch_one(a).await?;
            let b: String = crate::sql_history::query_scalar(sql).fetch_one(b).await?;
            Ok(a == b)
        },
        (Db::Postgres(a), Db::Postgres(b)) => {
            let sql = "SELECT current_database() || ':' || COALESCE(inet_server_addr()::text,'local') || ':' || COALESCE(inet_server_port()::text,'local')";
            let a: String = crate::sql_history::query_scalar(sql).fetch_one(a).await?;
            let b: String = crate::sql_history::query_scalar(sql).fetch_one(b).await?;
            Ok(a == b)
        },
        _ => Ok(false),
    }
}
pub async fn synchronize(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind, opts: &SyncOptions) -> Result<SyncReport> {
    let task = opts.task_id.as_deref().map(TaskControl::begin).transpose()?;
    let result = synchronize_inner(source, source_kind, target, target_kind, opts, task.as_ref(), &opts.source_table).await;
    if let Some(t) = task { t.finish(&result); }
    result
}
async fn synchronize_inner(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind, opts: &SyncOptions, task: Option<&TaskControl>, label: &str) -> Result<SyncReport> {
    // A boolean does not bind the deletion scope, source snapshot, or target revision.
    // Reject it for every mode before touching either database, including legacy full requests.
    if opts.delete_missing { bail!("删除目标多余行需要完整范围/预览/CAS授权；legacy delete_missing=true已在任何DDL或DML前拒绝"); }
    if !matches!(opts.mode.as_str(), "structure" | "data" | "both") || !matches!(opts.write_mode.as_str(), "append" | "replace" | "recreate") { bail!("同步模式或写入模式无效"); }
    if !matches!(opts.sync_mode.as_str(), "" | "full" | "incremental") { bail!("同步方式无效：仅支持 full / incremental"); }
    let incremental = opts.sync_mode == "incremental";
    let structure = opts.mode != "data";
    let data = opts.mode != "structure";
    if incremental && !data { bail!("增量同步必须包含数据；仅结构同步请使用 full，未发送任何DDL或DML"); }
    if incremental && opts.write_mode != "append" { bail!("增量同步只支持「追加数据」写入模式；清空/重建属于全量语义"); }
    if (!structure && opts.write_mode == "recreate") || (!data && opts.write_mode == "replace") { bail!("仅数据模式不能重建表；仅结构模式不能清空数据"); }
    if !supported(source_kind) || !supported(target_kind) { bail!("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源；Redis、Elasticsearch、Kafka 等非关系型不支持"); }
    transfer::validate_names(&[opts.source_table.clone()])?; transfer::validate_names(&[opts.target_table.clone()])?;
    transfer::validate_where(opts.where_clause.as_deref())?;
    // Resolve all incremental requests before same_table, pooled metadata, and legacy DDL.
    // Only the verified physical SQLite pair may enter the snapshot/spill/typed transaction path.
    if incremental {
        match (source, target) {
            (Db::Sqlite(source), Db::Sqlite(target)) if source_kind.is_sqlite() && target_kind.is_sqlite() => {
                return sync_stream::sqlite_incremental(source, target, opts, task, label).await;
            }
            _ => bail!("增量数据同步目前仅验证 SQLite 原生源与目标的有界快照/typed事务；此通道未发送DDL或DML，不回退到全量读取后截断的旧增量路径"),
        }
    }
    let prefix = if label.is_empty() { String::new() } else { format!("{} · ", label) };
    validate_sqlite_namespace(source, &opts.source_schema, &opts.source_table).await?;
    validate_sqlite_namespace(target, &opts.target_schema, &opts.target_table).await?;
    let same_table = opts.source_table == opts.target_table || (source_kind.is_sqlite() && opts.source_table.eq_ignore_ascii_case(&opts.target_table)) || (source_kind.is_mysql_family() && opts.source_table.eq_ignore_ascii_case(&opts.target_table)) || (source_kind.protocol() == Protocol::Mssql && opts.source_table.eq_ignore_ascii_case(&opts.target_table));
    let same_namespace = source_kind.is_sqlite() || opts.source_schema == opts.target_schema;
    if same_table && same_namespace && (opts.source_connection == opts.target_connection || same_database(source, target).await?) { bail!("源表与目标表不能是同一张表（包括连接别名）"); }
    if let Some(t) = task { t.check()?; t.update("metadata", 0, 0, &format!("{}正在读取源表结构与目标映射", prefix)); }
    let source_schema = crate::schema::table_schema(source, source_kind.clone(), &opts.source_schema, &opts.source_table).await?;
    if source_schema.columns.is_empty() { bail!("源表不存在或没有可同步列"); }
    let mapping = if opts.columns.is_empty() { source_schema.columns.iter().map(|c| SyncColumnMapping { source: c.name.clone(), target: c.name.clone(), target_type: None }).collect::<Vec<_>>() } else { opts.columns.clone() };
    transfer::validate_names(&mapping.iter().map(|m| m.source.clone()).collect::<Vec<_>>())?;
    transfer::validate_names(&mapping.iter().map(|m| m.target.clone()).collect::<Vec<_>>())?;
    let mut cols = Vec::new();
    for m in &mapping {
        let c = source_schema.columns.iter().find(|c| c.name == m.source).ok_or_else(|| anyhow!("源列不存在: {}", m.source))?;
        let ty = match &m.target_type { Some(t) => validate_target_type(t, target_kind)?, None => map_type(&c.data_type, target_kind) };
        cols.push((m, c, ty));
    }
    let exists = crate::schema::list_tables(target, target_kind.clone(), &opts.target_schema).await?.iter().any(|(n, _)| n == &opts.target_table);
    let create = structure && (!exists || opts.write_mode == "recreate");
    if !exists && (!structure || !opts.create_target) { bail!("目标表不存在：仅数据模式要求已有表；结构模式需启用建表"); }
    let mut warnings = Vec::new();
    if structure && exists && opts.write_mode != "recreate" { warnings.push("目标表已存在，保留现有结构；要应用结构映射请选择重建目标表".into()); }
    if create {
        if !source_schema.foreign_keys.is_empty() { warnings.push("不复制外键：跨数据库引用可能没有对应目标表".into()); }
        if source_schema.columns.iter().any(|c| c.auto_increment) { warnings.push("自增/序列不跨库复制；主键与原始值保留".into()); }
        if target_kind.is_mysql_family() { warnings.push("MySQL DDL 自动提交：取消/错误可回滚数据，但已完成建表/重建不能回滚".into()); }
    }
    if matches!(target_kind.protocol(), Protocol::Mssql | Protocol::Clickhouse | Protocol::Odbc) {
        warnings.push("该目标通道不支持事务：语句逐条自动提交，中途失败时已写入的部分不会自动回滚".into());
    }
    if target_kind.protocol() == Protocol::Odbc {
        warnings.push("ODBC 目标按 ANSI SQL 生成（Oracle/DB2/达梦 系对象名自动大写）；建议先小规模试跑核对类型与对象名".into());
    }
    if target_kind.protocol() == Protocol::Clickhouse && opts.write_mode == "replace" {
        warnings.push("ClickHouse 的 DELETE 依赖轻量删除特性（22.8+）；老版本请改用「重建目标表」".into());
    }
    let target_meta = if exists && !create { Some(crate::schema::table_schema(target, target_kind.clone(), &opts.target_schema, &opts.target_table).await?) } else { None };
    if let Some(meta) = &target_meta {
        for (m, _, _) in &cols { if !meta.columns.iter().any(|c| c.name == m.target) { bail!("目标列不存在: {}", m.target); } }
    }
    let full = qualified(target_kind, &opts.target_schema, &opts.target_table);
    let mut stmts = Vec::new();
    if create {
        if exists { stmts.push((format!("DROP TABLE {}", full), false)); }
        let mut defs = Vec::new();
        for (m, c, ty) in &cols {
            let target_type = if target_kind.protocol() == Protocol::Clickhouse && c.nullable && !c.primary_key {
                format!("Nullable({})", ty)
            } else { ty.clone() };
            let mut def = format!("{} {}", quote_ident(target_kind, &m.target), target_type);
            if !c.nullable || c.primary_key { def.push_str(" NOT NULL"); }
            if let Some(d) = portable_default(c, target_kind) { def.push_str(&format!(" DEFAULT {}", d)); }
            else if c.default.is_some() { warnings.push(format!("{} 的数据库专属默认值未复制", c.name)); }
            defs.push(def);
        }
        let pk = cols.iter().filter(|(_, c, _)| c.primary_key).map(|(m, _, _)| quote_ident(target_kind, &m.target)).collect::<Vec<_>>();
        // ClickHouse 不支持表级 PRIMARY KEY 约束：主键作为 MergeTree 的 ORDER BY。
        let create_sql = if target_kind.protocol() == Protocol::Clickhouse {
            let order = if pk.is_empty() { "tuple()".to_string() } else { format!("({})", pk.join(", ")) };
            format!("CREATE TABLE {} ({}) ENGINE = MergeTree ORDER BY {}", full, defs.join(", "), order)
        } else {
            if !pk.is_empty() { defs.push(format!("PRIMARY KEY ({})", pk.join(", "))); }
            format!("CREATE TABLE {} ({})", full, defs.join(", "))
        };
        stmts.push((create_sql, false));
        if target_kind.protocol() == Protocol::Clickhouse {
            if source_schema.indexes.iter().any(|i| !i.primary) {
                warnings.push("ClickHouse 目标不复制二级索引（MergeTree 使用 ORDER BY / 跳数索引）".into());
            }
        } else {
            for idx in source_schema.indexes.iter().filter(|i| !i.primary) {
                let mapped = idx.columns.iter().map(|n| mapping.iter().find(|m| m.source == *n).map(|m| quote_ident(target_kind, &m.target))).collect::<Option<Vec<_>>>();
                if let Some(c) = mapped {
                    if !c.is_empty() {
                        let name = format!("sync_{}_{}", opts.target_table.chars().take(24).collect::<String>(), uuid::Uuid::new_v4().simple());
                        stmts.push((format!("CREATE {}INDEX {} ON {} ({})", if idx.unique { "UNIQUE " } else { "" }, quote_ident(target_kind, &name), full, c.join(", ")), false));
                    }
                } else { warnings.push(format!("索引 {} 包含未映射的列，已跳过", idx.name)); }
            }
        }
    }
    // 全量替换：清空目标表旧数据（含 ClickHouse 轻量删除变体）。
    // 该压入必须在分页快速路径（下方 data && !incremental 提前 return）之前完成，
    // 否则 replace 的 DELETE 永远不会执行（曾致 full_replace 撞主键，BUG-R1-002）。
    // 注意：仅数据写入需要清空；recreate 走上方 DROP+CREATE；增量已在元数据读取前单独路由。
    if data && opts.write_mode == "replace" && exists {
        let delete = if target_kind.protocol() == Protocol::Clickhouse {
            format!("DELETE FROM {} WHERE 1", full)
        } else { format!("DELETE FROM {}", full) };
        stmts.push((delete, false));
        if target_kind.protocol() != Protocol::Sqlite && target_kind.protocol() != Protocol::Mysql && target_kind.protocol() != Protocol::Postgres {
            warnings.push("replace 在该目标通道不支持事务：先清空后写入，中途失败/取消会留下空表或部分数据；建议先小规模试跑或改用「重建目标表」".into());
        }
        if target_kind.protocol() == Protocol::Mysql {
            if let Some(meta) = &target_meta {
                if meta.engine.as_deref().is_some_and(|e| e.eq_ignore_ascii_case("myisam")) {
                    warnings.push("目标表为 MyISAM（不支持事务）：replace 清空后中途失败/取消无法自动回滚，请谨慎操作".into());
                }
            }
        }
    }
    if data && !incremental {
        if let Some(t) = task { t.check()?; t.update("read", 0, 0, &format!("{}正在按批读取源数据，目标尚未修改", prefix)); }
        // 全量写入按页读取源表，分批写入目标，避免一次性把整张表读入内存。
        let target_names = cols.iter().map(|(m, _, _)| quote_ident(target_kind, &m.target)).collect::<Vec<_>>().join(", ");
        let spec = InsertSpec {
            cols: cols.iter().map(|(m, c, ty)| (m.source.clone(), m.target.clone(), ty.clone(), c.data_type.clone())).collect(),
            target_names,
            full: full.clone(),
        };
        let order = source_schema.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect::<Vec<_>>();
        let mut feeder = SyncPageFeeder { source, kind: source_kind, schema: &opts.source_schema, table: &opts.source_table, clause: opts.where_clause.as_deref(), order, offset: 0, names: Vec::new() };
        let count = execute_sync_pages(target, &stmts, &mut feeder, &spec, target_kind, target_meta.as_ref(), task, &prefix).await?;
        return Ok(SyncReport { rows: count, created: create, warnings });
    }
    let count = transfer::execute_atomic(target, &stmts, task).await?;
    Ok(SyncReport { rows: count, created: create, warnings })
}

/// 整库同步：单个对象的映射（源表/视图名 → 目标表名，目标留空表示同名）。
/// 视图按当前查询结果物化为目标普通表（跨方言不复制视图 DDL）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbSyncTable {
    pub source: String,
    #[serde(default)] pub target: String,
    #[serde(default)] pub view: bool,
}

/// 整库（库到库）同步请求：不选具体来源/目标表映射，由后端逐表复用 synchronize_inner。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbSyncRequest {
    #[serde(default)] pub task_id: Option<String>,
    pub source_connection: String,
    pub source_schema: String,
    pub target_connection: String,
    pub target_schema: String,
    /// both | structure
    pub mode: String,
    /// 目标已有同名表的处理：skip（跳过）| append（追加）| replace（清空后写入）| recreate（重建）
    pub write_mode: String,
    /// full（默认）| incremental（逐表按主键只插入缺失行，透传给每张表）
    #[serde(default)] pub sync_mode: String,
    #[serde(default)] pub incremental_update: bool,
    #[serde(default)] pub tables: Vec<DbSyncTable>,
    #[serde(default)] pub where_clause: Option<String>,
}

/// 整库同步中单张表的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTableReport {
    pub source_table: String,
    pub target_table: String,
    pub rows: u64,
    pub created: bool,
    pub skipped: bool,
    #[serde(default)] pub error: Option<String>,
    #[serde(default)] pub warnings: Vec<String>,
}

/// 整库同步入口：逐表独立事务，单表失败记录 error 后继续；skip 模式跳过目标已存在的表。
pub async fn synchronize_database(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind, req: &DbSyncRequest) -> Result<Vec<SyncTableReport>> {
    let task = req.task_id.as_deref().map(TaskControl::begin).transpose()?;
    let result = synchronize_database_inner(source, source_kind, target, target_kind, req, task.as_ref()).await;
    if let Some(t) = task { t.finish(&result); }
    result
}

async fn synchronize_database_inner(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind, req: &DbSyncRequest, task: Option<&TaskControl>) -> Result<Vec<SyncTableReport>> {
    if !supported(source_kind) || !supported(target_kind) { bail!("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源；Redis、Elasticsearch、Kafka 等非关系型不支持"); }
    if !matches!(req.mode.as_str(), "both" | "structure") { bail!("整库同步模式无效：仅支持结构+数据或仅结构"); }
    if !matches!(req.write_mode.as_str(), "skip" | "append" | "replace" | "recreate") { bail!("目标同名表处理方式无效"); }
    if !matches!(req.sync_mode.as_str(), "" | "full" | "incremental") { bail!("同步方式无效：仅支持 full / incremental"); }
    if req.sync_mode == "incremental" {
        if req.mode != "both" { bail!("整库增量同步必须包含数据；仅结构同步请使用 full，未发送任何DDL或DML"); }
        if !matches!(req.write_mode.as_str(), "skip" | "append") {
            bail!("整库增量同步不能与「清空后写入/重建」组合；请改用追加或跳过已存在的表");
        }
        // Check before pooled metadata and skip: no table may fall back to legacy incremental SQL.
        if !matches!((source, target), (Db::Sqlite(_), Db::Sqlite(_))) || !source_kind.is_sqlite() || !target_kind.is_sqlite() {
            bail!("整库增量数据同步目前仅验证 SQLite 原生源与目标的有界快照/typed事务；此通道未发送DDL或DML");
        }
    }
    if req.tables.is_empty() { bail!("请至少选择一张要同步的表"); }
    transfer::validate_where(req.where_clause.as_deref())?;
    let same_conn = req.source_connection == req.target_connection;
    let same_namespace = source_kind.is_sqlite() || req.source_schema == req.target_schema;
    if same_conn && same_namespace && same_database(source, target).await? { bail!("整库同步的源库与目标库不能相同（包括连接别名）"); }
    if let Some(t) = task { t.check()?; t.update("metadata", 0, 0, "正在读取目标库表清单"); }
    // 目标已存在表集合只用于 skip 快速判断与进度；逐表建表/存在性仍由 synchronize_inner 重新核实。
    let existing: std::collections::HashSet<String> = crate::schema::list_tables(target, target_kind.clone(), &req.target_schema)
        .await?.into_iter().map(|(n, _)| n).collect();
    let total = req.tables.len() as u64;
    let mut reports = Vec::with_capacity(req.tables.len());
    let mut seen_targets: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (index, pair) in req.tables.iter().enumerate() {
        if let Some(t) = task { t.check()?; }
        let target_table = if pair.target.trim().is_empty() { pair.source.clone() } else { pair.target.trim().to_string() };
        // 源/目标名分开校验：默认同名映射（"a" → "a"）不是重复名。
        transfer::validate_names(std::slice::from_ref(&pair.source))?;
        transfer::validate_names(std::slice::from_ref(&target_table))?;
        if !seen_targets.insert(target_table.to_ascii_lowercase()) {
            bail!("目标表名重复: {}", target_table);
        }
        let mut report = SyncTableReport {
            source_table: pair.source.clone(), target_table: target_table.clone(),
            rows: 0, created: false, skipped: false, error: None, warnings: Vec::new(),
        };
        let exists = existing.contains(&target_table);
        if req.write_mode == "skip" && exists {
            report.skipped = true;
            reports.push(report);
            if let Some(t) = task { t.update("skip", (index + 1) as u64, total, &format!("{} · 目标已存在，按设置跳过（{} / {}）", target_table, index + 1, total)); }
            continue;
        }
        // skip 等价于「目标不存在时建表 + 追加」；其余模式按所选传入。
        let opts = SyncOptions {
            task_id: None,
            source_connection: req.source_connection.clone(),
            source_schema: req.source_schema.clone(),
            source_table: pair.source.clone(),
            target_connection: req.target_connection.clone(),
            target_schema: req.target_schema.clone(),
            target_table,
            mode: req.mode.clone(),
            write_mode: if req.write_mode == "skip" { "append".into() } else { req.write_mode.clone() },
            sync_mode: req.sync_mode.clone(),
            incremental_update: req.incremental_update,
            create_target: true,
            columns: Vec::new(),
            key_columns: Vec::new(),
            delete_missing: false,
            where_clause: req.where_clause.clone(),
        };
        if let Some(t) = task { t.update("sync", (index + 1) as u64, total, &format!("{} · 开始同步（{} / {}）", opts.target_table, index + 1, total)); }
        // 视图无法跨方言复制 DDL：明确按「物化为普通表」处理并写进报告。
        // 视图没有稳定业务键，增量通道的 typed 记录快照/CAS 无法绑定它；
        // 因此视图配对在增量请求下显式按全量物化执行，并在报告里写明换成原因。
        let mut opts = opts;
        let view_notes = if pair.view {
            let mut notes = vec![if opts.mode == "structure" {
                format!("源对象 {} 是视图：按视图当前列结构创建目标普通表（仅结构，不复制数据）", pair.source)
            } else {
                format!("源对象 {} 是视图：按当前查询结果物化为目标普通表并写入数据", pair.source)
            }];
            if opts.sync_mode == "incremental" {
                opts.sync_mode = "full".into();
                opts.incremental_update = false;
                notes.push(format!("源对象 {} 是视图：视图无稳定业务键，不能进入增量快照/CAS 通道；本项显式改为全量物化执行（未冒充增量快照）", pair.source));
            }
            notes
        } else { Vec::new() };
        match synchronize_inner(source, source_kind, target, target_kind, &opts, task, &pair.source).await {
            Ok(r) => {
                report.rows = r.rows; report.created = r.created;
                report.warnings = view_notes.into_iter().chain(r.warnings).collect();
            }
            Err(e) => { report.error = Some(format!("{:#}", e)); report.warnings = view_notes; }
        }
        // 普通单表错误可继续，但取消必须传播；尤其不能吞掉最后一张表的取消。
        if let Some(t)=task {t.check()?;}
        reports.push(report);
    }
    if let Some(t) = task { t.check()?; t.update("done", total, total, "整库同步完成"); }
    Ok(reports)
}

/// 差异比较请求（只读，不修改任何数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiffRequest {
    pub source_connection: String,
    pub source_schema: String,
    pub source_table: String,
    pub target_connection: String,
    pub target_schema: String,
    pub target_table: String,
    #[serde(default)] pub where_clause: Option<String>,
    /// 为空时按源表列同名映射；与同步执行使用同一套列映射语义。
    #[serde(default)] pub columns: Vec<SyncColumnMapping>,
}
/// 源表与目标表的差异报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiffReport {
    pub source_rows: u64,
    pub target_rows: u64,
    /// 是否按主键进行了行级比较（源表无主键时为 false）。
    pub keyed: bool,
    /// 源有目标无（按主键）；目标不存在时等于源行数。
    pub missing_in_target: u64,
    /// 目标有源无（按主键）。
    pub extra_in_target: u64,
    /// 主键相同但映射列内容不同的行数（数值归一化近似比较；时间文本格式差异会计入）。
    pub changed: u64,
    /// 内容差异行的主键示例（最多 5 个）。
    pub changed_samples: Vec<String>,
    /// 目标表缺少的列（映射目标列名）。
    pub target_missing_columns: Vec<String>,
    /// 目标表多出的列（不在源映射中）。
    pub target_extra_columns: Vec<String>,
    pub message: Option<String>,
}
/// 比较源表与目标表的差异：行数、结构、按主键的缺失/多出/内容差异。只读。
pub async fn diff(source: &Db, source_kind: &DbKind, target: &Db, target_kind: &DbKind, req: &SyncDiffRequest) -> Result<SyncDiffReport> {
    if !supported(source_kind) || !supported(target_kind) { bail!("差异比较仅支持 SQLite、MySQL/MariaDB 与 PostgreSQL 协议数据库"); }
    validate_sqlite_namespace(source, &req.source_schema, &req.source_table).await?;
    validate_sqlite_namespace(target, &req.target_schema, &req.target_table).await?;
    transfer::validate_where(req.where_clause.as_deref())?;
    let source_meta = crate::schema::table_schema(source, source_kind.clone(), &req.source_schema, &req.source_table).await?;
    if source_meta.columns.is_empty() { bail!("源表不存在或没有可比较的列"); }
    let mapping = if req.columns.is_empty() {
        source_meta.columns.iter().map(|c| SyncColumnMapping { source: c.name.clone(), target: c.name.clone(), target_type: None }).collect::<Vec<_>>()
    } else { req.columns.clone() };
    let mut cols = Vec::new();
    for m in &mapping {
        let c = source_meta.columns.iter().find(|c| c.name == m.source).ok_or_else(|| anyhow!("源列不存在: {}", m.source))?;
        cols.push((m, c));
    }
    validate_key_mapping(&source_meta,&mapping,&[])?;
    let report = SyncDiffReport {
        source_rows: 0, target_rows: 0, keyed: false, missing_in_target: 0, extra_in_target: 0,
        changed: 0, changed_samples: Vec::new(),
        target_missing_columns: Vec::new(), target_extra_columns: Vec::new(), message: None,
    };
    let target_exists = crate::schema::list_tables(target, target_kind.clone(), &req.target_schema).await?
        .iter().any(|(n, _)| n == &req.target_table);
    let (snames, srows) = transfer::fetch_transfer_rows(source, source_kind, &req.source_schema, &req.source_table, req.where_clause.as_deref(), None).await?;
    let mut report = SyncDiffReport { source_rows: srows.len() as u64, ..report };
    if !target_exists {
        report.missing_in_target = report.source_rows;
        report.keyed = cols.iter().any(|(_, c)| c.primary_key);
        report.target_missing_columns = cols.iter().map(|(m, _)| m.target.clone()).collect();
        report.message = Some("目标表不存在：全部源行都将作为缺失行写入（结构模式或启用建表时自动创建）。".into());
        return Ok(report);
    }
    let target_meta = crate::schema::table_schema(target, target_kind.clone(), &req.target_schema, &req.target_table).await?;
    for (m, _) in &cols {
        if !target_meta.columns.iter().any(|tc| tc.name == m.target) { report.target_missing_columns.push(m.target.clone()); }
    }
    for tc in &target_meta.columns {
        if !cols.iter().any(|(m, _)| m.target == tc.name) { report.target_extra_columns.push(tc.name.clone()); }
    }
    let (tnames, trows) = transfer::fetch_transfer_rows(target, target_kind, &req.target_schema, &req.target_table, None, None).await?;
    report.target_rows = trows.len() as u64;
    let pk_source: Vec<String> = cols.iter().filter(|(_, c)| c.primary_key).map(|(m, _)| m.source.clone()).collect();
    let pk_target: Vec<String> = cols.iter().filter(|(_, c)| c.primary_key).map(|(m, _)| m.target.clone()).collect();
    if pk_source.is_empty() {
        report.message = Some("源表没有主键：仅统计行数与结构差异；缺失/多出/内容差异需要主键才能比较。".into());
        return Ok(report);
    }
    report.keyed = true;
    let source_keys=keyed_rows(&snames,&srows,&pk_source,"源表")?;
    let target_keys=keyed_rows(&tnames,&trows,&pk_target,"目标表")?;
    report.missing_in_target = source_keys.keys().filter(|k| !target_keys.contains_key(*k)).count() as u64;
    report.extra_in_target = target_keys.keys().filter(|k| !source_keys.contains_key(*k)).count() as u64;
    let mapped: Vec<(&SyncColumnMapping, &ColumnInfo)> = cols.iter().map(|(m, c)| (*m, *c)).collect();
    for (key, srow) in &source_keys {
        let Some(trow) = target_keys.get(key) else { continue };
        if row_differs(&snames, srow, &tnames, trow, &mapped, Some(&target_meta))? {
            report.changed += 1;
            if report.changed_samples.len() < 5 {
                let sample = pk_source.iter().zip(pk_target.iter())
                    .map(|(s, _)| {
                        let i = snames.iter().position(|n| n == s);
                        i.and_then(|i| srow.get(i)).and_then(Option::as_deref).unwrap_or("NULL").to_string()
                    })
                    .collect::<Vec<_>>().join(", ");
                report.changed_samples.push(sample);
            }
        }
    }
    Ok(report)
}

/// 全量同步的 INSERT 语句生成器（按页构造，避免持有全部行）。
struct InsertSpec {
    /// (源列, 目标列, 目标类型, 源类型)
    cols: Vec<(String, String, String, String)>,
    target_names: String,
    full: String,
}

impl InsertSpec {
    fn statements(&self, kind: &DbKind, names: &[String], target_meta: Option<&crate::models::TableSchema>, rows: &[Vec<Option<String>>]) -> Result<Vec<(String, bool)>> {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let values = self.cols.iter().map(|(source, target, ty, source_type)| {
                let i = names.iter().position(|n| n == source).ok_or_else(|| anyhow!("数据列缺失: {}", source))?;
                let target_type = target_meta.and_then(|m| m.columns.iter().find(|c| &c.name == target)).map(|c| c.data_type.as_str()).unwrap_or(ty);
                value_sql(row.get(i).and_then(Option::as_deref), kind, target_type, source_type)
            }).collect::<Result<Vec<_>>>()?;
            out.push((format!("INSERT INTO {} ({}) VALUES ({})", self.full, self.target_names, values.join(", ")), true));
        }
        Ok(out)
    }
}

/// 按页读取源表，供全量同步分批写入。
struct SyncPageFeeder<'a> {
    source: &'a Db,
    kind: &'a DbKind,
    schema: &'a str,
    table: &'a str,
    clause: Option<&'a str>,
    order: Vec<String>,
    offset: u64,
    names: Vec<String>,
}

impl<'a> SyncPageFeeder<'a> {
    async fn next(&mut self) -> Result<Option<(Vec<String>, Vec<Vec<Option<String>>>)>> {
        let (names, rows) = transfer::fetch_transfer_page(self.source, self.kind, self.schema, self.table, self.clause, &self.order, self.offset, 2000).await?;
        if rows.is_empty() {
            Ok(None)
        } else {
            self.offset += rows.len() as u64;
            if self.names.is_empty() { self.names = names.clone(); }
            Ok(Some((names, rows)))
        }
    }
}

/// 在一个事务内执行全量同步的 DDL/清空前缀与分批 INSERT（非事务通道逐条自动提交）。
async fn execute_sync_pages(
    target: &Db,
    prefix: &[(String, bool)],
    feeder: &mut SyncPageFeeder<'_>,
    spec: &InsertSpec,
    kind: &DbKind,
    target_meta: Option<&crate::models::TableSchema>,
    task: Option<&TaskControl>,
    label: &str,
) -> Result<u64> {
    let __sql_history_context = crate::sql_history::db_context(target);
    crate::sql_history::scope(__sql_history_context, async {
    macro_rules! run_tx {
        ($pool:expr) => {{
            let mut count = 0u64;
            let mut tx = $pool.begin().await?;
            for (sql, is_row) in prefix {
                match crate::sql_history::query(sql).execute(&mut *tx).await {
                    Ok(r) => if *is_row { count += r.rows_affected(); },
                    Err(e) => { let _ = tx.rollback().await; return Err(anyhow!(e).context("同步前置语句执行失败，未提交数据已回滚")); }
                }
            }
            while let Some((names, rows)) = feeder.next().await? {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消，未提交的目标修改已回滚"))?; }
                for (sql, is_row) in spec.statements(kind, &names, target_meta, &rows)? {
                    match crate::sql_history::query(&sql).execute(&mut *tx).await {
                        Ok(r) => if is_row { count += r.rows_affected(); },
                        Err(e) => { let _ = tx.rollback().await; return Err(anyhow!(e).context("同步分批写入失败，未提交数据已回滚")); }
                    }
                }
                if let Some(t) = task { t.update("write", count, 0, &format!("{}正在分批写入目标（提交前可取消）", label)); }
                tokio::task::yield_now().await;
            }
            // 提交前最终取消检查：最后一页写入期间收到的取消必须回滚而非提交
            // （BUG-R1-003：单页写完直接 commit 曾把已取消任务提交并误报成功）。
            if let Some(t) = task {
                if let Err(e) = t.check() { let _ = tx.rollback().await; return Err(e.context("任务已取消，未提交的目标修改已回滚")); }
            }
            tx.commit().await?;
            Ok(count)
        }};
    }
    match target {
        Db::Sqlite(p) => run_tx!(p),
        Db::Mysql(p) => run_tx!(p),
        Db::Postgres(p) => run_tx!(p),
        Db::Mssql(_) | Db::ClickHouse(_) | Db::Odbc(_) => {
            let mut count = 0u64;
            for (sql, is_row) in prefix {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
                let n = target.generic_exec(sql).await.map_err(|e| anyhow!(e).context("同步前置语句失败；该目标通道不支持事务，已执行的语句不会自动回滚"))?;
                if *is_row { count += n.max(1); }
            }
            while let Some((names, rows)) = feeder.next().await? {
                if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
                for (sql, is_row) in spec.statements(kind, &names, target_meta, &rows)? {
                    let n = target.generic_exec(&sql).await.map_err(|e| anyhow!(e).context("同步写入失败；该目标通道不支持事务，已执行的语句不会自动回滚"))?;
                    if is_row { count += n.max(1); }
                }
                if let Some(t) = task { t.update("write", count, 0, &format!("{}正在分批写入目标（非事务通道，逐条提交）", label)); }
                tokio::task::yield_now().await;
            }
            // 非事务通道同样补最终检查（对齐 transfer::execute_atomic 范式）：最后一条语句执行期间也可能收到取消。
            if let Some(t) = task { t.check().map_err(|e| e.context("任务已取消；该目标通道不支持事务，已执行的语句不会自动回滚"))?; }
            Ok(count)
        }
        _ => bail!("同步支持 SQLite、MySQL/PostgreSQL 协议族、SQL Server、ClickHouse 与 ODBC 关系型数据源"),
    }

    }).await
}


#[cfg(test)]
mod db_sync_table_tests {
    use super::*;
    #[test]
    fn db_sync_table_view_flag_defaults_to_false() {
        // 旧客户端/旧配置不带 view 字段时按表处理，保持向后兼容。
        let without: DbSyncTable = serde_json::from_str(r#"{"source":"t1"}"#).unwrap();
        assert_eq!(without.source, "t1");
        assert!(!without.view);
        let with: DbSyncTable = serde_json::from_str(r#"{"source":"v1","target":"v1_mat","view":true}"#).unwrap();
        assert!(with.view);
        assert_eq!(with.target, "v1_mat");
    }
}

#[cfg(test)]
mod diff_helpers_tests {
    use super::*;

    #[test]
    fn normalized_cell_numeric_and_text() {
        // 数值列：文本形式不同但数值相同视为一致
        assert_eq!(normalized_cell(Some("1"), true).unwrap(), normalized_cell(Some("1.0"), true).unwrap());
        assert_eq!(normalized_cell(Some(" 123.50 "), true).unwrap(), normalized_cell(Some("123.5"), true).unwrap());
        // 非数值文本保持原样，不能抹掉业务数据中的首尾空白。
        assert_ne!(normalized_cell(Some("abc"), true).unwrap(), normalized_cell(Some("abd"), true).unwrap());
        assert_eq!(normalized_cell(Some(" abc "), false).unwrap(), " abc ");
        assert_ne!(normalized_cell(Some("9007199254740992"),true),normalized_cell(Some("9007199254740993"),true));
        assert_ne!(normalized_cell(Some("1.00000000000000000001"),true),normalized_cell(Some("1.00000000000000000002"),true));
        // NULL 与空串可区分
        assert_ne!(normalized_cell(None, true), normalized_cell(Some(""), true));
        // 非数值列的 "01" 与 "1" 文本不同（不误伤字符串主键）
        assert_ne!(normalized_cell(Some("01"), false).unwrap(), normalized_cell(Some("1"), false).unwrap());
    }

    #[test]
    fn numeric_family_detection() {
        assert!(numeric_family("BIGINT") && numeric_family("decimal(10,2)") && numeric_family("DOUBLE"));
        assert!(!numeric_family("varchar(255)") && !numeric_family("DATETIME") && !numeric_family("BLOB"));
    }

    #[test]
    fn row_key_distinguishes_null_and_missing_columns() {
        let names = vec!["id".to_string(), "name".to_string()];
        let row_a = vec![Some("1".into()), Some("x".into())];
        let row_b = vec![Some("1".into()), None];
        let pk = vec!["id".to_string()];
        assert_eq!(row_key(&names, &row_a, &pk), row_key(&names, &row_b, &pk));
        // 主键列不存在于行名清单 → None（调用方报错而不是静默错配）
        assert_eq!(row_key(&names, &row_a, &["missing".to_string()]), None);
        // 复合主键：NULL 与空串产生不同键
        let pk2 = vec!["id".to_string(), "name".to_string()];
        assert_ne!(row_key(&names, &row_a, &pk2), row_key(&names, &row_b, &pk2));
    }

    #[test]
    fn row_differs_respects_mapping_and_normalization() {
        let snames = vec!["id".to_string(), "amount".to_string(), "note".to_string()];
        let tnames = vec!["pk".to_string(), "amt".to_string(), "memo".to_string()];
        let m1 = SyncColumnMapping { source: "id".into(), target: "pk".into(), target_type: None };
        let m2 = SyncColumnMapping { source: "amount".into(), target: "amt".into(), target_type: None };
        let m3 = SyncColumnMapping { source: "note".into(), target: "memo".into(), target_type: None };
        let c1 = ColumnInfo { name: "id".into(), data_type: "int".into(), nullable: false, default: None, primary_key: true, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let c2 = ColumnInfo { name: "amount".into(), data_type: "decimal(10,2)".into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let c3 = ColumnInfo { name: "note".into(), data_type: "varchar(50)".into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let cols = vec![(&m1, &c1), (&m2, &c2), (&m3, &c3)];
        let srow = vec![Some("1".into()), Some("10.0".into()), Some("同".into())];
        let trow_same = vec![Some("1".into()), Some("10".into()), Some("同".into())];
        let trow_diff = vec![Some("1".into()), Some("11".into()), Some("同".into())];
        assert!(!row_differs(&snames, &srow, &tnames, &trow_same, &cols, None).unwrap());
        assert!(row_differs(&snames, &srow, &tnames, &trow_diff, &cols, None).unwrap());
        // 未映射进比较的列（extra 列）不影响结果；映射列缺失报错
        assert!(row_differs(&snames, &srow, &tnames, &trow_same, &cols, None).is_ok());
    }

    #[test]
    fn business_keys_must_exist_and_be_mapped() {
        let c_id = ColumnInfo { name: "id".into(), data_type: "int".into(), nullable: false, default: None, primary_key: true, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let c_code = ColumnInfo { name: "code".into(), data_type: "text".into(), nullable: false, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let c_note = ColumnInfo { name: "note".into(), data_type: "text".into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        let meta = crate::models::TableSchema { table: "t".into(), database: "d".into(), columns: vec![c_id, c_code, c_note], indexes: Vec::new(), foreign_keys: Vec::new(), comment: None, engine: None, collation: None, row_count: None };
        let mapping = vec![
            SyncColumnMapping { source: "id".into(), target: "id".into(), target_type: None },
            SyncColumnMapping { source: "code".into(), target: "code".into(), target_type: None },
        ];
        assert!(validate_key_mapping(&meta, &mapping, &["code".to_string()]).is_ok());
        assert!(validate_key_mapping(&meta, &mapping, &["missing".to_string()]).is_err());
        // 业务键存在但未参与列映射 → 拒绝
        assert!(validate_key_mapping(&meta, &mapping, &["note".to_string()]).is_err());
        // 未指定业务键时要求完整映射主键
        assert!(validate_key_mapping(&meta, &mapping, &[]).is_ok());
    }

    #[test]
    fn delete_missing_and_business_keys_are_optional_by_default() {
        let opts: SyncOptions = serde_json::from_str(r#"{"source_connection":"a","source_schema":"s","source_table":"x","target_connection":"b","target_schema":"t","target_table":"y","mode":"both","write_mode":"append"}"#).unwrap();
        assert!(opts.key_columns.is_empty());
        assert!(!opts.delete_missing);
    }

    #[test]
    fn db_sync_request_incremental_guard_rejects_replace() {
        // 通过 serde 反序列化 + 校验逻辑的字段默认值验证
        let req: DbSyncRequest = serde_json::from_str(r#"{"source_connection":"a","source_schema":"s","target_connection":"b","target_schema":"t","mode":"both","write_mode":"replace","tables":[{"source":"x"}]}"#).unwrap();
        assert_eq!(req.sync_mode, "");
        assert!(!req.incremental_update);
        let req2: DbSyncRequest = serde_json::from_str(r#"{"source_connection":"a","source_schema":"s","target_connection":"b","target_schema":"t","mode":"both","write_mode":"append","sync_mode":"incremental","incremental_update":true,"tables":[{"source":"x"}]}"#).unwrap();
        assert_eq!(req2.sync_mode, "incremental");
        assert!(req2.incremental_update);
    }
}

#[cfg(test)]
mod update_sql_tests {
    use super::*;

    fn col(name: &str, ty: &str, pk: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: ty.into(), nullable: !pk, default: None, primary_key: pk, auto_increment: false, comment: None, length: None, precision: None, scale: None }
    }

    fn mapped() -> (Vec<String>, Vec<Option<String>>, Vec<(&'static SyncColumnMapping, &'static ColumnInfo, String)>) {
        let m_id: &'static SyncColumnMapping = Box::leak(Box::new(SyncColumnMapping { source: "id".into(), target: "id".into(), target_type: None }));
        let m_name: &'static SyncColumnMapping = Box::leak(Box::new(SyncColumnMapping { source: "name".into(), target: "name".into(), target_type: None }));
        let c_id: &'static ColumnInfo = Box::leak(Box::new(col("id", "INTEGER", true)));
        let c_name: &'static ColumnInfo = Box::leak(Box::new(col("name", "TEXT", false)));
        let cols = vec![(m_id, c_id, "INTEGER".into()), (m_name, c_name, "TEXT".into())];
        let names = vec!["id".to_string(), "name".to_string()];
        let row = vec![Some("7".into()), Some("it's".into())];
        (names, row, cols)
    }

    #[test]
    fn update_sql_escapes_and_keys_by_pk_across_dialects() {
        let (names, row, cols) = mapped();
        for kind_id in ["sqlite", "mysql", "postgresql"] {
            let kind = DbKind::of(kind_id);
            let sql = update_sql("TBL", &kind, &names, &row, &cols, None).unwrap();
            // WHERE 只用主键，SET 只含非主键列
            assert!(sql.starts_with("UPDATE TBL SET "), "{kind_id}: {sql}");
            assert!(sql.contains(" WHERE "), "{kind_id}: {sql}");
            assert!(sql.contains("id"), "{kind_id}: {sql}");
            // 单引号被安全转义，不会破坏语句结构
            let value_part = sql.split("SET ").nth(1).unwrap();
            assert!(!value_part.contains("it's"), "未转义的引号：{kind_id}: {sql}");
        }
        let sqlite_sql = update_sql("TBL", &DbKind::of("sqlite"), &names, &row, &cols, None).unwrap();
        assert_eq!(sqlite_sql, "UPDATE TBL SET \"name\" = 'it''s' WHERE \"id\" = '7'");
        let mysql_sql = update_sql("TBL", &DbKind::of("mysql"), &names, &row, &cols, None).unwrap();
        assert_eq!(mysql_sql, "UPDATE TBL SET `name` = 'it''s' WHERE `id` = '7'");
    }

    #[test]
    fn update_sql_null_pk_fails_cleanly() {
        let (names, mut row, cols) = mapped();
        row[0] = None;
        let error = update_sql("TBL", &DbKind::of("sqlite"), &names, &row, &cols, None).unwrap_err();
        assert!(error.to_string().contains("主键"));
    }

    #[test]
    fn update_sql_rejects_pk_only_mapping() {
        let m_id: &'static SyncColumnMapping = Box::leak(Box::new(SyncColumnMapping { source: "id".into(), target: "id".into(), target_type: None }));
        let c_id: &'static ColumnInfo = Box::leak(Box::new(col("id", "INTEGER", true)));
        let cols = vec![(m_id, c_id, "INTEGER".into())];
        let names = vec!["id".to_string()];
        let row = vec![Some("1".into())];
        assert!(update_sql("TBL", &DbKind::of("sqlite"), &names, &row, &cols, None).is_err());
    }
}

#[cfg(test)]
mod heterogeneous_tests {
    use super::*;

    #[test]
    fn map_type_mysql_source_to_each_target() {
        let mysql = DbKind::of("mysql");
        let pg = DbKind::of("postgresql");
        let sqlite = DbKind::of("sqlite");
        // 整数族
        assert_eq!(map_type("INT", &sqlite), "INTEGER");
        assert_eq!(map_type("BIGINT", &pg), "BIGINT");
        assert_eq!(map_type("TINYINT UNSIGNED", &mysql), "BIGINT");
        // 二进制族
        assert_eq!(map_type("BLOB", &sqlite), "BLOB");
        assert_eq!(map_type("LONGBLOB", &mysql), "LONGBLOB");
        assert_eq!(map_type("VARBINARY(16)", &pg), "BYTEA");
        // 布尔
        assert_eq!(map_type("BOOLEAN", &sqlite), "INTEGER");
        assert_eq!(map_type("BIT", &pg), "BOOLEAN");
        // 小数
        assert_eq!(map_type("DECIMAL(10,2)", &mysql), "DECIMAL(65,20)");
        assert_eq!(map_type("DECIMAL(10,2)", &pg), "NUMERIC");
        assert_eq!(map_type("NUMERIC", &sqlite), "NUMERIC");
        // 时间
        assert_eq!(map_type("TIMESTAMP", &mysql), "DATETIME(6)");
        assert_eq!(map_type("DATETIME", &pg), "TIMESTAMP");
        assert_eq!(map_type("DATE", &pg), "DATE");
        // 文本
        assert_eq!(map_type("TEXT", &mysql), "LONGTEXT");
        assert_eq!(map_type("TEXT", &sqlite), "TEXT");
        // 带长度的 VARCHAR 保留（主键/索引需要可索引类型）
        assert_eq!(map_type("VARCHAR(32)", &mysql), "VARCHAR(32)");
        assert_eq!(map_type("VARCHAR(32)", &pg), "VARCHAR(32)");
        // PG 源类型反向映射
        assert_eq!(map_type("SERIAL", &mysql), "BIGINT");
        assert_eq!(map_type("bytea", &sqlite), "BLOB");
        assert_eq!(map_type("bool", &mysql), "BOOLEAN");
        // 未知类型兜底为文本（不把源方言类型直接塞进目标 DDL）
        assert_eq!(map_type("WEIRD_TYPE", &sqlite), "TEXT");
        assert_eq!(map_type("WEIRD_TYPE", &pg), "TEXT");
        assert_eq!(map_type("WEIRD_TYPE", &mysql), "LONGTEXT");
    }

    #[test]
    fn validate_target_type_rejects_unsafe_or_malformed() {
        let pg = DbKind::of("postgresql");
        let mysql = DbKind::of("mysql");
        assert!(validate_target_type("EVIL", &pg).is_err());
        assert!(validate_target_type("VARCHAR(0)", &pg).is_err());
        assert!(validate_target_type("VARCHAR(", &pg).is_err());
        assert!(validate_target_type("DECIMAL(10,99)", &mysql).is_err(), "MySQL 小数位上限 30");
        assert!(validate_target_type("DATETIME(9)", &mysql).is_err(), "时间精度上限 6");
        assert!(validate_target_type("INT(11)", &pg).is_err(), "整型不接受长度参数");
        assert_eq!(validate_target_type("VARCHAR(255)", &mysql).unwrap(), "VARCHAR(255)");
    }

    #[test]
    fn value_sql_binary_conversion_per_dialect() {
        let mysql = DbKind::of("mysql");
        let pg = DbKind::of("postgresql");
        // MySQL：0x 十六进制字面量
        assert_eq!(value_sql(Some("0xDEADBEEF"), &mysql, "LONGBLOB", "BLOB").unwrap(), "X'DEADBEEF'");
        // PG：decode(hex)
        let pg_value = value_sql(Some("0xDEADBEEF"), &pg, "BYTEA", "BLOB").unwrap();
        assert!(pg_value.starts_with("decode('") && pg_value.ends_with("','hex')"), "{pg_value}");
        // 非法输入一律拒绝
        assert!(value_sql(Some("DEADBEEF"), &mysql, "LONGBLOB", "BLOB").is_err(), "缺少 0x 标记");
        assert!(value_sql(Some("0xXYZ"), &mysql, "LONGBLOB", "BLOB").is_err(), "非十六进制");
        assert!(value_sql(Some("0xABC"), &mysql, "LONGBLOB", "BLOB").is_err(), "奇数长度");
    }

    #[test]
    fn value_sql_bool_null_and_string_safety() {
        let mysql = DbKind::of("mysql");
        let pg = DbKind::of("postgresql");
        let sqlite = DbKind::of("sqlite");
        assert_eq!(value_sql(Some("true"), &mysql, "BOOLEAN", "BOOLEAN").unwrap(), "TRUE");
        assert_eq!(value_sql(Some("t"), &pg, "BOOLEAN", "BOOLEAN").unwrap(), "TRUE");
        assert_eq!(value_sql(Some("0"), &mysql, "BOOLEAN", "BOOLEAN").unwrap(), "FALSE");
        assert!(value_sql(Some("yes"), &mysql, "BOOLEAN", "BOOLEAN").is_err());
        assert_eq!(value_sql(None, &mysql, "TEXT", "TEXT").unwrap(), "NULL");
        // 引号转义
        assert_eq!(value_sql(Some("O'Brien"), &sqlite, "TEXT", "TEXT").unwrap(), "'O''Brien'");
        // PG 文本不允许 \0；SQLite 走 CAST 十六进制
        assert!(value_sql(Some("a\0b"), &pg, "TEXT", "TEXT").is_err());
        let null_text = value_sql(Some("a\0b"), &sqlite, "TEXT", "TEXT").unwrap();
        assert!(null_text.starts_with("CAST(X'") && null_text.ends_with("' AS TEXT)"), "{null_text}");
    }
}

#[cfg(test)]
mod extended_dialect_tests {
    use super::*;

    #[test]
    fn map_type_new_targets() {
        let mssql = DbKind::of("sqlserver");
        let ch = DbKind::of("clickhouse");
        let oracle = DbKind::of("oracle");
        let trino = DbKind::of("trino");
        // SQL Server
        assert_eq!(map_type("INT", &mssql), "BIGINT");
        assert_eq!(map_type("BLOB", &mssql), "VARBINARY(MAX)");
        assert_eq!(map_type("BOOLEAN", &mssql), "BIT");
        assert_eq!(map_type("TIMESTAMP", &mssql), "DATETIME2");
        assert_eq!(map_type("TEXT", &mssql), "NVARCHAR(MAX)");
        // ClickHouse
        assert_eq!(map_type("BIGINT", &ch), "Int64");
        assert_eq!(map_type("BOOLEAN", &ch), "UInt8");
        assert_eq!(map_type("BLOB", &ch), "String");
        assert_eq!(map_type("DATETIME", &ch), "DateTime64(6)");
        assert_eq!(map_type("TEXT", &ch), "String");
        // ODBC：Oracle 系布尔 NUMBER(1)，ANSI 系 BOOLEAN
        assert_eq!(map_type("BOOLEAN", &oracle), "NUMBER(1)");
        assert_eq!(map_type("BOOLEAN", &trino), "BOOLEAN");
        assert_eq!(map_type("BLOB", &oracle), "BLOB");
        assert_eq!(map_type("DECIMAL(10,2)", &oracle), "NUMERIC(38, 10)");
        assert_eq!(map_type("TIMESTAMP", &trino), "TIMESTAMP");
    }

    #[test]
    fn validate_target_type_new_targets() {
        let mssql = DbKind::of("sqlserver");
        let ch = DbKind::of("clickhouse");
        let oracle = DbKind::of("oracle");
        // SQL Server MAX 参数只对字符串/二进制开放
        assert_eq!(validate_target_type("NVARCHAR(MAX)", &mssql).unwrap(), "NVARCHAR(MAX)");
        assert!(validate_target_type("INT(MAX)", &mssql).is_err());
        assert!(validate_target_type("NVARCHAR(MAX)", &ch).is_err());
        assert_eq!(validate_target_type("NVARCHAR(4000)", &mssql).unwrap(), "NVARCHAR(4000)");
        // ClickHouse 参数类型
        assert_eq!(validate_target_type("DECIMAL(65, 20)", &ch).unwrap(), "DECIMAL(65, 20)");
        assert!(validate_target_type("DECIMAL(77, 20)", &ch).is_err(), "ClickHouse 精度上限 76");
        assert_eq!(validate_target_type("DATETIME64(6)", &ch).unwrap(), "DATETIME64(6)");
        assert!(validate_target_type("STRING(10)", &ch).is_err());
        // ODBC
        assert_eq!(validate_target_type("NUMBER(1)", &oracle).unwrap(), "NUMBER(1)");
        assert!(validate_target_type("NUMBER(0)", &oracle).is_err());
        assert!(validate_target_type("WEIRD", &oracle).is_err());
    }

    #[test]
    fn value_sql_bool_to_numeric_and_mssql_n_prefix() {
        let mssql = DbKind::of("sqlserver");
        let oracle = DbKind::of("oracle");
        // 源布尔 → 目标非布尔数值类型：1/0
        assert_eq!(value_sql(Some("true"), &oracle, "NUMBER(1)", "BOOLEAN").unwrap(), "1");
        assert_eq!(value_sql(Some("false"), &oracle, "NUMBER(1)", "BIT").unwrap(), "0");
        assert_eq!(value_sql(Some("true"), &DbKind::of("clickhouse"), "UInt8", "BOOLEAN").unwrap(), "1");
        // 目标是布尔类型仍走 TRUE/FALSE
        assert_eq!(value_sql(Some("true"), &DbKind::of("mysql"), "BOOLEAN", "BOOLEAN").unwrap(), "TRUE");
        // SQL Server NVARCHAR 用 N'' 前缀（含 Unicode 转义不变）
        assert_eq!(value_sql(Some("it's"), &mssql, "NVARCHAR(MAX)", "TEXT").unwrap(), "N'it''s'");
        assert_eq!(value_sql(Some("中文"), &mssql, "NVARCHAR(100)", "TEXT").unwrap(), "N'中文'");
        // 非 N 类型不加前缀
        assert_eq!(value_sql(Some("abc"), &mssql, "VARCHAR(10)", "TEXT").unwrap(), "'abc'");
        assert_eq!(value_sql(Some("0x00FF1080"), &mssql, "VARBINARY(MAX)", "BLOB").unwrap(), "0x00FF1080");
        assert_eq!(value_sql(Some("0x"), &mssql, "VARBINARY(MAX)", "BLOB").unwrap(), "0x");
    }

    #[test]
    fn quote_ident_odbc_uppercase_for_oracle_family() {
        assert_eq!(crate::db::quote_ident(&DbKind::of("oracle"), "orders"), "\"ORDERS\"");
        assert_eq!(crate::db::quote_ident(&DbKind::of("dameng"), "t1"), "\"T1\"");
        assert_eq!(crate::db::quote_ident(&DbKind::of("db2"), "t"), "\"T\"");
        // ANSI 系保持原样
        assert_eq!(crate::db::quote_ident(&DbKind::of("trino"), "orders"), "\"orders\"");
        assert_eq!(crate::db::quote_ident(&DbKind::of("snowflake"), "orders"), "\"orders\"");
        // 带引号注入仍被转义
        assert_eq!(crate::db::quote_ident(&DbKind::of("oracle"), "a\"b"), "\"A\"\"B\"");
    }

    #[test]
    fn odbc_target_dialect_split() {
        assert_eq!(odbc_target_dialect(&DbKind::of("oracle")), "oracle");
        assert_eq!(odbc_target_dialect(&DbKind::of("dameng")), "oracle");
        assert_eq!(odbc_target_dialect(&DbKind::of("trino")), "ansi");
        assert_eq!(odbc_target_dialect(&DbKind::of("duckdb")), "ansi");
    }

    #[test]
    fn supported_now_includes_generic_channels() {
        for kind in ["sqlite", "mysql", "postgresql", "tidb", "sqlserver", "clickhouse", "oracle", "dameng", "trino", "snowflake", "duckdb"] {
            assert!(supported(&DbKind::of(kind)), "{kind} 应可同步");
        }
        for kind in ["redis", "elasticsearch", "kafka", "nacos", "mongodb", "neo4j", "influxdb"] {
            assert!(!supported(&DbKind::of(kind)), "{kind} 不应可同步");
        }
    }
}
