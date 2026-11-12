//! PL-05/06：原生 SQLite main 现有表的非破坏受控保存。
//! 仅添加尾部列/普通有序列索引；不 DROP、rename、改 PK、重建、执行客户端 SQL。
//! 本模块不签 grant、不 register、不 finish。父服务保管 TypedReview，只接受 opaque review id，
//! 在 grant/register 前调用 validate_request，再把实际注册请求和 owned ordinary 租约传给 run。

use std::{collections::BTreeSet, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use anyhow::{anyhow, ensure, Result};
use serde::Serialize;
use crate::{db::Db, design::{ColumnDesign, DesignReceipt, DesignStep, DesignStepResult, ForeignKeyDesign, IndexDesign, TableDesign, TableOptionsDesign, CheckDesign, TriggerDesign},
    error::{AppError, CmdResult}, execution::{self, Binding, Entry}, execution_contracts::ExecutionRequest,
    manual_transaction::OrdinaryLease, models::{DbKind, IndexInfo, MultiResult, Protocol},
    structure_execution::metadata::{self, OwnedPair, OwnedTableSnapshot}};

const REVIEW_TTL: Duration = Duration::from_secs(180);
const DESIGN_BYTES: usize = 4 * 1024 * 1024;
const REVIEW_BYTES: usize = 32 * 1024 * 1024;
const MAX_STEPS: usize = 512;

#[derive(Debug, Clone, Serialize)]
pub struct DesignObservation {
    pub snapshot: OwnedTableSnapshot,
    pub design: TableDesign,
    pub source_fingerprint: String,
    pub limitations: Vec<String>,
}

/// Server-only immutable review. Serialization is for displaying a preview, NOT restoring authority.
/// No Deserialize implementation, no public mutable fields, no Registry/second execution lifecycle.
#[derive(Debug, Clone, Serialize)]
pub struct TypedReview {
    review_id: String,
    digest: String,
    binding: Binding,
    design: TableDesign,
    baseline: OwnedTableSnapshot,
    steps: Vec<DesignStep>,
    limitations: Vec<String>,
    #[serde(skip)] issued_at: Instant,
    #[serde(skip)] used: Arc<AtomicBool>,
}
impl TypedReview {
    pub fn review_id(&self) -> &str { &self.review_id }
    pub fn digest(&self) -> &str { &self.digest }
    pub fn binding(&self) -> &Binding { &self.binding }
    pub fn design(&self) -> &TableDesign { &self.design }
    pub fn baseline(&self) -> &OwnedTableSnapshot { &self.baseline }
    pub fn steps(&self) -> &[DesignStep] { &self.steps }
    pub fn request_sql(&self) -> String { format!("TABLE_DESIGN_REVIEW {} {}", self.review_id, self.digest) }
    pub fn expires_in_ms(&self) -> u64 { REVIEW_TTL.saturating_sub(self.issued_at.elapsed()).as_millis() as u64 }
    fn seal(&self) -> String {
        crate::object_catalog::revision(&("table-design-owned/v1", &self.review_id, &self.binding, &self.design, &self.baseline, &self.baseline.base_revision, &self.steps))
    }
    fn valid(&self) -> CmdResult<()> {
        if self.used.load(Ordering::Acquire) { return Err(coded("TABLE_DESIGN_REVIEW_USED", "预览已经消费；禁止复用/自动重放")); }
        if self.issued_at.elapsed() >= REVIEW_TTL { return Err(coded("TABLE_DESIGN_REVIEW_EXPIRED", "180 秒预览已过期；没有替换编辑打开时的原始基线")); }
        if self.seal() != self.digest { return Err(coded("TABLE_DESIGN_REVIEW_CHANGED", "后端完整目标/目录/绑定与预览摘要不同")); }
        Ok(())
    }
}

pub struct BoundResult {
    pub result: CmdResult<MultiResult>,
    pub confirmed: bool,
    pub receipt: Option<DesignReceipt>,
    /// 真实 committed/rolled-back 目录观察。Unknown 永远没有迟到 snapshot/成功 receipt。
    pub snapshot: Option<OwnedTableSnapshot>,
}
impl BoundResult {
    fn rejected(error: AppError, confirmed: bool) -> Self { Self { result: Err(error), confirmed, receipt: None, snapshot: None } }
}
#[derive(Clone, Copy)]
enum TestMode {
    Normal,
    #[cfg(test)] StopAfterFirstDdl,
    #[cfg(test)] LoseCommitAcknowledgement,
}
struct Applied {
    result: Result<MultiResult>,
    receipt: Option<DesignReceipt>,
    snapshot: Option<OwnedTableSnapshot>,
    destroy: bool,
}
#[derive(Default)]
struct Published { receipt: Option<DesignReceipt>, snapshot: Option<OwnedTableSnapshot> }
fn coded(code: &str, message: impl Into<String>) -> AppError { AppError::coded(code, message) }
fn unknown(message: impl Into<String>) -> anyhow::Error { coded("EXECUTION_EFFECTS_UNKNOWN", message).into() }
fn is_unknown(error: &anyhow::Error) -> bool { error.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN") }
fn stopped(result: &CmdResult<MultiResult>) -> bool {
    result.as_ref().err().is_some_and(|e| matches!(e.code.as_str(), "EXECUTION_CANCELLED_BEFORE_SEND" | "TABLE_DESIGN_STOPPED_ROLLED_BACK"))
}
fn valid_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty() && name.len() <= 1024 && !name.chars().any(char::is_control), "TABLE_DESIGN_NAME_INVALID: 名称为空、过长或含控制字符");
    metadata::ident(name)?; Ok(())
}
fn validate_scope(kind: &DbKind, binding: &Binding, schema: &str, table: &str) -> CmdResult<()> {
    if kind.protocol() != Protocol::Sqlite { return Err(coded("TABLE_DESIGN_ADAPTER_UNSUPPORTED", "此 owned 执行器仅认证原生 SQLite；MySQL/PostgreSQL/H2/JDBC 未接入本通道")); }
    if schema != "main" || binding.namespace != schema { return Err(coded("TABLE_DESIGN_SCOPE_INVALID", "必须精确 main，不把 attached/temp/空命名空间当作 main")); }
    valid_name(table).map_err(AppError::from)?;
    if binding.principal.is_empty() || binding.principal.len() > 256 || binding.principal.chars().any(char::is_control)
        || binding.owner.trim().is_empty() || binding.owner.len() > 256 || binding.owner.chars().any(char::is_control)
        || binding.connection.is_empty() || binding.connection.len() > 128 || binding.connection.chars().any(char::is_control)
        || binding.fingerprint.len() != 64 || !binding.fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(coded("TABLE_DESIGN_BINDING_REQUIRED", "必须使用真实窗口/页面/保存连接与材料 SHA-256 绑定"));
    }
    Ok(())
}
fn same<T: Serialize>(left: &T, right: &T) -> Result<bool> { Ok(serde_json::to_vec(left)? == serde_json::to_vec(right)?) }
fn normalized_type(value: &str) -> String { value.chars().filter(|c| !c.is_ascii_whitespace()).flat_map(char::to_uppercase).collect() }
fn declared_type(column: &ColumnDesign, executable: bool) -> Result<String> {
    let mut value = column.data_type.trim().to_owned();
    if let Some(length) = &column.length {
        ensure!(!value.contains('('), "TABLE_DESIGN_TYPE_INVALID: 重复尺寸定义");
        let number = |raw: &str| !raw.is_empty() && raw.len() <= 10 && raw.bytes().all(|b| b.is_ascii_digit());
        ensure!(number(length), "TABLE_DESIGN_TYPE_INVALID: 尺寸必须是十进制整数");
        value.push('('); value.push_str(length);
        if let Some(scale) = &column.scale { ensure!(number(scale), "TABLE_DESIGN_TYPE_INVALID: 小数位必须是整数"); value.push(','); value.push_str(scale); }
        value.push(')');
    } else { ensure!(column.scale.is_none(), "TABLE_DESIGN_TYPE_INVALID: 只有小数位，没有精度"); }
    if !executable { return Ok(value); }
    let (base, suffix) = value.split_once('(').map_or((value.as_str(), None), |(base, suffix)| (base.trim(), Some(suffix)));
    let base = base.split_ascii_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    ensure!(["INT", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "UNSIGNED BIG INT", "INT2", "INT8", "TEXT", "CHARACTER", "VARCHAR", "VARYING CHARACTER", "NCHAR", "NATIVE CHARACTER", "NVARCHAR", "CLOB", "BLOB", "REAL", "DOUBLE", "DOUBLE PRECISION", "FLOAT", "NUMERIC", "DECIMAL", "BOOLEAN", "DATE", "DATETIME", "TIMESTAMP", "TIME"].contains(&base.as_str()), "TABLE_DESIGN_TYPE_UNSUPPORTED: 不把类型文本当作任意 SQL");
    let mut result = base;
    if let Some(suffix) = suffix {
        let suffix = suffix.strip_suffix(')').ok_or_else(|| anyhow!("TABLE_DESIGN_TYPE_INVALID"))?;
        let dimensions: Vec<&str> = suffix.split(',').map(str::trim).collect();
        ensure!(!dimensions.is_empty() && dimensions.len() <= 2 && dimensions.iter().all(|s| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit())), "TABLE_DESIGN_TYPE_INVALID");
        result.push('('); result.push_str(&dimensions.join(",")); result.push(')');
    }
    Ok(result)
}
/// SQLite ADD COLUMN only accepts constants: no expression/functions, CURRENT_* or SELECT.
fn constant(raw: &str) -> Result<String> {
    let value = raw.trim();
    ensure!(!value.is_empty() && value.len() <= 256 && !value.chars().any(|c| c == '\0'), "TABLE_DESIGN_DEFAULT_UNSUPPORTED");
    if ["NULL", "TRUE", "FALSE"].iter().any(|s| value.eq_ignore_ascii_case(s)) { return Ok(value.to_owned()); }
    let string = |value: &str| -> bool {
        if !value.starts_with('\'') { return false; }
        let b = value.as_bytes(); let mut i = 1;
        while i < b.len() {
            if b[i] == b'\'' { i += 1; if i < b.len() && b[i] == b'\'' { i += 1; continue; } return i == b.len(); }
            i += 1;
        }
        false
    };
    if string(value) { return Ok(value.to_owned()); }
    if value.len() >= 3 && (value.starts_with("X'") || value.starts_with("x'")) && value.ends_with('\'') {
        let bytes = &value[2..value.len()-1];
        ensure!(bytes.len() % 2 == 0 && bytes.bytes().all(|b| b.is_ascii_hexdigit()), "TABLE_DESIGN_DEFAULT_UNSUPPORTED");
        return Ok(value.to_owned());
    }
    let number = value.strip_prefix('+').or_else(|| value.strip_prefix('-')).unwrap_or(value);
    let mut exponent = number.split(['e', 'E']); let mantissa = exponent.next().unwrap_or("");
    let exp = exponent.next(); ensure!(exponent.next().is_none(), "TABLE_DESIGN_DEFAULT_UNSUPPORTED");
    ensure!(mantissa.bytes().any(|b| b.is_ascii_digit()) && mantissa.bytes().all(|b| b.is_ascii_digit() || b == b'.') && mantissa.bytes().filter(|b| *b == b'.').count() <= 1, "TABLE_DESIGN_DEFAULT_UNSUPPORTED: 不执行自定义默认值 SQL");
    if let Some(exp) = exp { let exp = exp.strip_prefix('+').or_else(|| exp.strip_prefix('-')).unwrap_or(exp); ensure!(!exp.is_empty() && exp.bytes().all(|b| b.is_ascii_digit()), "TABLE_DESIGN_DEFAULT_UNSUPPORTED"); }
    Ok(value.to_owned())
}
fn check_expression(definition: &str) -> Result<String> {
    let tokens = crate::table_objects::tokens(definition)?;
    ensure!(tokens.len() >= 3 && tokens[0].word("CHECK") && tokens[1].symbol("(") && tokens.last().is_some_and(|t| t.symbol(")")), "TABLE_DESIGN_CHECK_METADATA_UNKNOWN");
    Ok(definition[tokens[1].end..tokens.last().expect("length checked").start].to_owned())
}
fn target_from_snapshot(before: &OwnedTableSnapshot, binding: &Binding) -> Result<TableDesign> {
    Ok(TableDesign {
        schema: before.identity.schema.clone(), table: before.identity.table.clone(),
        columns: before.schema.columns.iter().map(|c| ColumnDesign { name: c.name.clone(), orig_name: Some(c.name.clone()), data_type: c.data_type.clone(), length: None, scale: None,
            nullable: c.nullable, default_value: c.default.clone(), auto_increment: c.auto_increment, comment: c.comment.clone(), unsigned: false, zerofill: false }).collect(),
        primary_key: crate::design::ordered_primary_key(&before.schema)?, comment: None,
        indexes: Some(before.schema.indexes.iter().filter(|i| !i.primary).map(|i| IndexDesign { name: i.name.clone(), columns: i.columns.clone(), unique: i.unique, method: i.method.clone(), descending: Vec::new() }).collect()),
        foreign_keys: Some(before.schema.foreign_keys.iter().map(|f| ForeignKeyDesign { name: f.name.clone(), columns: f.columns.clone(), ref_table: f.ref_table.clone(), ref_schema: f.ref_schema.clone(), ref_columns: f.ref_columns.clone(), on_update: f.on_update.clone(), on_delete: f.on_delete.clone() }).collect()),
        checks: Some(before.objects.checks.iter().map(|c| Ok(CheckDesign { name: c.name.clone(), orig_name: Some(c.name.clone()), expression: check_expression(&c.definition)?, generated_name: c.generated_name })).collect::<Result<_>>()?),
        triggers: Some(before.objects.triggers.iter().map(|t| TriggerDesign { name: t.name.clone(), orig_name: Some(t.name.clone()), timing: String::new(), event: String::new(), statement: t.statement.clone(), definition: Some(t.statement.clone()) }).collect()),
        options: Some(TableOptionsDesign { strict: Some(false), without_rowid: Some(false), ..Default::default() }),
        base_revision: Some(before.revision.clone()), source_fingerprint: Some(binding.fingerprint.clone()),
    })
}
fn limitations() -> Vec<String> { vec![
    "仅原生 SQLite main 现有表：安全尾部增列与新增普通/唯一有序列索引".into(),
    "不修改主键、既有列、外键、CHECK、触发器或物理表选项；不 DROP/rename/CASCADE/整表重建".into(),
    "generated/AUTOINCREMENT/STRICT/WITHOUT ROWID/表达式索引等不能完整建模的目录明确拒绝，不当作空目录".into(),
    "SQLite 声明长度/精度不是数据库强制长度/精度承诺；MySQL/PostgreSQL 的 owned 保存不在此通道".into(),
] }

async fn observe(db: &Db, kind: &DbKind, table: &str) -> CmdResult<OwnedTableSnapshot> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let pool = metadata::sqlite_pool(db, kind).map_err(AppError::from)?;
    let mut pair = metadata::acquire_pair(pool, pool, None).await.map_err(AppError::from)?;
    // Drop/panic cannot silently return a connection with raw BEGIN/callback state; explicit cleanup returns clean connections.
    pair.target.close_on_drop();
    let observed = async {
        ensure!(pair.same_database(), "TABLE_DESIGN_PHYSICAL_IDENTITY_UNKNOWN"); pair.validate_paths()?; pair.begin(false).await?;
        let before = metadata::snapshot(&mut pair.target, &pair.target_physical, "main", table).await?;
        let after = metadata::snapshot(&mut pair.target, &pair.target_physical, "main", table).await?;
        ensure!(before.revision == after.revision && same(&before.schema, &after.schema)? && same(&before.objects, &after.objects)?, "TABLE_DESIGN_OBSERVATION_CHANGED");
        pair.validate_paths()?; Ok::<_, anyhow::Error>(before)
    }.await;
    match pair.cleanup(false).await { Ok(()) => observed.map_err(AppError::from), Err(e) => Err(coded("EXECUTION_EFFECTS_UNKNOWN", format!("表设计只读观察的 owned 清理未确认：{e:#}"))) }

    }).await
}
/// Open-editor observation. Never fresh-replace this baseline at save time or treat it as a commit receipt.
pub async fn inspect(db: Arc<Db>, kind: DbKind, ordinary: OrdinaryLease, schema: String, table: String, binding: Binding) -> CmdResult<DesignObservation> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let _ordinary = ordinary; validate_scope(&kind, &binding, &schema, &table)?;
    let snapshot = observe(&db, &kind, &table).await?;
    let design = target_from_snapshot(&snapshot, &binding).map_err(AppError::from)?;
    Ok(DesignObservation { snapshot, design, source_fingerprint: binding.fingerprint, limitations: limitations() })

    }).await
}

fn compile(before: &OwnedTableSnapshot, design: &TableDesign) -> Result<Vec<DesignStep>> {
    ensure!(design.schema == "main" && design.table == before.identity.table && before.objects.completeness == "complete" && before.objects.limitations.is_empty(), "TABLE_DESIGN_METADATA_UNPROVEN");
    ensure!(serde_json::to_vec(design)?.len() <= DESIGN_BYTES && design.columns.len() <= 4096, "TABLE_DESIGN_DRAFT_QUOTA");
    ensure!(design.base_revision.as_deref() == Some(before.revision.as_str()), "TABLE_DESIGN_BASELINE_CHANGED: 不替换打开时基线");
    ensure!(design.primary_key == crate::design::ordered_primary_key(&before.schema)?, "TABLE_DESIGN_PRIMARY_KEY_UNSUPPORTED: 有序主键不可重排/改变");
    ensure!(design.columns.len() >= before.schema.columns.len(), "TABLE_DESIGN_DROP_UNSUPPORTED");
    let mut names = BTreeSet::new(); let mut steps = Vec::new();
    let table = format!("{}.{}", metadata::ident("main")?, metadata::ident(&design.table)?);
    for (i, column) in design.columns.iter().enumerate() {
        valid_name(&column.name)?;
        ensure!(names.insert(column.name.to_ascii_lowercase()), "TABLE_DESIGN_COLUMN_DUPLICATE");
        ensure!(!column.unsigned && !column.zerofill, "TABLE_DESIGN_COLUMN_ATTRIBUTE_UNSUPPORTED: SQLite 不忽略 unsigned/zerofill");
        if let Some(original) = before.schema.columns.get(i) {
            ensure!(column.orig_name.as_deref() == Some(original.name.as_str()) && column.name == original.name, "TABLE_DESIGN_REORDER_RENAME_UNSUPPORTED");
            ensure!(normalized_type(&declared_type(column, false)?) == normalized_type(&original.data_type)
                && column.nullable == original.nullable && column.default_value == original.default && column.auto_increment == original.auto_increment
                && column.comment.as_deref().unwrap_or("") == original.comment.as_deref().unwrap_or(""), "TABLE_DESIGN_EXISTING_COLUMN_CHANGED: type/尺寸/default/可空/自增/注释必须完整保留");
        } else {
            ensure!(column.orig_name.is_none() && !column.auto_increment && column.comment.as_deref().map_or(true, str::is_empty), "TABLE_DESIGN_NEW_COLUMN_UNSUPPORTED");
            let data_type = declared_type(column, true)?;
            let default = column.default_value.as_deref().map(constant).transpose()?;
            ensure!(column.nullable || default.as_ref().is_some_and(|d| !d.eq_ignore_ascii_case("NULL")), "TABLE_DESIGN_NOT_NULL_DEFAULT_REQUIRED: 非空增列须有非 NULL 常量默认值");
            let mut sql = format!("ALTER TABLE {table} ADD COLUMN {} {data_type}", metadata::ident(&column.name)?);
            if !column.nullable { sql.push_str(" NOT NULL"); }
            if let Some(default) = default { sql.push_str(" DEFAULT "); sql.push_str(&default); }
            steps.push(DesignStep { label: format!("新增列 {}", column.name), sql });
        }
    }
    ensure!(design.comment.as_deref().map_or(true, str::is_empty), "TABLE_DESIGN_COMMENT_UNSUPPORTED");
    let preserved = target_from_snapshot(before, &Binding { principal: String::new(), owner: String::new(), connection: String::new(), namespace: "main".into(), fingerprint: String::new() })?;
    if let Some(foreign_keys) = &design.foreign_keys { ensure!(same(foreign_keys, preserved.foreign_keys.as_ref().expect("snapshot creates Some"))?, "TABLE_DESIGN_FOREIGN_KEY_UNSUPPORTED: 完整有序外键只能保留"); }
    if let Some(checks) = &design.checks { ensure!(same(checks, preserved.checks.as_ref().expect("snapshot creates Some"))?, "TABLE_DESIGN_CHECK_UNSUPPORTED: 完整 CHECK 身份/字面量只能保留"); }
    if let Some(triggers) = &design.triggers { ensure!(same(triggers, preserved.triggers.as_ref().expect("snapshot creates Some"))?, "TABLE_DESIGN_TRIGGER_UNSUPPORTED: 完整父身份/定义只能保留"); }
    if let Some(options) = &design.options {
        ensure!(options.engine.is_none() && options.charset.is_none() && options.collation.is_none() && options.row_format.is_none() && options.fillfactor.is_none()
            && options.strict != Some(true) && options.without_rowid != Some(true), "TABLE_DESIGN_OPTIONS_UNSUPPORTED: 不忽略未建模表属性");
    }
    if let Some(indexes) = &design.indexes {
        ensure!(indexes.len() <= 512, "TABLE_DESIGN_INDEX_QUOTA"); let mut seen = BTreeSet::new();
        for index in indexes {
            valid_name(&index.name)?; ensure!(seen.insert(index.name.to_ascii_lowercase()), "TABLE_DESIGN_INDEX_DUPLICATE");
            ensure!(!index.columns.is_empty() && index.columns.len() <= 4096, "TABLE_DESIGN_INDEX_INVALID");
            let mut columns = BTreeSet::new();
            for name in &index.columns { ensure!(design.columns.iter().any(|c| &c.name == name) && columns.insert(name), "TABLE_DESIGN_INDEX_COLUMN_INVALID: 索引必须引用完整有序普通列"); }
        }
        for original in before.schema.indexes.iter().filter(|i| !i.primary) {
            let target = indexes.iter().find(|i| i.name == original.name).ok_or_else(|| anyhow!("TABLE_DESIGN_INDEX_DROP_UNSUPPORTED"))?;
            ensure!(target.columns == original.columns && target.unique == original.unique && original.method.is_none(), "TABLE_DESIGN_INDEX_REPLACEMENT_UNSUPPORTED");
        }
        for index in indexes.iter().filter(|i| !before.schema.indexes.iter().any(|old| old.name == i.name)) {
            ensure!(!index.name.to_ascii_lowercase().starts_with("sqlite_"), "TABLE_DESIGN_RESERVED_INDEX_NAME");
            let columns = index.columns.iter().map(|c| metadata::ident(c)).collect::<Result<Vec<_>>>()?.join(", ");
            // SQLite schema qualification belongs to the index name, not the ON table.
            steps.push(DesignStep { label: format!("新增索引 {}", index.name), sql: format!("CREATE {}INDEX {}.{} ON {} ({columns})", if index.unique { "UNIQUE " } else { "" }, metadata::ident("main")?, metadata::ident(&index.name)?, metadata::ident(&design.table)?) });
        }
    }
    ensure!(steps.len() <= MAX_STEPS, "TABLE_DESIGN_STEP_QUOTA"); Ok(steps)
}

pub async fn preflight(db: Arc<Db>, kind: DbKind, ordinary: OrdinaryLease, design: TableDesign, binding: Binding, read_only: bool) -> CmdResult<TypedReview> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let _ordinary = ordinary; validate_scope(&kind, &binding, &design.schema, &design.table)?;
    if read_only { return Err(coded("TABLE_DESIGN_READ_ONLY", "真实保存连接为只读；未获取写入预览资格")); }
    if design.source_fingerprint.as_deref() != Some(binding.fingerprint.as_str()) { return Err(coded("TABLE_DESIGN_SOURCE_CHANGED", "必须携带打开编辑器时的真实材料指纹")); }
    if design.base_revision.as_deref().map_or(true, str::is_empty) { return Err(coded("TABLE_DESIGN_BASELINE_REQUIRED", "没有编辑打开时的原始目录基线，不能保存时新读基线")); }
    if serde_json::to_vec(&design).map_err(AppError::from)?.len() > DESIGN_BYTES { return Err(coded("TABLE_DESIGN_DRAFT_QUOTA", "完整表设计超过 4 MiB")); }
    let baseline = observe(&db, &kind, &design.table).await?;
    if design.base_revision.as_deref() != Some(baseline.revision.as_str()) { return Err(coded("TABLE_DESIGN_CONFLICT", "真实目录已改变；保留原始草稿/基线，未编译执行预览")); }
    let steps = compile(&baseline, &design).map_err(AppError::from)?;
    if steps.is_empty() { return Err(coded("TABLE_DESIGN_NO_CHANGES", "没有数据库 DDL 变更；只读观察不是保存成功")); }
    let mut review = TypedReview { review_id: uuid::Uuid::new_v4().to_string(), digest: String::new(), binding, design, baseline, steps, limitations: limitations(), issued_at: Instant::now(), used: Arc::new(AtomicBool::new(false)) };
    review.digest = review.seal();
    if serde_json::to_vec(&review).map_err(AppError::from)?.len() > REVIEW_BYTES { return Err(coded("TABLE_DESIGN_REVIEW_QUOTA", "完整后端预览超过 32 MiB")); }
    Ok(review)

    }).await
}
/// Must be called by the trusted parent BEFORE issuing/consuming C1 grant. It never executes marker SQL.
pub fn validate_request(review: &TypedReview, request: &ExecutionRequest, binding: &Binding) -> CmdResult<()> {
    review.valid()?;
    if binding != &review.binding || request.connection_id != binding.connection || request.namespace != binding.namespace || request.owner_id != binding.owner
        || request.sql != review.request_sql() || request.transaction_owner.is_some() {
        return Err(coded("TABLE_DESIGN_REQUEST_MISMATCH", "完整注册请求/真实窗口页面/目录/后端 marker 不匹配；手动事务不支持"));
    }
    Ok(())
}

fn verify_readback(before: &OwnedTableSnapshot, after: &OwnedTableSnapshot, design: &TableDesign) -> Result<()> {
    ensure!(before.identity == after.identity && after.objects.completeness == "complete" && after.objects.limitations.is_empty(), "TABLE_DESIGN_READBACK_IDENTITY_CHANGED");
    ensure!(after.schema.columns.len() == design.columns.len(), "TABLE_DESIGN_READBACK_COLUMN_COUNT");
    for (i, target) in design.columns.iter().enumerate() {
        let actual = &after.schema.columns[i]; ensure!(actual.name == target.name, "TABLE_DESIGN_READBACK_COLUMN_ORDER");
        if let Some(original) = before.schema.columns.get(i) { ensure!(same(original, actual)?, "TABLE_DESIGN_READBACK_EXISTING_COLUMN_CHANGED"); }
        else {
            ensure!(normalized_type(&actual.data_type) == normalized_type(&declared_type(target, true)?) && actual.nullable == target.nullable
                && actual.default == target.default_value.as_deref().map(str::trim).map(str::to_owned)
                && !actual.auto_increment && !actual.primary_key && actual.comment.is_none(), "TABLE_DESIGN_READBACK_NEW_COLUMN_CHANGED");
        }
    }
    ensure!(crate::design::ordered_primary_key(&before.schema)? == crate::design::ordered_primary_key(&after.schema)?, "TABLE_DESIGN_READBACK_PRIMARY_KEY_CHANGED");
    ensure!(same(&before.schema.foreign_keys, &after.schema.foreign_keys)? && same(&before.objects.checks, &after.objects.checks)? && same(&before.objects.triggers, &after.objects.triggers)?, "TABLE_DESIGN_READBACK_CONSTRAINT_CHANGED");
    let expected: Vec<IndexInfo> = match &design.indexes {
        None => before.schema.indexes.clone(),
        Some(indexes) => before.schema.indexes.iter().filter(|i| i.primary).cloned().chain(indexes.iter().map(|i| IndexInfo { name: i.name.clone(), columns: i.columns.clone(), unique: i.unique, primary: false, method: None })).collect(),
    };
    ensure!(expected.len() == after.schema.indexes.len(), "TABLE_DESIGN_READBACK_INDEX_COUNT");
    for index in &expected { let actual = after.schema.indexes.iter().find(|i| i.name == index.name).ok_or_else(|| anyhow!("TABLE_DESIGN_READBACK_INDEX_MISSING"))?; ensure!(same(index, actual)?, "TABLE_DESIGN_READBACK_INDEX_ORDER_CHANGED"); }
    ensure!(after.schema.comment == before.schema.comment && after.schema.engine == before.schema.engine && after.schema.collation == before.schema.collation, "TABLE_DESIGN_READBACK_OPTIONS_CHANGED");
    Ok(())
}
async fn rollback(pair: &mut OwnedPair) -> Result<()> {
    execution::clear_sqlite(&mut pair.target).await?;
    if pair.target_tx { crate::sql_history::query("ROLLBACK").execute(&mut *pair.target).await?; metadata::ensure_autocommit(&mut pair.target).await?; pair.target_tx = false; }
    Ok(())
}
async fn apply(pair: &mut OwnedPair, review: &TypedReview, entry: &Arc<Entry>, mode: TestMode) -> Applied {
    let started = Instant::now(); let mut before: Option<OwnedTableSnapshot> = None;
    let mut results: Vec<DesignStepResult> = Vec::new();
    let outcome = async {
        if entry.stopping() { return Err(coded("EXECUTION_CANCELLED_BEFORE_SEND", "发送表设计 DDL 前停止").into()); }
        if review.issued_at.elapsed() >= REVIEW_TTL { return Err(coded("TABLE_DESIGN_REVIEW_EXPIRED", "池等待超过原始预览期限").into()); }
        ensure!(pair.same_database(), "TABLE_DESIGN_PHYSICAL_IDENTITY_UNKNOWN"); pair.validate_paths()?; pair.begin(true).await?;
        let current = metadata::snapshot(&mut pair.target, &pair.target_physical, "main", &review.design.table).await?;
        before = Some(current.clone());
        if current.revision != review.baseline.revision || current.identity != review.baseline.identity || current.base_revision != review.baseline.base_revision {
            return Err(coded("TABLE_DESIGN_CONFLICT", "BEGIN IMMEDIATE 后完整目录/物理文件/有序键与原预览不同，没有发送 DDL").into());
        }
        let steps = compile(&current, &review.design)?;
        ensure!(steps == review.steps && review.seal() == review.digest, "TABLE_DESIGN_RECOMPILE_CHANGED");
        for (ordinal, step) in steps.iter().enumerate() {
            if entry.stopping() { return Err(coded("TABLE_DESIGN_STOPPED_ROLLED_BACK", "COMMIT 前停止，必须确认回滚").into()); }
            match crate::sql_history::query(&step.sql).execute(&mut *pair.target).await {
                Ok(_) => results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }),
                Err(error) => { results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(error.to_string()) }); return Err(error.into()); }
            }
            #[cfg(test)] if ordinal == 0 && matches!(mode, TestMode::StopAfterFirstDdl) { entry.request_stop(false); }
            #[cfg(not(test))] let _ = ordinal;
        }
        if entry.stopping() { return Err(coded("TABLE_DESIGN_STOPPED_ROLLED_BACK", "读回前停止").into()); }
        let after = metadata::snapshot(&mut pair.target, &pair.target_physical, "main", &review.design.table).await?;
        verify_readback(&current, &after, &review.design)?; pair.validate_paths()?;
        ensure!(crate::sql_history::query("PRAGMA main.foreign_key_check").fetch_optional(&mut *pair.target).await?.is_none(), "TABLE_DESIGN_FOREIGN_KEY_VIOLATION: 已拒绝 COMMIT");
        if entry.stopping() { return Err(coded("TABLE_DESIGN_STOPPED_ROLLED_BACK", "读回期间停止，未发送 COMMIT").into()); }
        crate::sql_history::query("COMMIT").execute(&mut *pair.target).await.map_err(|e| unknown(format!("TABLE_DESIGN_COMMIT_UNKNOWN: COMMIT 回执未确认，禁止重放：{e}")))?;
        pair.target_tx = false;
        #[cfg(test)] if matches!(mode, TestMode::LoseCommitAcknowledgement) { return Err(unknown("TABLE_DESIGN_COMMIT_UNKNOWN: 真实 COMMIT 成功后故障注入丢 ACK，无成功 receipt")); }
        if entry.stopping() { return Err(unknown("TABLE_DESIGN_COMMIT_STOP_RACE: 已发 COMMIT 与停止竞态，不声称已回滚")); }
        metadata::ensure_autocommit(&mut pair.target).await.map_err(|e| unknown(format!("TABLE_DESIGN_COMMIT_CLEANUP_UNKNOWN: {e:#}")))?;
        let result = MultiResult { sets: Vec::new(), messages: vec!["SQLite 表设计 DDL 同一物理 BEGIN IMMEDIATE 内读回完整列/有序键/CHECK/触发器/外键并提交".into()], elapsed_ms: started.elapsed().as_millis() as u64, spool_id: None };
        Ok::<_, anyhow::Error>((result, after))
    }.await;
    #[cfg(not(test))] let _ = mode;
    match outcome {
        Ok((result, snapshot)) => Applied { result: Ok(result), receipt: Some(DesignReceipt { steps: results, verified: true, unverified: false, mismatches: Vec::new(), transaction: "committed".into(), schema: Some(snapshot.schema.clone()) }), snapshot: Some(snapshot), destroy: false },
        Err(error) if is_unknown(&error) => {
            // No application rollback/replay after unknown COMMIT; physical close may discard an uncommitted transaction.
            pair.target_tx = false; Applied { result: Err(error), receipt: None, snapshot: None, destroy: true }
        }
        Err(error) => {
            if let Err(cleanup) = rollback(pair).await { pair.target_tx = false; return Applied { result: Err(unknown(format!("TABLE_DESIGN_ROLLBACK_UNKNOWN: 原错误 {error:#}；ROLLBACK/清理未确认 {cleanup:#}"))), receipt: None, snapshot: None, destroy: true }; }
            for step in &mut results { if step.status == "applied" { step.status = "rolled_back".into(); } }
            for step in review.steps.iter().skip(results.len()) { results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "skipped".into(), error: None }); }
            let snapshot = if let Some(before) = &before {
                metadata::snapshot(&mut pair.target, &pair.target_physical, "main", &review.design.table).await.ok().filter(|s| s.revision == before.revision && s.identity == before.identity)
            } else { None };
            let message = format!("{error:#}");
            let code = if entry.stopping() && before.is_some() { "TABLE_DESIGN_STOPPED_ROLLED_BACK" }
                else if let Some(e) = error.downcast_ref::<AppError>() { e.code.as_str() } else { "TABLE_DESIGN_FAILED_ROLLED_BACK" };
            let receipt = before.as_ref().map(|_| DesignReceipt { steps: results, verified: false, unverified: false, mismatches: vec![message.clone()], transaction: "rolled_back".into(), schema: snapshot.as_ref().map(|s| s.schema.clone()) });
            Applied { result: Err(coded(code, format!("未提交表设计；ROLLBACK/自提交状态已确认：{message}")).into()), receipt, snapshot, destroy: false }
        }
    }
}

pub async fn run(db: Arc<Db>, kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease, review: TypedReview, request: ExecutionRequest, read_only: bool) -> BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    run_mode(db, kind, entry, ordinary, review, request, read_only, TestMode::Normal).await

    }).await
}
async fn pinned_work(db: &Db, kind: &DbKind, entry: &Arc<Entry>, review: &TypedReview, mode: TestMode, output: &Arc<parking_lot::Mutex<Published>>) -> Result<MultiResult> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let pool = metadata::sqlite_pool(db, kind)?;
    let mut pair = match metadata::acquire_pair(pool, pool, Some(entry)).await {
        Ok(pair) => pair,
        Err(error) if error.to_string().starts_with("STRUCTURE_ACQUIRE_CLEANUP_UNKNOWN:") => {
            entry.record_remote_transaction("unknown");
            return Err(unknown(format!("TABLE_DESIGN_ACQUIRE_CLEANUP_UNKNOWN: {error:#}")));
        }
        Err(error) => return Err(error),
    };
    pair.target.close_on_drop();
    let applied = apply(&mut pair, review, entry, mode).await;
    if applied.destroy { entry.record_remote_transaction("unknown"); }
    if let Err(error) = pair.cleanup(applied.destroy).await {
        entry.record_remote_transaction("unknown");
        return Err(unknown(format!("TABLE_DESIGN_OWNED_CLEANUP_UNKNOWN: {error:#}")));
    }
    if !applied.destroy {
        if let Some(receipt) = &applied.receipt { entry.record_remote_transaction(&receipt.transaction); }
        else { entry.record_remote_transaction("none"); }
        *output.lock() = Published { receipt: applied.receipt, snapshot: applied.snapshot };
    }
    applied.result

    }).await
}
async fn worker(db: Arc<Db>, kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease, review: TypedReview, mode: TestMode, output: Arc<parking_lot::Mutex<Published>>) -> (CmdResult<MultiResult>, bool) {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    // The owning lease is INSIDE the supervised worker and outlives actual callbacks/rollback/connection cleanup.
    let _ordinary = ordinary;
    if entry.stopping() || !entry.start() { return (Err(coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队期间停止，零表设计 SQL")), true); }
    let (result, proof) = execution::drive(pinned_work(&db, &kind, &entry, &review, mode, &output), &entry, || async { Ok(()) }).await;
    let result = result.map_err(AppError::from); let proof = proof || stopped(&result); (result, proof)

    }).await
}
async fn run_mode(db: Arc<Db>, kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease, review: TypedReview, request: ExecutionRequest, read_only: bool, mode: TestMode) -> BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if read_only { return BoundResult::rejected(coded("TABLE_DESIGN_READ_ONLY", "真实保存连接只读，未获取 pool"), false); }
    if let Err(error) = validate_scope(&kind, &entry.binding, &review.design.schema, &review.design.table).and_then(|_| validate_request(&review, &request, &entry.binding)) { return BoundResult::rejected(error, false); }
    if entry.id != request.execution_id || request.execution_grant.as_deref().map_or(true, str::is_empty) { return BoundResult::rejected(coded("TABLE_DESIGN_EXECUTION_MISMATCH", "实际 C1 Entry 与完整已注册请求 ID/grant 不同"), false); }
    if !matches!(db.as_ref(), Db::Sqlite(_)) { return BoundResult::rejected(coded("TABLE_DESIGN_ADAPTER_UNSUPPORTED", "不是原生 SQLite owned pool"), false); }
    if review.used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return BoundResult::rejected(coded("TABLE_DESIGN_REVIEW_USED", "预览只消费一次，禁止重放"), false); }
    let output = Arc::new(parking_lot::Mutex::new(Published::default()));
    let (result, confirmed) = execution::supervise(entry.clone(), worker(db, kind, entry.clone(), ordinary, review, mode, output.clone())).await;
    let publish = result.is_ok() && !entry.stopping() || result.as_ref().err().is_some_and(|e| matches!(e.code.as_str(), "TABLE_DESIGN_CONFLICT" | "TABLE_DESIGN_FAILED_ROLLED_BACK" | "TABLE_DESIGN_STOPPED_ROLLED_BACK"));
    let published = if publish { std::mem::take(&mut *output.lock()) } else { Published::default() };
    BoundResult { confirmed: confirmed || stopped(&result), result, receipt: published.receipt, snapshot: published.snapshot }

    }).await
}

#[cfg(test)]
#[path = "table_design_execution_tests.rs"]
mod tests;
