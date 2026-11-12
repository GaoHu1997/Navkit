//! JDBC/H2 的 C2 驱动桥。没有第二套授权/计划状态机，也不接受客户端 SQL。
//! 调用方必须持 activity/ordinary lease，用有效保存配置校验只读和端点指纹，
//! 并使用 mutation::Registry 的窗口/页面一次性授权。此模块只消费真实驱动证据。

use std::collections::{BTreeSet, HashSet};

use anyhow::{anyhow, Result};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::http_db::{relational::{self, JdbcCapabilities, JdbcColumnMetadata, JdbcMetadataSnapshot,
    JdbcMutationOutcome, JdbcObjectId, JdbcRowPlan}, HttpDb};
use super::{DbValue, MutationDraft, MutationReceipt, PlanInternal};

type Pairs = Vec<(String, DbValue)>;
const TYPES: [&str; 12] = ["null", "text", "integer", "decimal", "float", "bool", "binary",
    "date", "time", "timestamp", "timestamptz", "uuid"];

/// 与原生计划共用同一 Registry 和完整序列化字节预算；客户端不能构造该绑定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JdbcPlanBinding {
    pub capabilities: JdbcCapabilities,
    pub metadata: JdbcMetadataSnapshot,
}

fn fail<T>(code: &str, message: &str) -> Result<T> {
    Err(AppError::coded(code, message).into())
}
fn unknown<T>(operation: &str, message: &str) -> Result<T> {
    Err(AppError::coded("MUTATION_EFFECTS_UNKNOWN", message).operation(operation).into())
}
fn metadata_error(_: anyhow::Error) -> anyhow::Error {
    AppError::coded("MUTATION_METADATA_UNKNOWN", "无法取得完整的真实驱动结构元数据；没有登记或发送写入").into()
}
fn context_error(_: anyhow::Error) -> anyhow::Error {
    AppError::coded("MUTATION_CONTEXT_CHANGED", "写入计划的驱动会话、能力或结构已失效；没有发送 DML").into()
}
fn value_error(_: anyhow::Error) -> anyhow::Error {
    AppError::coded("MUTATION_VALUE_INVALID", "绑定值类型、格式、范围或精度与真实列不一致；没有发送 DML").into()
}
fn capabilities_valid(caps: &JdbcCapabilities) -> Result<()> {
    relational::validate_result("capabilities", &json!({}),
        &json!({"session_id": caps.session_id, "result": caps})).map_err(metadata_error)?;
    if caps.read_only {
        return fail("READ_ONLY_VIOLATION", "真实物理连接为只读连接，不能生成图形化写入计划");
    }
    let types: BTreeSet<_> = caps.bound_types.iter().map(String::as_str).collect();
    if caps.protocol_version != 2 || !matches!(caps.adapter_id.as_str(), "jdbc" | "native-h2")
        || caps.product != "H2" || caps.product_version != "2.3.232 (2024-08-11)"
        || !caps.driver_name.starts_with("H2 JDBC Driver") || !caps.driver_version.starts_with("2.3.232")
        || !caps.transactions || !caps.metadata || !caps.row_write || caps.ddl_write
        || caps.catalog.is_empty() || types != TYPES.into_iter().collect::<BTreeSet<_>>() {
        // REGULAR 是 Java 对真实 INFORMATION_SCHEMA.SETTINGS 的查询结果，不能靠名称猜测。
        return fail("UNSUPPORTED_MUTATION", "该实际产品、驱动版本或模式未通过类型化事务写入认证；没有生成计划");
    }
    Ok(())
}
fn same_capabilities(left: &JdbcCapabilities, right: &JdbcCapabilities) -> bool {
    // schema 是当前物理连接的临时 namespace，而非 qualified 对象的不可变身份。
    left.protocol_version == right.protocol_version && left.session_id == right.session_id
        && left.adapter_id == right.adapter_id && left.product == right.product
        && left.product_version == right.product_version && left.driver_name == right.driver_name
        && left.driver_version == right.driver_version && left.catalog == right.catalog
        && left.read_only == right.read_only && left.transactions == right.transactions
        && left.metadata == right.metadata && left.row_write == right.row_write
        && left.ddl_write == right.ddl_write && left.bound_types == right.bound_types
}
fn snapshot_valid(caps: &JdbcCapabilities, metadata: &JdbcMetadataSnapshot, object: &JdbcObjectId) -> Result<()> {
    relational::validate_result("metadata", &json!({"object": object}),
        &json!({"session_id": caps.session_id, "result": metadata})).map_err(metadata_error)?;
    if metadata.object != *object || metadata.session_id != caps.session_id || object.catalog != caps.catalog {
        return fail("MUTATION_CONTEXT_CHANGED", "结构元数据不属于绑定的真实 catalog/会话/对象");
    }
    if !matches!(metadata.table_type.as_str(), "BASE TABLE" | "TABLE") {
        return fail("UNSUPPORTED_MUTATION", "视图不能作为具有真实行身份的基表编辑");
    }
    Ok(())
}
fn same_snapshot(left: &JdbcMetadataSnapshot, right: &JdbcMetadataSnapshot) -> Result<bool> {
    Ok(left.object == right.object && left.session_id == right.session_id
        && left.backend_object_id == right.backend_object_id && left.revision == right.revision
        && serde_json::to_value(left)? == serde_json::to_value(right)?)
}
fn object_id(caps: &JdbcCapabilities, draft: &MutationDraft) -> Result<JdbcObjectId> {
    if [&caps.catalog, &draft.schema, &draft.table].into_iter().any(|name|
        name.is_empty() || name.contains('\0') || name.encode_utf16().count() > 1024) {
        return fail("MUTATION_CONTEXT_CHANGED", "JDBC 写入必须指定明确的 schema/table；catalog 只取自真实物理连接");
    }
    Ok(JdbcObjectId { catalog: caps.catalog.clone(), schema: draft.schema.clone(), name: draft.table.clone() })
}

/// 驱动只读预览：能力、两份完整元数据和再次能力都必须属于同一实际会话。
/// fingerprint 来自调用方服务器配置绑定，而非 MutationDraft 或驱动自报。
pub(crate) async fn preview(http: &HttpDb, draft: &MutationDraft, principal: &str,
    fingerprint: String) -> Result<PlanInternal> {
    let caps = http.relational_capabilities().await.map_err(|_| anyhow!(AppError::coded(
        "UNSUPPORTED_MUTATION", "该适配器没有真实的类型化关系写入能力；没有生成计划")))?;
    capabilities_valid(&caps)?;
    let object = object_id(&caps, draft)?;
    let before = http.relational_metadata(&object).await.map_err(metadata_error)?;
    snapshot_valid(&caps, &before, &object)?;
    let after = http.relational_metadata(&object).await.map_err(metadata_error)?;
    snapshot_valid(&caps, &after, &object)?;
    let final_caps = http.relational_capabilities().await.map_err(context_error)?;
    capabilities_valid(&final_caps)?;
    if !same_capabilities(&caps, &final_caps) || !same_snapshot(&before, &after)? {
        return fail("MUTATION_CONTEXT_CHANGED", "预览期间实际会话或完整结构版本已变化；没有生成计划");
    }
    build_preview(draft, caps, before, principal, fingerprint)
}

fn column<'a>(metadata: &'a JdbcMetadataSnapshot, name: &str) -> Result<&'a JdbcColumnMetadata> {
    metadata.columns.iter().find(|column| column.name == name)
        .ok_or_else(|| AppError::coded("MUTATION_METADATA_UNKNOWN", "绑定列不存在于完整的真实元数据").into())
}
fn time_text(time: chrono::NaiveTime) -> String {
    let mut text = format!("{:02}:{:02}", time.hour(), time.minute());
    if time.second() != 0 || time.nanosecond() != 0 {
        text.push_str(&format!(":{:02}", time.second()));
        let nano = time.nanosecond();
        if nano != 0 {
            let (digits, divisor) = if nano % 1_000_000 == 0 { (3, 1_000_000) }
                else if nano % 1000 == 0 { (6, 1000) } else { (9, 1) };
            text.push_str(&format!(".{:0width$}", nano / divisor, width = digits));
        }
    }
    text
}
fn check_time(column: &JdbcColumnMetadata, time: chrono::NaiveTime) -> Result<()> {
    if !(0..=9).contains(&column.scale) || time.nanosecond() >= 1_000_000_000
        || time.nanosecond() % 10_u32.pow((9 - column.scale) as u32) != 0 {
        return fail("MUTATION_VALUE_INVALID", "时间的小数精度超过实际列精度，禁止舍入");
    }
    Ok(())
}
fn local_date(date: chrono::NaiveDate) -> Result<String> {
    if !(0..=9999).contains(&date.year()) {
        return fail("UNSUPPORTED_MUTATION_TYPE", "扩展纪年日期没有通过当前驱动无损读写认证");
    }
    Ok(date.format("%Y-%m-%d").to_string())
}
fn local_time(raw: &str) -> Result<chrono::NaiveTime> {
    if raw.len() > 32 { return fail("MUTATION_VALUE_INVALID", "时间文字过长"); }
    chrono::NaiveTime::parse_from_str(raw, "%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveTime::parse_from_str(raw, "%H:%M")).map_err(|error| value_error(error.into()))
}
fn local_timestamp(raw: &str) -> Result<chrono::NaiveDateTime> {
    if raw.len() > 64 { return fail("MUTATION_VALUE_INVALID", "时间戳文字过长"); }
    let raw = raw.replace(' ', "T");
    chrono::NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M"))
        .map_err(|error| value_error(error.into()))
}
fn decimal_text(column: &JdbcColumnMetadata, raw: &str) -> Result<String> {
    if !(0..=1000).contains(&column.scale) || column.precision == 0 || column.precision > 1000 {
        return fail("UNSUPPORTED_MUTATION_TYPE", "该十进制列精度未通过有界无损绑定认证");
    }
    let number = super::decimal_value(raw).map_err(value_error)?;
    let scaled = number.with_scale(column.scale as i64);
    if number != scaled { return fail("MUTATION_VALUE_INVALID", "十进制值超过实际 scale，禁止舍入"); }
    let (coefficient, scale) = scaled.as_bigint_and_exponent();
    let digits = coefficient.to_string();
    let (sign, digits) = if let Some(digits) = digits.strip_prefix('-') { ("-", digits) } else { ("", digits.as_str()) };
    if digits.len() > column.precision as usize || scale != column.scale as i64 {
        return fail("MUTATION_VALUE_INVALID", "十进制值超过实际 precision");
    }
    let scale = column.scale as usize;
    let text = if scale == 0 { digits.to_string() }
        else if digits.len() <= scale { format!("0.{}{}", "0".repeat(scale - digits.len()), digits) }
        else { format!("{}.{}", &digits[..digits.len() - scale], &digits[digits.len() - scale..]) };
    Ok(format!("{sign}{text}"))
}
fn normalize_value(column: &JdbcColumnMetadata, value: &DbValue) -> Result<DbValue> {
    if !column.writable_type {
        return fail("UNSUPPORTED_MUTATION_TYPE", "目标列类型未通过无损 PreparedStatement 绑定认证（NULL 也不能绕过）");
    }
    if serde_json::to_vec(value)?.len() > super::VALUE_LIMIT {
        return fail("MUTATION_VALUE_INVALID", "单个绑定值超过 16 MiB");
    }
    let supported = matches!(column.jdbc_type, -6 | 5 | 4 | -5 | 2 | 3 | 7 | 6 | 8 | -7 | 16 | 1 | 12 | -15 | -9 | -2 | -3 | 91 | 92 | 93 | 2014)
        || column.type_name == "UUID";
    if !supported { return fail("UNSUPPORTED_MUTATION_TYPE", "该 JDBC 类型没有经过当前驱动绑定与读回认证"); }
    if value.is_null() { return Ok(DbValue::Null); }
    if column.type_name == "UUID" {
        return if let DbValue::Uuid(raw) = value {
            Ok(DbValue::Uuid(uuid::Uuid::parse_str(raw).map_err(|error| value_error(error.into()))?.hyphenated().to_string()))
        } else { fail("MUTATION_VALUE_INVALID", "UUID 列必须使用 uuid 文字标签") };
    }
    match (column.jdbc_type, value) {
        (-6 | 5 | 4 | -5, DbValue::Integer(_) | DbValue::Int(_)) => {
            value.validate().map_err(value_error)?;
            let number = match value { DbValue::Int(number) => *number,
                DbValue::Integer(raw) => raw.parse::<i64>().map_err(|error| value_error(error.into()))?, _ => unreachable!() };
            if column.jdbc_type == -6 && i8::try_from(number).is_err()
                || column.jdbc_type == 5 && i16::try_from(number).is_err()
                || column.jdbc_type == 4 && i32::try_from(number).is_err() {
                return fail("MUTATION_VALUE_INVALID", "整数超出目标 JDBC 列范围");
            }
            Ok(DbValue::Integer(number.to_string()))
        }
        (2 | 3, DbValue::Decimal(raw)) => Ok(DbValue::Decimal(decimal_text(column, raw)?)),
        (7 | 6 | 8, DbValue::Float(number)) => {
            if !number.is_finite() || column.jdbc_type == 7 && f64::from(*number as f32) != *number {
                return fail("MUTATION_VALUE_INVALID", "浮点值非法或不能在实际列中精确表达");
            }
            Ok(value.clone())
        }
        (-7 | 16, DbValue::Bool(_)) => Ok(value.clone()),
        (1 | 12 | -15 | -9, DbValue::Text(raw)) => {
            if raw.encode_utf16().count() > column.precision as usize {
                return fail("MUTATION_VALUE_INVALID", "文字超过实际列长度");
            }
            Ok(value.clone())
        }
        (-2 | -3, DbValue::Binary(raw)) => {
            if super::binary_value(raw).map_err(value_error)?.len() > column.precision as usize {
                return fail("MUTATION_VALUE_INVALID", "二进制值超过实际列长度");
            }
            Ok(value.clone())
        }
        (91, DbValue::Date(raw)) => Ok(DbValue::Date(local_date(super::date_value(raw).map_err(value_error)?)?)),
        (92, DbValue::Time(raw)) => { let time = local_time(raw)?; check_time(column, time)?; Ok(DbValue::Time(time_text(time))) }
        (93, DbValue::Timestamp(raw)) => {
            let time = local_timestamp(raw)?; check_time(column, time.time())?;
            Ok(DbValue::Timestamp(format!("{}T{}", local_date(time.date())?, time_text(time.time()))))
        }
        (2014, DbValue::Timestamptz(raw)) => {
            if raw.len() > 80 { return fail("MUTATION_VALUE_INVALID", "时区时间戳文字过长"); }
            let raw = raw.replace(' ', "T");
            // OffsetDateTime.toString() emits minute-only ISO values when seconds
            // and nanos are zero, while chrono RFC3339 requires an explicit second.
            // Restore only that omitted zero, preserving date, nanoseconds and offset.
            let raw = if raw.as_bytes().get(16).is_some_and(|byte| matches!(byte, b'Z' | b'+' | b'-'))
                && raw.is_char_boundary(16) { format!("{}:00{}", &raw[..16], &raw[16..]) } else { raw };
            let time = chrono::DateTime::parse_from_rfc3339(&raw).map_err(|error| value_error(error.into()))?;
            check_time(column, time.time())?;
            let seconds = time.offset().local_minus_utc();
            if seconds.unsigned_abs() > 18 * 3600 || seconds % 60 != 0 {
                return fail("UNSUPPORTED_MUTATION_TYPE", "该时区偏移未通过当前驱动无损认证");
            }
            let offset = if seconds == 0 { "Z".to_string() } else {
                format!("{}{:02}:{:02}", if seconds < 0 { '-' } else { '+' }, seconds.unsigned_abs() / 3600, seconds.unsigned_abs() % 3600 / 60)
            };
            Ok(DbValue::Timestamptz(format!("{}T{}{offset}", local_date(time.date_naive())?, time_text(time.time()))))
        }
        _ => fail("MUTATION_VALUE_INVALID", "绑定标签与实际 JDBC 列类型不一致，禁止文字或 Number 猜测转换"),
    }
}
fn normalize_pairs(metadata: &JdbcMetadataSnapshot, pairs: &Pairs) -> Result<Pairs> {
    if pairs.len() > 1024 { return fail("MUTATION_VALUE_INVALID", "绑定列超过有界计划上限"); }
    let mut names = HashSet::new();
    pairs.iter().map(|(name, value)| {
        if !names.insert(name) { return fail("MUTATION_VALUE_INVALID", "绑定列重复"); }
        Ok((name.clone(), normalize_value(column(metadata, name)?, value)?))
    }).collect()
}
fn candidate_key(metadata: &JdbcMetadataSnapshot, values: &Pairs) -> Option<Pairs> {
    metadata.unique_keys.iter().find_map(|key| {
        key.iter().map(|name| values.iter().find(|(column, value)| column == name && !value.is_null()).cloned())
            .collect::<Option<Pairs>>()
    })
}
fn overwrite(values: &Pairs, changes: &Pairs) -> Pairs {
    let mut values = values.clone();
    for (name, value) in changes {
        if let Some((_, old)) = values.iter_mut().find(|(column, _)| column == name) { *old = value.clone(); }
        else { values.push((name.clone(), value.clone())); }
    }
    values
}
fn writable(column: &JdbcColumnMetadata) -> bool {
    !column.read_only && column.generated == Some(false) && column.auto_increment == Some(false)
}
fn quote(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
fn display_sql(object: &JdbcObjectId, kind: &str, changes: &Pairs, conditions: &Pairs) -> Result<(String, Vec<DbValue>)> {
    let table = format!("{}.{}", quote(&object.schema), quote(&object.name));
    let mut binds = Vec::new();
    let mut predicate = Vec::new();
    for (name, value) in conditions {
        if value.is_null() { predicate.push(format!("{} IS NULL", quote(name))); }
        else { predicate.push(format!("{} = ?", quote(name))); }
    }
    let sql = match kind {
        "insert" if changes.is_empty() => format!("INSERT INTO {table} DEFAULT VALUES"),
        "insert" => {
            binds.extend(changes.iter().map(|(_, value)| value.clone()));
            format!("INSERT INTO {table} ({}) VALUES ({})", changes.iter().map(|(name, _)| quote(name)).collect::<Vec<_>>().join(","), vec!["?"; changes.len()].join(","))
        }
        "update" => {
            binds.extend(changes.iter().map(|(_, value)| value.clone()));
            format!("UPDATE {table} SET {} WHERE {}", changes.iter().map(|(name, _)| format!("{} = ?", quote(name))).collect::<Vec<_>>().join(","), predicate.join(" AND "))
        }
        "delete" => format!("DELETE FROM {table} WHERE {}", predicate.join(" AND ")),
        _ => return fail("UNSUPPORTED_MUTATION", "只允许服务端编译的单行 insert/update/delete"),
    };
    if kind != "insert" { binds.extend(conditions.iter().filter(|(_, value)| !value.is_null()).map(|(_, value)| value.clone())); }
    Ok((sql, binds))
}
fn build_preview(draft: &MutationDraft, caps: JdbcCapabilities, metadata: JdbcMetadataSnapshot,
    principal: &str, fingerprint: String) -> Result<PlanInternal> {
    if principal.is_empty() || draft.owner_id.trim().is_empty() || fingerprint.is_empty() {
        return fail("MUTATION_CONTEXT_CHANGED", "窗口、页面或服务器端点授权绑定为空");
    }
    capabilities_valid(&caps)?;
    snapshot_valid(&caps, &metadata, &object_id(&caps, draft)?)?;
    if !matches!(draft.kind.as_str(), "insert" | "update" | "delete") {
        return fail("UNSUPPORTED_MUTATION", "不支持此类图形化写入");
    }
    let keys = normalize_pairs(&metadata, &draft.keys)?;
    let original = normalize_pairs(&metadata, &draft.original_values)?;
    let mut changes = Vec::new();
    let mut seen = HashSet::new();
    if draft.changes.len() > 1024 { return fail("MUTATION_VALUE_INVALID", "变化列超过上限"); }
    for (name, value) in &draft.changes {
        if !seen.insert(name) { return fail("MUTATION_VALUE_INVALID", "变化列重复"); }
        let column = column(&metadata, name)?;
        if draft.kind == "insert" && column.auto_increment == Some(true) && value.is_null() { continue; }
        if !writable(column) { return fail("READ_ONLY_COLUMN", "真实 generated/identity/read-only 或未知可写性列不可赋值"); }
        let value = normalize_value(column, value)?;
        if value.is_null() && column.nullable != Some(true) {
            return fail("MUTATION_VALUE_INVALID", "非空或未知可空性列不能赋值 NULL");
        }
        changes.push((name.clone(), value));
    }
    let mut conditions = super::combine_pairs(&keys, &original).map_err(|_| anyhow!(AppError::coded(
        "MUTATION_VALUE_INVALID", "同一原始列有冲突的旧值")))?;
    let selected_key;
    if draft.kind == "insert" {
        if !conditions.is_empty() { return fail("MUTATION_VALUE_INVALID", "新增行不能继承旧行 keys/original_values"); }
        let generated_primary = !metadata.primary_key.is_empty() && metadata.primary_key.iter().all(|name| {
            changes.iter().any(|(column, value)| column == name && !value.is_null())
                || metadata.columns.iter().any(|column| &column.name == name && column.writable_type
                    && (column.auto_increment == Some(true) || column.default_value.is_some()))
        });
        if metadata.unique_keys.is_empty() || candidate_key(&metadata, &changes).is_none() && !generated_primary {
            return fail("MUTATION_MISSING_UNIQUE_KEY", "新增行没有完整真实唯一键或同连接可读回的生成主键");
        }
        for column in &metadata.columns {
            if writable(column) && column.nullable == Some(false) && column.default_value.is_none()
                && !changes.iter().any(|(name, _)| name == &column.name) {
                return fail("MUTATION_VALUE_INVALID", "新增行缺少真实 NOT NULL 且无默认值的列");
            }
        }
        selected_key = Vec::new();
    } else {
        if draft.kind == "update" && changes.is_empty() {
            return fail("MUTATION_VALUE_INVALID", "空更新不是经过锁定旧行校验的 no-op");
        }
        if draft.kind == "delete" && !changes.is_empty() {
            return fail("MUTATION_VALUE_INVALID", "删除行不能包含新值 changes");
        }
        selected_key = candidate_key(&metadata, &conditions).ok_or_else(|| anyhow!(AppError::coded(
            "MUTATION_MISSING_UNIQUE_KEY", "必须提供真实完整且非 NULL 的主键或候选唯一键")))?;
        if draft.kind == "update" {
            if changes.iter().any(|(name, _)| !conditions.iter().any(|(column, _)| column == name)) {
                return fail("MUTATION_BASELINE_REQUIRED", "每个更新列必须提供加载时旧值，不能执行时读取当前值替代基线");
            }
            if candidate_key(&metadata, &overwrite(&conditions, &changes)).is_none() {
                return fail("MUTATION_MISSING_UNIQUE_KEY", "更新后没有已验证的完整非 NULL 唯一键供同事务读回");
            }
        } else {
            if metadata.columns.iter().any(|column| !column.writable_type) {
                return fail("UNSUPPORTED_MUTATION_TYPE", "删除不能对含未认证可比较类型的整行伪造完整 CAS");
            }
            if metadata.columns.iter().any(|column| !conditions.iter().any(|(name, _)| name == &column.name)) {
                return fail("MUTATION_BASELINE_REQUIRED", "删除必须提供加载时整行可比较旧值；没有发送 DML");
            }
        }
        // Java 的 keys 是真实候选键，其余旧值均保留在原始 CAS 条件中。
        conditions = super::combine_pairs(&selected_key, &conditions)?;
    }
    let (sql, binds) = display_sql(&metadata.object, &draft.kind, &changes, &conditions)?;
    let plan = PlanInternal { connection_id: draft.connection_id.clone(), principal: principal.into(), owner_id: draft.owner_id.clone(),
        endpoint_fingerprint: fingerprint, schema: metadata.object.schema.clone(), table: metadata.object.name.clone(), kind: draft.kind.clone(),
        sql, binds, base_revision: metadata.revision.clone(), expected_rows: 1, keys: selected_key,
        conditions, changes, columns: Vec::new(), unique_keys: metadata.unique_keys.clone(),
        jdbc: Some(JdbcPlanBinding { capabilities: caps, metadata }), query_source: None };
    if serde_json::to_vec(&plan)?.len() > super::PLAN_LIMIT {
        return fail("MUTATION_VALUE_INVALID", "含完整结构与重复绑定的服务端计划超过 32 MiB，未登记");
    }
    Ok(plan)
}

/// 每次 await 后调用方仍需检查应用保存配置/实际端点绑定。这里重校物理驱动证据。
pub(crate) async fn validate_context(http: &HttpDb, plan: &PlanInternal) -> Result<()> {
    let binding = plan.jdbc.as_ref().ok_or_else(|| anyhow!(AppError::coded("MUTATION_CONTEXT_CHANGED", "计划没有真实 JDBC 绑定")))?;
    let caps = http.relational_capabilities().await.map_err(context_error)?;
    capabilities_valid(&caps).map_err(context_error)?;
    if !same_capabilities(&binding.capabilities, &caps) {
        return fail("MUTATION_CONTEXT_CHANGED", "实际物理会话、catalog、驱动或有效能力已切换，没有发送 DML");
    }
    let metadata = http.relational_metadata(&binding.metadata.object).await.map_err(context_error)?;
    snapshot_valid(&caps, &metadata, &binding.metadata.object).map_err(context_error)?;
    if !same_snapshot(&binding.metadata, &metadata)? || plan.base_revision != metadata.revision {
        return fail("MUTATION_CONTEXT_CHANGED", "同名对象身份或完整结构版本已改变，没有发送 DML");
    }
    let after = http.relational_capabilities().await.map_err(context_error)?;
    capabilities_valid(&after).map_err(context_error)?;
    if !same_capabilities(&caps, &after) { return fail("MUTATION_CONTEXT_CHANGED", "排队期间实际驱动会话或能力已变化，没有发送 DML"); }
    Ok(())
}
fn operation(plan_id: &str, plan: &PlanInternal, effective_read_only: bool, timeout_secs: u64) -> Result<JdbcRowPlan> {
    let binding = plan.jdbc.as_ref().ok_or_else(|| anyhow!(AppError::coded("MUTATION_CONTEXT_CHANGED", "无 JDBC 计划绑定")))?;
    if uuid::Uuid::parse_str(plan_id).is_err() { return fail("MUTATION_CONTEXT_CHANGED", "操作 ID 必须来自一次性服务端 Registry"); }
    if effective_read_only { return fail("READ_ONLY_VIOLATION", "保存的有效连接为只读，没有发送 DML"); }
    if !(1..=300).contains(&timeout_secs) { return fail("UNSUPPORTED_MUTATION", "该驱动图形化事务预算必须为 1..300 秒"); }
    let old = plan.conditions.iter().filter(|(name, _)| !plan.keys.iter().any(|(key, _)| key == name)).cloned().collect();
    Ok(JdbcRowPlan { operation_id: plan_id.into(), session_id: binding.capabilities.session_id.clone(), object: binding.metadata.object.clone(),
        base_revision: plan.base_revision.clone(), kind: plan.kind.clone(), keys: plan.keys.clone(), original_values: old,
        changes: plan.changes.clone(), read_only: effective_read_only, timeout_secs })
}
fn normalize_readback(metadata: &JdbcMetadataSnapshot, pairs: &Pairs) -> Result<Pairs> {
    let values = normalize_pairs(metadata, pairs)?;
    if values != *pairs { return fail("MUTATION_VALUE_INVALID", "驱动读回不是实际列的规范无损值"); }
    Ok(values)
}
fn map_outcome(plan_id: &str, plan: &PlanInternal, operation: &JdbcRowPlan, outcome: JdbcMutationOutcome) -> Result<MutationReceipt> {
    if outcome.connection_poisoned || outcome.status == "unknown" || outcome.affected_rows.is_none() {
        return unknown(plan_id, "事务回执未知或物理连接已隔离；禁止自动重放，请在新连接中核对结果");
    }
    relational::validate_result("row_plan", &serde_json::to_value(operation)?,
        &json!({"session_id": operation.session_id, "result": outcome})).map_err(|_| anyhow!(AppError::coded(
            "MUTATION_EFFECTS_UNKNOWN", "驱动返回的事务证据矛盾或不完整；禁止自动重放").operation(plan_id)))?;
    let binding = plan.jdbc.as_ref().ok_or_else(|| anyhow!(AppError::coded("MUTATION_CONTEXT_CHANGED", "丢失驱动证据绑定")))?;
    if matches!(outcome.status.as_str(), "applied" | "unchanged") {
        let readback = normalize_readback(&binding.metadata, &outcome.readback).map_err(|_| anyhow!(AppError::coded(
            "MUTATION_EFFECTS_UNKNOWN", "已执行操作的类型化读回证据失效，禁止自动重放").operation(plan_id)))?;
        let generated = normalize_readback(&binding.metadata, &outcome.generated_keys).map_err(|_| anyhow!(AppError::coded(
            "MUTATION_EFFECTS_UNKNOWN", "生成键类型化证据失效，禁止自动重放").operation(plan_id)))?;
        if plan.kind != "delete" {
            if readback.len() != binding.metadata.columns.iter().filter(|column| column.writable_type).count()
                || plan.changes.iter().any(|(name, value)| !readback.iter().any(|(column, actual)| column == name && actual == value))
                || generated.iter().any(|(name, value)| !binding.metadata.primary_key.contains(name)
                    || !readback.iter().any(|(column, actual)| column == name && actual == value)) {
                return unknown(plan_id, "已执行操作的目标值、生成键或完整读回证据不一致，禁止自动重放");
            }
            let expected = if plan.kind == "insert" { plan.changes.clone() } else { overwrite(&plan.conditions, &plan.changes) };
            let expected = super::combine_pairs(&expected, &generated).map_err(|_| anyhow!(AppError::coded(
                "MUTATION_EFFECTS_UNKNOWN", "生成键与计划目标值冲突，禁止自动重放").operation(plan_id)))?;
            let key = candidate_key(&binding.metadata, &expected).ok_or_else(|| anyhow!(AppError::coded(
                "MUTATION_EFFECTS_UNKNOWN", "缺少与当前结果关联的真实目标唯一键，禁止自动重放").operation(plan_id)))?;
            if key.iter().any(|(name, value)| !readback.iter().any(|(column, actual)| column == name && actual == value)) {
                return unknown(plan_id, "读回行不是计划的新主键/唯一键目标，禁止自动重放");
            }
            if outcome.status == "unchanged" && plan.conditions.iter().any(|(name, value)|
                !readback.iter().any(|(column, actual)| column == name && actual == value)) {
                return unknown(plan_id, "no-op 没有提供锁定旧行的精确 CAS 证据，禁止自动重放");
            }
        }
    }
    let affected = match outcome.affected_rows.as_deref() { Some("0") => 0, Some("1") => 1,
        _ => return unknown(plan_id, "影响行数未能精确确认，禁止自动重放") };
    Ok(MutationReceipt { plan_id: plan_id.into(), status: outcome.status, affected, verified: outcome.verified,
        mismatches: outcome.mismatches, sql: plan.sql.clone(), transaction: outcome.transaction })
}

/// 调用者在 validate_context 和此函数之间重校有效配置/端点，再发送唯一服务端计划。
/// 任何发送后的无确认错误都不转成 affected=0，也不换执行 API 或重试。
pub(crate) async fn execute(http: &HttpDb, plan_id: &str, plan: &PlanInternal,
    effective_read_only: bool, timeout_secs: u64) -> Result<MutationReceipt> {
    let request = operation(plan_id, plan, effective_read_only, timeout_secs)?;
    let outcome = http.apply_row_plan(&request).await.map_err(|_| anyhow!(AppError::coded(
        "MUTATION_EFFECTS_UNKNOWN", "类型化事务操作没有获得完整确认；禁止自动重放，必须隔离并重新核对连接").operation(plan_id)))?;
    map_outcome(plan_id, plan, &request, outcome)
}

#[cfg(test)]
#[path = "jdbc_mutation_bridge_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "jdbc_mutation_c2_real_tests.rs"]
mod real_tests;
