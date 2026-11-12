//! 图形化写入：一次性、绑定窗口/页面/端点的服务端计划。
//! 所有定位、DML、影响行数检查与读回都在同一事务/连接完成；冲突绝不提交。

#[path = "jdbc_mutation_bridge.rs"]
pub(crate) mod jdbc;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{MySql, Postgres, Row, Sqlite};

use crate::db::{self, Db};
use crate::models::{ColumnInfo, DbKind, Protocol, TableSchema};

/// 精确整数/十进制/日期/二进制以文字穿过 IPC，不经过 JavaScript Number。
/// Int/Float 保留旧客户端兼容，Int 仅允许 JS 安全整数，Float 禁止 NaN/Infinity。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DbValue {
    Null,
    Text(String),
    Int(i64),
    Integer(String),
    Decimal(String),
    Float(f64),
    Bool(bool),
    /// RFC 4648 标准 base64，不是数据库字符串。
    Binary(String),
    Date(String),
    Time(String),
    Timestamp(String),
    Timestamptz(String),
    Uuid(String),
    Json(String),
}

impl DbValue {
    fn is_null(&self) -> bool { matches!(self, Self::Null) }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Int(n) if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(n) =>
                bail!("不安全的数字整数；请使用 integer 十进制字符串"),
            Self::Integer(s) => {
                if s.len() > 21 || !integer_text(s) { bail!("整数必须是完整的十进制文字"); }
                if s.parse::<i64>().is_err() && s.parse::<u64>().is_err() { bail!("整数超出 64 位范围"); }
            }
            Self::Decimal(s) => { decimal_value(s)?; }
            Self::Float(f) if !f.is_finite() => bail!("浮点值必须有限"),
            Self::Binary(s) => { binary_value(s)?; }
            Self::Date(s) => { date_value(s)?; }
            Self::Time(s) => { time_value(s)?; }
            Self::Timestamp(s) => { timestamp_value(s)?; }
            Self::Timestamptz(s) => { chrono::DateTime::parse_from_rfc3339(s).context("时间戳必须含明确时区")?; }
            Self::Uuid(s) => { uuid::Uuid::parse_str(s).context("UUID 格式非法")?; }
            Self::Json(s) => { serde_json::value::RawValue::from_string(s.clone()).context("JSON 格式非法")?; }
            _ => {}
        }
        if serde_json::to_vec(self)?.len() > VALUE_LIMIT { bail!("单个绑定值超过 16 MiB，未发送 SQL"); }
        Ok(())
    }
}

const VALUE_LIMIT: usize = 16 * 1024 * 1024;
const PLAN_LIMIT: usize = 32 * 1024 * 1024;
const PLAN_TTL: Duration = Duration::from_secs(120);
const PLAN_CAPACITY: usize = 512;
const PLAN_CACHE_BYTES: usize = 64 * 1024 * 1024;

fn integer_text(s: &str) -> bool {
    let s = s.strip_prefix('-').or_else(|| s.strip_prefix('+')).unwrap_or(s);
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn decimal_value(s: &str) -> Result<sqlx::types::BigDecimal> {
    if s.is_empty() || s.len() > 1024 || s.trim() != s { bail!("十进制值格式或长度非法"); }
    let mut parts = s.split(['e', 'E']);
    let mantissa = parts.next().unwrap_or_default();
    if let Some(exponent) = parts.next() {
        let exponent: i32 = exponent.parse().context("十进制指数非法")?;
        if exponent.unsigned_abs() > 1000 { bail!("十进制指数超过安全范围"); }
    }
    if parts.next().is_some() { bail!("十进制值含多个指数"); }
    let mantissa = mantissa.strip_prefix('-').or_else(|| mantissa.strip_prefix('+')).unwrap_or(mantissa);
    if !mantissa.bytes().any(|b| b.is_ascii_digit())
        || mantissa.bytes().filter(|b| *b == b'.').count() > 1
        || !mantissa.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        bail!("十进制值必须是完整数字文字");
    }
    sqlx::types::BigDecimal::from_str(s).context("十进制值非法")
}

fn binary_value(s: &str) -> Result<Vec<u8>> {
    if s.len() > VALUE_LIMIT { bail!("二进制值过大"); }
    let bytes = STANDARD.decode(s).context("二进制值必须是标准 base64")?;
    if STANDARD.encode(&bytes) != s { bail!("二进制 base64 编码不是规范形式"); }
    Ok(bytes)
}
fn date_value(s: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").context("日期必须是 YYYY-MM-DD")
}
fn time_value(s: &str) -> Result<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(s, "%H:%M:%S%.f").context("时间必须是 HH:MM:SS[.小数]")
}
fn timestamp_value(s: &str) -> Result<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f"))
        .context("无时区时间戳格式非法")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationDraft {
    pub connection_id: String,
    #[serde(default)] pub owner_id: String,
    #[serde(default)] pub schema: String,
    pub table: String,
    pub kind: String,
    #[serde(default)] pub changes: Vec<(String, DbValue)>,
    #[serde(default)] pub keys: Vec<(String, DbValue)>,
    #[serde(default)] pub original_values: Vec<(String, DbValue)>,
}
#[derive(Debug, Clone, Serialize)]
pub struct MutationPlan {
    pub plan_id: String,
    pub sql: String,
    pub binds: Vec<DbValue>,
    pub base_revision: String,
    pub expected_rows: u64,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct MutationReceipt {
    pub plan_id: String,
    pub status: String,
    /// 只统计最终已提交的影响行数；回滚冲突为 0。
    pub affected: u64,
    pub verified: bool,
    pub mismatches: Vec<String>,
    pub sql: String,
    pub transaction: String,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlanInternal {
    pub connection_id: String,
    pub principal: String,
    pub owner_id: String,
    pub endpoint_fingerprint: String,
    pub schema: String,
    pub table: String,
    pub kind: String,
    pub sql: String,
    pub binds: Vec<DbValue>,
    pub base_revision: String,
    pub expected_rows: u64,
    pub keys: Vec<(String, DbValue)>,
    pub conditions: Vec<(String, DbValue)>,
    pub changes: Vec<(String, DbValue)>,
    pub columns: Vec<ColumnInfo>,
    pub unique_keys: Vec<Vec<String>>,
    // Same opaque Registry/byte budget, no separate driver authorization state.
    // Omitted for native plans so their existing serialization/cache budget is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jdbc: Option<jdbc::JdbcPlanBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_source: Option<crate::query_result_mutation::QueryMutationSource>,
}
struct Entry { plan: PlanInternal, created: Instant, bytes: usize }
#[derive(Default)]
pub struct Registry { inner: Mutex<HashMap<String, Entry>> }
impl Registry {
    pub(crate) fn put(&self, plan: PlanInternal) -> Result<String> {
        // Account for duplicated binds/conditions as stored, not only the input draft.
        let bytes = serde_json::to_vec(&plan)?.len();
        if bytes > PLAN_LIMIT { bail!("服务端写入计划超过 32 MiB，未登记或发送 SQL"); }
        let id = uuid::Uuid::new_v4().to_string();
        let mut map = self.inner.lock();
        map.retain(|_, e| e.created.elapsed() < PLAN_TTL);
        while map.len() >= PLAN_CAPACITY || map.values().fold(bytes, |n, e| n.saturating_add(e.bytes)) > PLAN_CACHE_BYTES {
            if let Some(oldest) = map.iter().min_by_key(|(_, e)| e.created).map(|(id, _)| id.clone()) {
                map.remove(&oldest);
            } else { break; }
        }
        map.insert(id.clone(), Entry { plan, created: Instant::now(), bytes });
        Ok(id)
    }
    /// 错窗口/页面请求既不能执行，也不能消费另一所有者的计划。
    pub(crate) fn take(&self, id: &str, principal: &str, owner: &str) -> Result<PlanInternal> {
        let mut map = self.inner.lock();
        map.retain(|_, e| e.created.elapsed() < PLAN_TTL);
        let entry = map.get(id).ok_or_else(|| anyhow!("写入计划已过期或已执行，请重新预览"))?;
        if entry.plan.principal != principal || entry.plan.owner_id != owner {
            bail!("写入计划不属于当前窗口/页面");
        }
        Ok(map.remove(id).expect("entry checked under lock").plan)
    }
    pub fn clear_connection(&self, id: &str) { self.inner.lock().retain(|_, e| e.plan.connection_id != id); }
}

/// 使用真实目录定义而非行数/自增游标计算结构版本；事务内再次读取同一摘要。
/// 包含列默认值、生成/自增属性、索引、外键与触发器；权限失败不能当空元数据。
fn revision_rows(rows: &[Vec<Option<String>>]) -> Result<String> {
    if rows.is_empty() { bail!("目标表不存在或结构元数据不可读取"); }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(rows)?)))
}
fn mysql_stable_ddl(ddl: &str) -> String {
    // SHOW CREATE 的表选项 AUTO_INCREMENT=n 会随行写入增长，不是结构变化。
    // 字符串、注释和中文标识符原样保留，不能逐 UTF-8 字节转 char。
    let mut result = String::with_capacity(ddl.len());
    let mut i = 0;
    let mut quote = None;
    let mut escaped = false;
    while i < ddl.len() {
        let rest = &ddl[i..];
        let ch = rest.chars().next().expect("UTF-8 boundary");
        if let Some(q) = quote {
            result.push(ch);
            if escaped { escaped = false; }
            else if ch == '\\' { escaped = true; }
            else if ch == q { quote = None; }
        } else {
            if rest.starts_with(" AUTO_INCREMENT=") {
                let start = i + " AUTO_INCREMENT=".len();
                let mut end = start;
                while end < ddl.len() && ddl.as_bytes()[end].is_ascii_digit() { end += 1; }
                if end > start { i = end; continue; }
            }
            if matches!(ch, '\'' | '"' | '`') { quote = Some(ch); }
            result.push(ch);
        }
        i += ch.len_utf8();
    }
    result
}
async fn sqlite_revision(conn: &mut sqlx::SqliteConnection, schema: &str, table: &str) -> Result<String> {
    if !schema.is_empty() && schema != "main" { bail!("SQLite 写入目前仅支持 main 命名空间"); }
    let rows = crate::sql_history::query("SELECT type, name, sql FROM main.sqlite_master WHERE (tbl_name = ? OR name = ?) AND type IN ('table','index','trigger') ORDER BY type, name")
        .bind(table).bind(table).fetch_all(conn).await?;
    revision_rows(&rows.iter().map(|r| crate::values::sqlite_row_values(r, 3)).collect::<Vec<_>>())
}
async fn mysql_revision(conn: &mut sqlx::MySqlConnection, schema: &str, table: &str) -> Result<String> {
    let full = db::qualified(&DbKind::of("mysql"), schema, table);
    let rows = crate::sql_history::query(&format!("SHOW CREATE TABLE {full}")).fetch_all(conn).await?;
    let ddl = rows.first().and_then(|r| crate::values::my_row_values(r, r.columns().len()).get(1).cloned().flatten())
        .ok_or_else(|| anyhow!("表定义不可读取"))?;
    revision_rows(&[vec![Some(mysql_stable_ddl(&ddl))]])
}
async fn pg_revision(conn: &mut sqlx::PgConnection, schema: &str, table: &str) -> Result<String> {
    let row = crate::sql_history::query(
        "SELECT jsonb_build_object('oid', c.oid, 'columns', (SELECT jsonb_agg(jsonb_build_array(a.attname, format_type(a.atttypid,a.atttypmod), a.attnotnull,a.attidentity,a.attgenerated,pg_get_expr(d.adbin,d.adrelid)) ORDER BY a.attnum) FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped), 'indexes', (SELECT jsonb_agg(pg_get_indexdef(i.indexrelid) ORDER BY i.indexrelid) FROM pg_index i WHERE i.indrelid=c.oid), 'constraints', (SELECT jsonb_agg(jsonb_build_array(k.conname,pg_get_constraintdef(k.oid)) ORDER BY k.conname) FROM pg_constraint k WHERE k.conrelid=c.oid), 'triggers', (SELECT jsonb_agg(pg_get_triggerdef(t.oid) ORDER BY t.tgname) FROM pg_trigger t WHERE t.tgrelid=c.oid AND NOT t.tgisinternal))::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p')"
    ).bind(schema).bind(table).fetch_optional(conn).await?.ok_or_else(|| anyhow!("目标表不存在"))?;
    revision_rows(&[vec![row.try_get::<Option<String>, _>(0)?]])
}
pub async fn table_revision(db: &Db, kind: &DbKind, schema: &str, table: &str) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let _ = kind;
    match db {
        Db::Sqlite(pool) => sqlite_revision(&mut *pool.acquire().await?, schema, table).await,
        Db::Mysql(pool) => mysql_revision(&mut *pool.acquire().await?, schema, table).await,
        Db::Postgres(pool) => pg_revision(&mut *pool.acquire().await?, schema, table).await,
        _ => bail!("该驱动尚未提供可验证的结构版本"),
    }

    }).await
}

fn check_pairs(pairs: &[(String, DbValue)]) -> Result<()> {
    let mut seen = HashSet::new();
    for (name, value) in pairs {
        if name.is_empty() || name.contains('\0') || !seen.insert(name) { bail!("列名为空、非法或重复：{name}"); }
        value.validate()?;
    }
    Ok(())
}
fn combine_pairs(a: &[(String, DbValue)], b: &[(String, DbValue)]) -> Result<Vec<(String, DbValue)>> {
    let mut pairs = a.to_vec();
    for (name, value) in b {
        if let Some((_, existing)) = pairs.iter().find(|(n, _)| n == name) {
            if existing != value { bail!("定位原值互相冲突：{name}"); }
        } else { pairs.push((name.clone(), value.clone())); }
    }
    Ok(pairs)
}
fn marker(kind: &DbKind, value: &DbValue, binds: &mut Vec<DbValue>) -> String {
    if value.is_null() { return "NULL".into(); }
    binds.push(value.clone());
    if kind.is_pg_family() { format!("${}", binds.len()) } else { "?".into() }
}
fn conditions(kind: &DbKind, pairs: &[(String, DbValue)], binds: &mut Vec<DbValue>) -> String {
    pairs.iter().map(|(name, value)| {
        let col = db::quote_ident(kind, name);
        if value.is_null() { return format!("{col} IS NULL"); }
        let mark = marker(kind, value, binds);
        match (kind.protocol(), value) {
            (Protocol::Mysql, DbValue::Text(_)) => format!("BINARY {col} = BINARY {mark}"),
            (Protocol::Sqlite, DbValue::Text(_)) => format!("{col} COLLATE BINARY = {mark}"),
            (Protocol::Postgres, DbValue::Text(_)) => format!("{col}::text COLLATE \"C\" = {mark}::text COLLATE \"C\""),
            (Protocol::Postgres, DbValue::Json(_)) => format!("{col}::jsonb = {mark}::jsonb"),
            (Protocol::Mysql, DbValue::Json(_)) => format!("JSON_EXTRACT({col}, '$') = JSON_EXTRACT({mark}, '$')"),
            _ => format!("{col} = {mark}"),
        }
    }).collect::<Vec<_>>().join(" AND ")
}
pub(crate) fn compile(draft: &MutationDraft, kind: &DbKind, _revision: &str) -> Result<(String, Vec<DbValue>, u64, String)> {
    if !matches!(kind.protocol(), Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) { bail!("该协议尚未提供类型化写入"); }
    if draft.table.is_empty() || draft.table.contains('\0') { bail!("目标表名非法"); }
    check_pairs(&draft.changes)?; check_pairs(&draft.keys)?; check_pairs(&draft.original_values)?;
    if serde_json::to_vec(draft)?.len() > PLAN_LIMIT { bail!("写入草稿超过 32 MiB"); }
    let full = db::qualified(kind, &draft.schema, &draft.table);
    let mut binds = Vec::new();
    let sql = match draft.kind.as_str() {
        "insert" if draft.changes.is_empty() => {
            if kind.is_mysql_family() { format!("INSERT INTO {full} () VALUES ()") }
            else { format!("INSERT INTO {full} DEFAULT VALUES") }
        }
        "insert" => {
            let cols = draft.changes.iter().map(|(c, _)| db::quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
            let values = draft.changes.iter().map(|(_, v)| marker(kind, v, &mut binds)).collect::<Vec<_>>().join(", ");
            format!("INSERT INTO {full} ({cols}) VALUES ({values})")
        }
        "update" => {
            if draft.changes.is_empty() { bail!("没有要更新的列"); }
            let original = combine_pairs(&draft.keys, &draft.original_values)?;
            if original.is_empty() { bail!("缺少定位条件，无法更新该行"); }
            let sets = draft.changes.iter().map(|(c, v)| format!("{} = {}", db::quote_ident(kind, c), marker(kind, v, &mut binds))).collect::<Vec<_>>().join(", ");
            let where_sql = conditions(kind, &original, &mut binds);
            format!("UPDATE {full} SET {sets} WHERE {where_sql}")
        }
        "delete" => {
            if draft.keys.is_empty() && draft.original_values.is_empty() { bail!("缺少定位条件，无法删除该行"); }
            let original = combine_pairs(&draft.keys, &draft.original_values)?;
            let where_sql = conditions(kind, &original, &mut binds);
            format!("DELETE FROM {full} WHERE {where_sql}")
        }
        _ => bail!("未知写入类型"),
    };
    Ok((sql, binds, 1, format!("{} {} 的 1 行；冲突将回滚", draft.kind, full)))
}

fn unique_keys(schema: &TableSchema) -> Vec<Vec<String>> {
    let mut keys = Vec::new();
    let primary = schema.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect::<Vec<_>>();
    if !primary.is_empty() { keys.push(primary); }
    for index in &schema.indexes {
        if index.unique && !index.columns.is_empty() && index.columns.iter().all(|name| schema.columns.iter().any(|c| c.name == *name))
            && !keys.contains(&index.columns) { keys.push(index.columns.clone()); }
    }
    keys
}
fn pick_key(candidates: &[Vec<String>], values: &[(String, DbValue)]) -> Vec<(String, DbValue)> {
    for key in candidates {
        let pairs = key.iter().filter_map(|name| values.iter().find(|(n, v)| n == name && !v.is_null()).cloned()).collect::<Vec<_>>();
        if pairs.len() == key.len() { return pairs; }
    }
    Vec::new()
}
fn column_base(column: &ColumnInfo) -> String {
    let mut result = String::new();
    let mut depth = 0usize;
    for ch in column.data_type.chars() {
        if ch == '(' { depth += 1; }
        else if ch == ')' { depth = depth.saturating_sub(1); }
        else if depth == 0 { result.push(ch); }
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}
fn normalize_value(kind: &DbKind, column: &ColumnInfo, value: &DbValue) -> Result<DbValue> {
    value.validate()?;
    if value.is_null() { return Ok(value.clone()); }
    let base = column_base(column);
    let scalar = base.strip_suffix(" unsigned").unwrap_or(&base);
    let integer = matches!(scalar, "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "int2" | "int4" | "int8" | "serial" | "bigserial" | "smallserial");
    let decimal = matches!(scalar, "decimal" | "numeric" | "number");
    let float = matches!(scalar, "float" | "double" | "double precision" | "real" | "float4" | "float8");
    let text_affinity = ["char", "clob", "text"].iter().any(|part| base.contains(part));
    let valid = if integer { matches!(value, DbValue::Integer(_) | DbValue::Int(_)) }
        else if decimal { matches!(value, DbValue::Decimal(_)) }
        else if float { matches!(value, DbValue::Float(_)) }
        else if matches!(scalar, "bool" | "boolean") { matches!(value, DbValue::Bool(_)) }
        else if matches!(scalar, "blob" | "tinyblob" | "mediumblob" | "longblob" | "bytea" | "binary" | "varbinary") { matches!(value, DbValue::Binary(_)) }
        else if scalar == "date" { matches!(value, DbValue::Date(_)) }
        else if matches!(scalar, "time" | "time without time zone") { matches!(value, DbValue::Time(_)) }
        else if matches!(scalar, "timestamp" | "datetime" | "timestamp without time zone") { matches!(value, DbValue::Timestamp(_)) }
        else if matches!(scalar, "timestamptz" | "timestamp with time zone") { matches!(value, DbValue::Timestamptz(_)) }
        else if scalar == "uuid" { matches!(value, DbValue::Uuid(_)) }
        else if matches!(scalar, "json" | "jsonb") { matches!(value, DbValue::Json(_)) }
        else if scalar.ends_with("[]") || matches!(scalar, "timetz" | "time with time zone" | "money") { false }
        else { matches!(value, DbValue::Text(_)) || (kind.is_sqlite() && text_affinity && matches!(value, DbValue::Decimal(_))) };
    if !valid { bail!("绑定值类型与目标列 {}（{}）不匹配，或该类型尚不支持无损写入", column.name, column.data_type); }
    if kind.is_sqlite() && matches!(value, DbValue::Decimal(_)) && !text_affinity {
        bail!("SQLite 数值亲和列无法保证十进制无损；请使用 TEXT 列或原生精确数值数据库");
    }
    if kind.is_sqlite() && matches!(scalar, "json" | "jsonb") {
        if let DbValue::Json(s) = value {
            if decimal_value(s).is_ok() { bail!("SQLite JSON 数值亲和列不能无损存储数字根值；请使用 TEXT 列"); }
        }
    }
    if let DbValue::Float(number) = value {
        if (kind.is_pg_family() && matches!(scalar, "real" | "float4")) || (kind.is_mysql_family() && scalar == "float") {
            let rounded = *number as f32;
            if !rounded.is_finite() { bail!("浮点值超出目标 32 位浮点范围：{}", column.name); }
            return Ok(DbValue::Float(rounded as f64));
        }
    }
    Ok(value.clone())
}

pub(crate) fn build_plan(draft: &MutationDraft, kind: &DbKind, metadata: &TableSchema, revision: String,
    principal: &str, fingerprint: String) -> Result<PlanInternal> {
    if draft.owner_id.trim().is_empty() { bail!("必须指定写入所属页面"); }
    if kind.is_mysql_family() && metadata.engine.as_deref().map_or(true, |engine| !engine.eq_ignore_ascii_case("InnoDB")) {
        bail!("MySQL 图形化写入仅允许已确认 InnoDB 的事务表；非事务或未知引擎不能保证冲突回滚");
    }
    let mut draft = draft.clone();
    if draft.kind == "insert" {
        let primary_count = metadata.columns.iter().filter(|c| c.primary_key).count();
        draft.changes.retain(|(name, value)| !value.is_null() || !metadata.columns.iter().any(|column|
            column.name == *name && (column.auto_increment || (kind.is_sqlite() && primary_count == 1 && column.primary_key && column_base(column) == "integer"))));
    }
    for pairs in [&mut draft.changes, &mut draft.keys, &mut draft.original_values] {
        for (name, value) in pairs {
            let column = metadata.columns.iter().find(|c| c.name == *name).ok_or_else(|| anyhow!("目标列不存在：{name}"))?;
            *value = normalize_value(kind, column, value)?;
        }
    }
    for (name, value) in &draft.changes {
        let column = metadata.columns.iter().find(|c| c.name == *name).expect("validated target column");
        if value.is_null() && !column.nullable { bail!("非空列不能写入 NULL：{name}"); }
    }
    let (sql, binds, expected_rows, _) = compile(&draft, kind, &revision)?;
    let unique_keys = unique_keys(metadata);
    let keys = if draft.kind == "insert" { pick_key(&unique_keys, &draft.changes) }
        else if !draft.keys.is_empty() { draft.keys.clone() } else { draft.original_values.clone() };
    if draft.kind == "insert" && unique_keys.is_empty() { bail!("目标表没有可验证的唯一键，不能安全读回新增行"); }
    if draft.kind == "insert" && kind.is_mysql_family() && keys.is_empty()
        && !metadata.columns.iter().any(|c| c.auto_increment && unique_keys.iter().any(|key| key.contains(&c.name))) {
        bail!("MySQL 新增行缺少完整唯一键，且没有可读回的自增键");
    }
    Ok(PlanInternal { connection_id: draft.connection_id.clone(), principal: principal.into(), owner_id: draft.owner_id.clone(),
        endpoint_fingerprint: fingerprint, schema: draft.schema.clone(), table: draft.table.clone(), kind: draft.kind.clone(),
        sql, binds, base_revision: revision, expected_rows, keys,
        conditions: combine_pairs(&draft.keys, &draft.original_values)?, changes: draft.changes.clone(), columns: metadata.columns.clone(), unique_keys, jdbc: None, query_source: None })
}

fn bind_sqlite<'q>(q: crate::sql_history::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>, value: &DbValue)
    -> Result<crate::sql_history::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>> {
    Ok(match value {
        DbValue::Null => q.bind(None::<String>),
        DbValue::Text(s) | DbValue::Decimal(s) | DbValue::Date(s) | DbValue::Time(s) | DbValue::Timestamp(s) | DbValue::Timestamptz(s) | DbValue::Uuid(s) | DbValue::Json(s) => q.bind(s.clone()),
        DbValue::Integer(s) => q.bind(s.parse::<i64>().context("SQLite 整数超出有符号 64 位范围")?),
        DbValue::Int(n) => q.bind(*n), DbValue::Float(n) => q.bind(*n), DbValue::Bool(b) => q.bind(*b),
        DbValue::Binary(s) => q.bind(binary_value(s)?),
    })
}
fn bind_mysql<'q>(q: crate::sql_history::Query<'q, MySql, sqlx::mysql::MySqlArguments>, value: &DbValue)
    -> Result<crate::sql_history::Query<'q, MySql, sqlx::mysql::MySqlArguments>> {
    Ok(match value {
        DbValue::Null => q.bind(None::<String>), DbValue::Text(s) | DbValue::Uuid(s) => q.bind(s.clone()),
        DbValue::Int(n) => q.bind(*n),
        DbValue::Integer(s) => match s.parse::<i64>() { Ok(n) => q.bind(n), Err(_) => q.bind(s.parse::<u64>()?) },
        DbValue::Decimal(s) => q.bind(decimal_value(s)?), DbValue::Float(n) => q.bind(*n), DbValue::Bool(b) => q.bind(*b),
        DbValue::Binary(s) => q.bind(binary_value(s)?), DbValue::Date(s) => q.bind(date_value(s)?),
        DbValue::Time(s) => q.bind(time_value(s)?), DbValue::Timestamp(s) => q.bind(timestamp_value(s)?),
        DbValue::Timestamptz(_) => bail!("MySQL 无法无损绑定带时区时间戳；请明确目标时区后使用无时区值"),
        DbValue::Json(s) => q.bind(sqlx::types::Json(serde_json::value::RawValue::from_string(s.clone())?)),
    })
}
fn bind_pg<'q>(q: crate::sql_history::Query<'q, Postgres, sqlx::postgres::PgArguments>, value: &DbValue)
    -> Result<crate::sql_history::Query<'q, Postgres, sqlx::postgres::PgArguments>> {
    Ok(match value {
        DbValue::Null => q.bind(None::<String>), DbValue::Text(s) => q.bind(s.clone()), DbValue::Int(n) => q.bind(*n),
        DbValue::Integer(s) => q.bind(s.parse::<i64>().context("PostgreSQL 整数超出有符号 64 位范围")?),
        DbValue::Decimal(s) => q.bind(decimal_value(s)?), DbValue::Float(n) => q.bind(*n), DbValue::Bool(b) => q.bind(*b),
        DbValue::Binary(s) => q.bind(binary_value(s)?), DbValue::Date(s) => q.bind(date_value(s)?),
        DbValue::Time(s) => q.bind(time_value(s)?), DbValue::Timestamp(s) => q.bind(timestamp_value(s)?),
        DbValue::Timestamptz(s) => q.bind(chrono::DateTime::parse_from_rfc3339(s)?.with_timezone(&chrono::Utc)),
        DbValue::Uuid(s) => q.bind(uuid::Uuid::parse_str(s)?),
        DbValue::Json(s) => q.bind(sqlx::types::Json(serde_json::value::RawValue::from_string(s.clone())?)),
    })
}
fn value_from_readback(value: &str, column: &ColumnInfo) -> Result<DbValue> {
    let base = column_base(column);
    let scalar = base.strip_suffix(" unsigned").unwrap_or(&base);
    let v = if matches!(scalar, "bigint" | "integer" | "int" | "int8" | "int4" | "int2" | "smallint" | "mediumint" | "tinyint") { DbValue::Integer(value.into()) }
        else if matches!(scalar, "numeric" | "decimal" | "number") { DbValue::Decimal(value.into()) }
        else if matches!(scalar, "float" | "double" | "double precision" | "real" | "float4" | "float8") { DbValue::Float(value.parse().context("浮点键解码失败")?) }
        else if matches!(scalar, "bool" | "boolean") { match value { "1" | "true" | "t" => DbValue::Bool(true), "0" | "false" | "f" => DbValue::Bool(false), _ => bail!("布尔键解码失败") } }
        else if scalar == "uuid" { DbValue::Uuid(value.into()) }
        else if scalar == "date" { DbValue::Date(value.into()) }
        else if matches!(scalar, "time" | "time without time zone") { DbValue::Time(value.into()) }
        else if matches!(scalar, "timestamp" | "datetime" | "timestamp without time zone") { DbValue::Timestamp(value.into()) }
        else if matches!(scalar, "timestamptz" | "timestamp with time zone") { DbValue::Timestamptz(value.into()) }
        else if matches!(scalar, "json" | "jsonb") { DbValue::Json(value.into()) }
        else if matches!(scalar, "blob" | "tinyblob" | "mediumblob" | "longblob" | "bytea" | "binary" | "varbinary") {
            let hex = value.strip_prefix("0x").ok_or_else(|| anyhow!("二进制键缺少明确编码"))?;
            if hex.len() % 2 != 0 || hex.len() > VALUE_LIMIT * 2 { bail!("二进制键编码非法或过大"); }
            let bytes = hex.as_bytes().chunks_exact(2).map(|pair| {
                let text = std::str::from_utf8(pair).context("二进制键编码非法")?;
                u8::from_str_radix(text, 16).context("二进制键编码非法")
            }).collect::<Result<Vec<_>>>()?;
            DbValue::Binary(STANDARD.encode(bytes))
        } else { DbValue::Text(value.into()) };
    v.validate()?;
    Ok(v)
}
fn returned_keys(plan: &PlanInternal, names: &[String], values: &[Option<String>]) -> Result<Vec<(String, DbValue)>> {
    let mut pairs = Vec::new();
    for column in &plan.columns {
        if !plan.unique_keys.iter().any(|key| key.contains(&column.name)) { continue; }
        if let Some(value) = names.iter().position(|n| n == &column.name).and_then(|i| values.get(i)).and_then(|s| s.as_deref()) {
            pairs.push((column.name.clone(), value_from_readback(value, column)?));
        }
    }
    let keys = pick_key(&plan.unique_keys, &pairs);
    if keys.is_empty() { bail!("写入后未取得完整、非空的唯一键"); }
    Ok(keys)
}
fn mysql_insert_keys(plan: &PlanInternal, last_id: u64) -> Result<Vec<(String, DbValue)>> {
    if !plan.keys.is_empty() { return Ok(plan.keys.clone()); }
    let mut values = plan.changes.clone();
    if let Some(column) = plan.columns.iter().find(|c| c.auto_increment) {
        values.retain(|(name, _)| name != &column.name);
        values.push((column.name.clone(), DbValue::Integer(last_id.to_string())));
    }
    let keys = pick_key(&plan.unique_keys, &values);
    if keys.is_empty() { bail!("未取得新增行完整唯一键"); }
    Ok(keys)
}
fn receipt(plan: &PlanInternal, status: &str, affected: u64, verified: bool, message: Option<String>, transaction: &str) -> MutationReceipt {
    MutationReceipt { plan_id: String::new(), status: status.into(), affected, verified,
        mismatches: message.into_iter().collect(), sql: plan.sql.clone(), transaction: transaction.into() }
}

/// Controlled C2 backend dispatch. Window/principal/owner/lease/current saved
/// endpoint/read-only admission still belongs to the command service, not here.
/// Native SQLx keeps its existing compiler, engine gate and atomic verification.
pub(crate) async fn prepare_plan(db: &Db, kind: &DbKind, draft: &MutationDraft,
    principal: &str, fingerprint: String) -> Result<PlanInternal> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match db {
        Db::Http(http) => jdbc::preview(http, draft, principal, fingerprint).await,
        Db::Sqlite(_) | Db::Mysql(_) | Db::Postgres(_) => {
            let revision = table_revision(db, kind, &draft.schema, &draft.table).await?;
            let metadata = crate::schema::table_schema(db, kind.clone(), &draft.schema, &draft.table).await?;
            if revision != table_revision(db, kind, &draft.schema, &draft.table).await? {
                return Err(anyhow!(crate::error::AppError::coded("MUTATION_CONTEXT_CHANGED", "读取写入元数据时表结构发生变化，请重新预览")));
            }
            build_plan(draft, kind, &metadata, revision, principal, fingerprint)
        }
        _ => Err(anyhow!(crate::error::AppError::coded("UNSUPPORTED_MUTATION", "当前实际适配器没有经过认证的类型化写入通道"))),
    }

    }).await
}

/// Checks backend selection and real JDBC physical session/object snapshot.
/// Native directory/row validation remains inside the original owned transaction.
pub(crate) async fn validate_plan_context(db: &Db, plan: &PlanInternal) -> Result<()> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    match (plan.jdbc.as_ref(), db) {
        (Some(_), Db::Http(http)) => jdbc::validate_context(http, plan).await,
        (None, Db::Sqlite(_) | Db::Mysql(_) | Db::Postgres(_)) => Ok(()),
        _ => Err(anyhow!(crate::error::AppError::coded("MUTATION_CONTEXT_CHANGED", "实际物理适配器与冻结写入计划不一致"))),
    }

    }).await
}

/// Send exactly one consumed server plan. Never synthesize SQL/affected counts
/// for REST, never retry a driver operation or fall back to native literal SQL.
pub(crate) async fn execute_plan(db: &Db, kind: &DbKind, plan_id: &str, plan: &PlanInternal,
    effective_read_only: bool, timeout_secs: u64) -> Result<MutationReceipt> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if effective_read_only {
        return Err(anyhow!(crate::error::AppError::coded("READ_ONLY_VIOLATION", "目标连接当前为只读，不能执行图形化写入")));
    }
    let mut receipt = match (plan.jdbc.as_ref(), db) {
        (Some(_), Db::Http(http)) => jdbc::execute(http, plan_id, plan, false, timeout_secs).await?,
        (None, Db::Sqlite(_) | Db::Mysql(_) | Db::Postgres(_)) => {
            let revision = table_revision(db, kind, &plan.schema, &plan.table).await?;
            run_plan(db, kind, plan, &revision).await.map_err(|error| {
                if error.to_string().contains("MUTATION_EFFECTS_UNKNOWN") {
                    anyhow!(crate::error::AppError::coded("MUTATION_EFFECTS_UNKNOWN", "原生事务提交确认丢失，禁止自动重放，请隔离并核对实际数据").operation(plan_id))
                } else { error }
            })?
        }
        _ => return Err(anyhow!(crate::error::AppError::coded("MUTATION_CONTEXT_CHANGED", "实际物理适配器与冻结写入计划不一致"))),
    };
    receipt.plan_id = plan_id.to_owned();
    Ok(receipt)

    }).await
}

async fn validate_sqlite_query_source(conn: &mut sqlx::SqliteConnection, plan: &PlanInternal) -> Result<()> {
    if let Some(source) = &plan.query_source { crate::query_lineage::validate_source(conn, &source.source).await?; }
    Ok(())
}
async fn reject_mysql_query_source(_: &mut sqlx::MySqlConnection, plan: &PlanInternal) -> Result<()> {
    if plan.query_source.is_some() { bail!("UNSUPPORTED_QUERY_MUTATION：SQLite 查询来源不能进入 MySQL 写入通道"); } Ok(())
}
async fn reject_pg_query_source(_: &mut sqlx::PgConnection, plan: &PlanInternal) -> Result<()> {
    if plan.query_source.is_some() { bail!("UNSUPPORTED_QUERY_MUTATION：SQLite 查询来源不能进入 PostgreSQL 写入通道"); } Ok(())
}

/// 计划必须由 build_plan 产生；每个分支在同一连接上读目录/锁行/执行/验证。
/// 不抛弃发送后的 future；提交错误明确标 effects unknown，调用者不可重放。
pub(crate) async fn run_plan(db: &Db, kind: &DbKind, plan: &PlanInternal, current_revision: &str) -> Result<MutationReceipt> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if plan.jdbc.is_some() { bail!("JDBC 写入计划只能由类型化 JDBC owned transaction 通道执行，禁止原生 SQL 回落"); }
    if plan.base_revision != current_revision {
        return Ok(receipt(plan, "conflict", 0, false, Some("表结构已变化，请重新预览".into()), "none"));
    }
    let full = db::qualified(kind, &plan.schema, &plan.table);
    macro_rules! run_atomic {
        ($pool:expr, $bind:ident, $revision:ident, $values:expr, $returning:expr, $mysql:expr, $pg:expr, $source_validator:ident) => {{
            // Keep the owning pool connection separate from its borrowed transaction.
            // An error/uncertain commit must discard it, not hand a tainted session back to the pool.
            let mut owned = $pool.acquire().await?;
            let result: Result<MutationReceipt> = async {
            let mut tx = sqlx::Connection::begin(&mut *owned).await?;
            // PG 的 relation lock 与 DDL 冲突；MySQL SELECT 持有 MDL；SQLite 目录读持有 schema 读锁。
            if $pg { crate::sql_history::query(&format!("LOCK TABLE {full} IN ROW EXCLUSIVE MODE")).execute(&mut *tx).await?; }
            if $mysql { crate::sql_history::query(&format!("SELECT 1 FROM {full} LIMIT 0")).fetch_all(&mut *tx).await?; }
            let actual = $revision(&mut *tx, &plan.schema, &plan.table).await?;
            if actual != plan.base_revision {
                tx.rollback().await?;
                return Ok(receipt(plan, "conflict", 0, false, Some("锁定目标后结构版本已变化，未发送写入".into()), "rolled_back"));
            }
            // Query origin is rechecked after the same transaction acquired its schema read lock,
            // before row lookup or DML. Never validate a second pooled connection instead.
            $source_validator(&mut *tx, plan).await?;
            if plan.kind != "insert" {
                let mut binds = Vec::new();
                let where_sql = conditions(kind, &plan.conditions, &mut binds);
                if where_sql.is_empty() { bail!("服务端计划缺少原值定位条件"); }
                let lock = if $mysql || $pg { " FOR UPDATE" } else { "" };
                let sql = format!("SELECT 1 FROM {full} WHERE {where_sql} LIMIT 2{lock}");
                let mut q = crate::sql_history::query(&sql);
                for v in &binds { q = $bind(q, v)?; }
                let rows = q.fetch_all(&mut *tx).await?;
                if rows.len() != 1 {
                    let message = format!("原值定位命中 {} 行（期望 1）：数据已变化或定位不唯一，未执行修改", rows.len());
                    tx.rollback().await?;
                    return Ok(receipt(plan, "conflict", 0, false, Some(message), "rolled_back"));
                }
                if plan.kind == "update" {
                    let wanted = conditions(kind, &plan.changes, &mut binds);
                    let sql = format!("SELECT 1 FROM {full} WHERE ({where_sql}) AND ({wanted}) LIMIT 2");
                    let mut q = crate::sql_history::query(&sql);
                    for value in &binds { q = $bind(q, value)?; }
                    if q.fetch_all(&mut *tx).await?.len() == 1 {
                        // A real no-op never fires UPDATE triggers or claims affected rows.
                        tx.rollback().await?;
                        return Ok(receipt(plan, "unchanged", 0, true, None, "rolled_back"));
                    }
                }
            }
            let mut post_keys = plan.keys.clone();
            let affected;
            if plan.kind == "insert" && $returning {
                let sql = format!("{} RETURNING *", plan.sql);
                let mut q = crate::sql_history::query(&sql);
                for v in &plan.binds { q = $bind(q, v)?; }
                let rows = q.fetch_all(&mut *tx).await?;
                affected = rows.len() as u64;
                if let Some(row) = rows.first() {
                    let names = row.columns().iter().map(|c| { use sqlx::Column; c.name().to_owned() }).collect::<Vec<_>>();
                    if post_keys.is_empty() { post_keys = returned_keys(plan, &names, &$values(row, names.len()))?; }
                }
            } else {
                let mut q = crate::sql_history::query(&plan.sql);
                for v in &plan.binds { q = $bind(q, v)?; }
                let result = q.execute(&mut *tx).await?;
                affected = result.rows_affected();
                if plan.kind == "insert" && $mysql {
                    // 同一事务连接上的 LAST_INSERT_ID，不从池中借另一连接。
                    let id: String = crate::sql_history::query_scalar("SELECT CAST(LAST_INSERT_ID() AS CHAR)").fetch_one(&mut *tx).await?;
                    post_keys = mysql_insert_keys(plan, id.parse::<u64>()?)?;
                }
            }
            if affected > plan.expected_rows || (plan.kind == "insert" && affected != 1) {
                tx.rollback().await?;
                return Ok(receipt(plan, "conflict", 0, false, Some(format!("影响行数 {affected}，期望 {}；已回滚", plan.expected_rows)), "rolled_back"));
            }
            for (name, value) in &mut post_keys {
                if let Some((_, changed)) = plan.changes.iter().find(|(n, _)| n == name) { *value = changed.clone(); }
            }
            let check = if plan.kind == "delete" { plan.keys.clone() } else { combine_pairs(&post_keys, &plan.changes)? };
            let mut binds = Vec::new();
            let where_sql = conditions(kind, &check, &mut binds);
            if where_sql.is_empty() { bail!("没有可验证的读回身份"); }
            let sql = format!("SELECT 1 FROM {full} WHERE {where_sql} LIMIT 2");
            let mut q = crate::sql_history::query(&sql);
            for v in &binds { q = $bind(q, v)?; }
            let rows = q.fetch_all(&mut *tx).await?;
            let verified = if plan.kind == "delete" { rows.is_empty() && affected == 1 } else { rows.len() == 1 };
            if !verified {
                tx.rollback().await?;
                return Ok(receipt(plan, "conflict", 0, false, Some("写入后原始目标值未通过读回校验；已回滚".into()), "rolled_back"));
            }
            tx.commit().await.context("MUTATION_EFFECTS_UNKNOWN：提交回执丢失，结果未知；禁止自动重放")?;
            Ok(receipt(plan, if affected == 0 { "unchanged" } else { "applied" }, affected, true, None, "committed"))
            }.await;
            if result.is_err() {
                owned.close_on_drop();
                owned.close().await.context("MUTATION_EFFECTS_UNKNOWN：写入物理连接清理未确认；禁止自动重放")?;
            } else {
                // The verified receipt is published only after physical return has completed.
                owned.return_to_pool().await;
            }
            result
        }};
    }
    match db {
        Db::Sqlite(pool) => run_atomic!(pool, bind_sqlite, sqlite_revision, crate::values::sqlite_row_values, true, false, false, validate_sqlite_query_source),
        Db::Mysql(pool) => run_atomic!(pool, bind_mysql, mysql_revision, crate::values::my_row_values, false, true, false, reject_mysql_query_source),
        Db::Postgres(pool) => run_atomic!(pool, bind_pg, pg_revision, crate::values::pg_row_values, true, false, true, reject_pg_query_source),
        _ => bail!("该数据源尚未接入可验证的事务型图形化写入；没有发送 SQL"),
    }

    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(kind: &str) -> MutationDraft {
        MutationDraft { connection_id: "c1".into(), owner_id: "page1".into(), schema: "main".into(), table: "items".into(), kind: kind.into(),
            changes: vec![("name".into(), DbValue::Text("a".into())), ("qty".into(), DbValue::Int(3))],
            keys: vec![("id".into(), DbValue::Int(7))], original_values: Vec::new() }
    }
    #[test] fn insert_uses_placeholders() {
        let (sql, binds, expected, _) = compile(&draft("insert"), &DbKind::of("sqlite"), "r").unwrap();
        assert_eq!(sql, "INSERT INTO \"items\" (\"name\", \"qty\") VALUES (?, ?)"); assert_eq!(binds.len(), 2); assert_eq!(expected, 1);
    }
    #[test] fn postgres_uses_numbered_placeholders_in_write_and_cas() {
        let (sql, binds, _, _) = compile(&draft("update"), &DbKind::of("postgresql"), "r").unwrap();
        assert_eq!(sql, "UPDATE \"main\".\"items\" SET \"name\" = $1, \"qty\" = $2 WHERE \"id\" = $3"); assert_eq!(binds.len(), 3);
    }
    #[test] fn update_binds_values_then_keys() {
        let (sql, binds, _, _) = compile(&draft("update"), &DbKind::of("sqlite"), "r").unwrap();
        assert_eq!(sql, "UPDATE \"items\" SET \"name\" = ?, \"qty\" = ? WHERE \"id\" = ?"); assert_eq!(binds[2], DbValue::Int(7));
    }
    #[test] fn delete_requires_keys() {
        let mut d = draft("delete"); d.keys.clear(); assert!(compile(&d, &DbKind::of("sqlite"), "r").is_err());
    }
    #[test] fn null_key_uses_is_null() {
        let mut d = draft("delete"); d.keys = vec![("id".into(), DbValue::Null)];
        let (sql, binds, _, _) = compile(&d, &DbKind::of("sqlite"), "r").unwrap(); assert!(sql.ends_with("\"id\" IS NULL")); assert!(binds.is_empty());
    }
    #[test] fn null_assignments_do_not_bind_text_null_to_numeric_pg_columns() {
        let mut d = draft("update"); d.changes = vec![("qty".into(), DbValue::Null)];
        let (sql, binds, _, _) = compile(&d, &DbKind::of("postgresql"), "r").unwrap(); assert!(sql.contains("SET \"qty\" = NULL")); assert!(sql.ends_with("\"id\" = $1")); assert_eq!(binds.len(), 1);
    }
    #[test] fn rejects_empty_changes() {
        let mut d = draft("update"); d.changes.clear(); assert!(compile(&d, &DbKind::of("sqlite"), "r").is_err());
    }
    #[test] fn rejects_duplicate_unknown_and_unsafe_values() {
        let mut d = draft("update"); d.changes.push(d.changes[0].clone()); assert!(compile(&d, &DbKind::of("sqlite"), "r").is_err());
        assert!(DbValue::Int(9_007_199_254_740_992).validate().is_err()); assert!(DbValue::Float(f64::NAN).validate().is_err());
        assert!(DbValue::Decimal("1e99999999".into()).validate().is_err()); assert!(DbValue::Decimal("NaN".into()).validate().is_err());
        assert!(DbValue::Binary("not binary".into()).validate().is_err());
    }
    #[test] fn exact_integer_decimal_binary_survive_ipc() {
        for value in [DbValue::Integer("9223372036854775807".into()), DbValue::Decimal("-0.000000000000123400".into()), DbValue::Binary("AP/+QQ==".into())] {
            assert_eq!(serde_json::from_str::<DbValue>(&serde_json::to_string(&value).unwrap()).unwrap(), value); value.validate().unwrap();
        }
    }
    #[test] fn mysql_revision_ignores_data_counter_not_literals() {
        assert_eq!(mysql_stable_ddl("CREATE TABLE `a` (`x` text DEFAULT ' AUTO_INCREMENT=9') AUTO_INCREMENT=12 ENGINE=InnoDB"),
            "CREATE TABLE `a` (`x` text DEFAULT ' AUTO_INCREMENT=9') ENGINE=InnoDB");
    }
    async fn database() -> Db {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE items(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, qty INTEGER, exact TEXT, bytes BLOB)").execute(&pool).await.unwrap();
        crate::sql_history::query("INSERT INTO items(id,name,qty) VALUES(7,'old',1)").execute(&pool).await.unwrap(); Db::Sqlite(pool)
    }
    async fn plan(db: &Db, draft: &MutationDraft) -> PlanInternal {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
        let kind = DbKind::of("sqlite");
        let metadata = crate::schema::table_schema(db, kind.clone(), "main", "items").await.unwrap();
        build_plan(draft, &kind, &metadata, table_revision(db,&kind,"main","items").await.unwrap(), "window1", "endpoint1".into()).unwrap()
    
    }).await
}
    async fn run(db: &Db, plan: &PlanInternal) -> MutationReceipt {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
        run_plan(db, &DbKind::of("sqlite"), plan, &table_revision(db,&DbKind::of("sqlite"),"main","items").await.unwrap()).await.unwrap()
    
    }).await
}
    async fn name(db: &Db, id: i64) -> Option<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
        let Db::Sqlite(pool) = db else { panic!() }; crate::sql_history::query_scalar("SELECT name FROM items WHERE id=?").bind(id).fetch_optional(pool).await.unwrap().flatten()
    
    }).await
}
    #[tokio::test] async fn sqlite_real_update_is_verified_and_committed() {
        let db = database().await; let p = plan(&db, &draft("update")).await; let r = run(&db, &p).await;
        assert!(r.verified); assert_eq!(r.transaction,"committed"); assert_eq!(name(&db,7).await.as_deref(),Some("a"));
    }
    #[tokio::test] async fn changed_primary_key_reads_back_new_identity() {
        let db = database().await; let mut d = draft("update"); d.changes.push(("id".into(), DbValue::Integer("9007199254740993".into())));
        let r = run(&db,&plan(&db,&d).await).await; assert!(r.verified); assert_eq!(name(&db,9_007_199_254_740_993).await.as_deref(),Some("a")); assert!(name(&db,7).await.is_none());
    }
    #[tokio::test] async fn stale_row_is_conflict_without_write() {
        let db = database().await; let mut d = draft("update"); d.original_values=vec![("name".into(),DbValue::Text("not old".into()))];
        let r=run(&db,&plan(&db,&d).await).await; assert_eq!(r.status,"conflict"); assert_eq!(r.affected,0); assert_eq!(name(&db,7).await.as_deref(),Some("old"));
    }
    #[tokio::test] async fn duplicate_match_update_and_delete_never_change_any_row() {
        for operation in ["update","delete"] {
            let db=database().await; let Db::Sqlite(pool)=&db else { panic!() };
            crate::sql_history::query("INSERT INTO items(id,name,qty) VALUES(8,'old',1)").execute(pool).await.unwrap();
            let mut d=draft(operation); d.keys=vec![("name".into(),DbValue::Text("old".into()))];
            let r=run(&db,&plan(&db,&d).await).await; assert_eq!(r.status,"conflict"); assert_eq!(r.affected,0);
            assert_eq!(name(&db,7).await.as_deref(),Some("old")); assert_eq!(name(&db,8).await.as_deref(),Some("old"));
        }
    }
    #[tokio::test] async fn trigger_changes_requested_value_rolls_back() {
        let db=database().await; let Db::Sqlite(pool)=&db else { panic!() };
        crate::sql_history::query("CREATE TRIGGER rewrite AFTER UPDATE ON items BEGIN UPDATE items SET name='changed by trigger' WHERE id=NEW.id; END").execute(pool).await.unwrap();
        let r=run(&db,&plan(&db,&draft("update")).await).await; assert_eq!(r.status,"conflict"); assert_eq!(r.transaction,"rolled_back"); assert_eq!(name(&db,7).await.as_deref(),Some("old"));
    }
    #[tokio::test] async fn schema_change_is_rejected_before_dml() {
        let db=database().await; let p=plan(&db,&draft("update")).await; let Db::Sqlite(pool)=&db else { panic!() };
        crate::sql_history::query("ALTER TABLE items ADD COLUMN extra TEXT DEFAULT 'x'").execute(pool).await.unwrap();
        let r=run(&db,&p).await; assert_eq!(r.status,"conflict"); assert_eq!(r.affected,0); assert_eq!(name(&db,7).await.as_deref(),Some("old"));
    }
    #[tokio::test] async fn insert_auto_id_exact_decimal_binary_and_injection_are_data() {
        let db=database().await; let mut d=draft("insert"); d.keys.clear();
        d.changes=vec![("name".into(),DbValue::Text("'; DROP TABLE items; --😀".into())), ("exact".into(),DbValue::Decimal("-0.000000000000123400".into())),("bytes".into(),DbValue::Binary("AP/+QQ==".into()))];
        let r=run(&db,&plan(&db,&d).await).await; assert!(r.verified); let Db::Sqlite(pool)=&db else { panic!() };
        let row=crate::sql_history::query("SELECT name,exact,bytes FROM items WHERE id=8").fetch_one(pool).await.unwrap();
        assert_eq!(row.try_get::<String,_>(0).unwrap(),"'; DROP TABLE items; --😀"); assert_eq!(row.try_get::<String,_>(1).unwrap(),"-0.000000000000123400"); assert_eq!(row.try_get::<Vec<u8>,_>(2).unwrap(),vec![0,255,254,65]);
    }
    #[tokio::test] async fn deletion_verifies_absence() {
        let db=database().await; let r=run(&db,&plan(&db,&draft("delete")).await).await; assert!(r.verified); assert_eq!(r.affected,1); assert!(name(&db,7).await.is_none());
    }
    #[tokio::test] async fn no_op_update_does_not_fire_an_update_trigger() {
        let db = database().await; let Db::Sqlite(pool) = &db else { panic!() };
        crate::sql_history::query("CREATE TABLE audit(changed INTEGER)").execute(pool).await.unwrap();
        crate::sql_history::query("CREATE TRIGGER audit_update AFTER UPDATE ON items BEGIN INSERT INTO audit VALUES(NEW.id); END").execute(pool).await.unwrap();
        let mut d = draft("update"); d.changes = vec![("name".into(), DbValue::Text("old".into())), ("qty".into(), DbValue::Integer("1".into()))];
        let r = run(&db, &plan(&db, &d).await).await;
        assert_eq!(r.status, "unchanged"); assert_eq!(r.affected, 0); assert!(r.verified);
        let count: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM audit").fetch_one(pool).await.unwrap();
        assert_eq!(count, 0, "logical no-op must not issue UPDATE or trigger a side effect");
    }
    #[tokio::test] async fn explicit_null_auto_identity_and_default_values_are_verified() {
        let db = database().await; let mut d = draft("insert"); d.keys.clear();
        d.changes = vec![("id".into(), DbValue::Null)];
        let p = plan(&db, &d).await;
        assert!(p.changes.is_empty()); assert!(p.sql.ends_with("DEFAULT VALUES"));
        let r = run(&db, &p).await;
        assert!(r.verified); assert_eq!(r.affected, 1);
        let Db::Sqlite(pool) = &db else { panic!() };
        let ids: Vec<i64> = crate::sql_history::query_scalar("SELECT id FROM items ORDER BY id").fetch_all(pool).await.unwrap();
        assert_eq!(ids, vec![7, 8]);
    }
    #[tokio::test] async fn ascii_utf8_and_null_bytes_are_still_binary_and_cas_is_lossless() {
        let db = database().await; let Db::Sqlite(pool) = &db else { panic!() };
        for bytes in [b"ASCII".to_vec(), "中文😀".as_bytes().to_vec(), vec![0, 65, 0, 255], Vec::new()] {
            crate::sql_history::query("UPDATE items SET bytes=? WHERE id=7").bind(&bytes).execute(pool).await.unwrap();
            let row = crate::sql_history::query("SELECT bytes FROM items WHERE id=7").fetch_one(pool).await.unwrap();
            let display = crate::values::sqlite_row_values(&row, 1);
            assert_eq!(display[0].as_deref(), Some(crate::values::format_binary(&bytes).as_str()));
            let mut d = draft("update"); d.original_values = vec![("bytes".into(), DbValue::Binary(STANDARD.encode(&bytes)))];
            let r = run(&db, &plan(&db, &d).await).await; assert!(r.verified);
        }
    }
    #[tokio::test] async fn unknown_columns_and_lossy_sqlite_decimal_affinity_are_rejected() {
        let db = database().await;
        let metadata = crate::schema::table_schema(&db, DbKind::of("sqlite"), "main", "items").await.unwrap();
        let mut d = draft("update"); d.changes = vec![("missing".into(), DbValue::Text("x".into()))];
        assert!(build_plan(&d, &DbKind::of("sqlite"), &metadata, "r".into(), "window1", "endpoint1".into()).is_err());
        d.changes = vec![("qty".into(), DbValue::Decimal("9007199254740993.01".into()))];
        assert!(build_plan(&d, &DbKind::of("sqlite"), &metadata, "r".into(), "window1", "endpoint1".into()).is_err());
    }
    #[test] fn float4_normalization_and_date_validation_are_explicit() {
        let mut column = crate::models::ColumnInfo { name: "f".into(), data_type: "real".into(), nullable: true, default: None,
            primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None };
        assert_eq!(normalize_value(&DbKind::of("postgresql"), &column, &DbValue::Float(0.1)).unwrap(), DbValue::Float(f64::from(0.1_f32)));
        assert!(normalize_value(&DbKind::of("postgresql"), &column, &DbValue::Float(f64::MAX)).is_err());
        column.data_type = "date".into();
        assert!(normalize_value(&DbKind::of("postgresql"), &column, &DbValue::Date("2026-02-29".into())).is_err());
        assert!(normalize_value(&DbKind::of("postgresql"), &column, &DbValue::Date("2024-02-29".into())).is_ok());
    }
    #[test] fn json_big_numbers_and_utf8_revision_literals_are_not_rounded_or_folded() {
        let json = DbValue::Json("{\"exact\":1234567890123456789012345678901234567890,\"text\":\"中文😀\"}".into());
        json.validate().unwrap();
        assert_eq!(serde_json::from_str::<DbValue>(&serde_json::to_string(&json).unwrap()).unwrap(), json);
        assert_eq!(mysql_stable_ddl("CREATE TABLE `表` (`说明` text DEFAULT ' 中文😀 AUTO_INCREMENT=99') AUTO_INCREMENT=12 ENGINE=InnoDB"),
            "CREATE TABLE `表` (`说明` text DEFAULT ' 中文😀 AUTO_INCREMENT=99') ENGINE=InnoDB");
    }
    #[tokio::test] async fn registry_enforces_total_byte_budget_and_expires_old_plans() {
        let db = database().await; let reg = Registry::default();
        let mut p = plan(&db, &draft("delete")).await;
        let text = DbValue::Text("x".repeat(3 * 1024 * 1024));
        p.changes = vec![("name".into(), text.clone())]; p.binds = vec![text];
        let first = reg.put(p.clone()).unwrap();
        for _ in 0..11 { reg.put(p.clone()).unwrap(); }
        {
            let entries = reg.inner.lock();
            assert!(entries.values().map(|entry| entry.bytes).sum::<usize>() <= PLAN_CACHE_BYTES);
            assert!(!entries.contains_key(&first), "oldest plans must be evicted before exhausting the cache");
        }
        let last = reg.put(plan(&db, &draft("delete")).await).unwrap();
        reg.inner.lock().get_mut(&last).unwrap().created = Instant::now() - (PLAN_TTL + Duration::from_secs(1));
        assert!(reg.take(&last, "window1", "page1").is_err());
    }
    #[tokio::test] async fn registry_takes_once_and_rejects_other_owner_without_consuming() {
        let db=database().await; let reg=Registry::default(); let id=reg.put(plan(&db,&draft("delete")).await).unwrap();
        assert!(reg.take(&id,"window2","page1").is_err()); assert!(reg.take(&id,"window1","page2").is_err());
        assert!(reg.take(&id,"window1","page1").is_ok()); assert!(reg.take(&id,"window1","page1").is_err());
    }
}
