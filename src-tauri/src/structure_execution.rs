//! PL-10 原生 SQLite owned 执行。这里只是既有 C1 Entry/supervise/drive 的调用者。
//! 不签 grant、不 register、不 finish；父服务负责真实 window/page/双端 endpoint 和唯一终态。

#[path = "structure_metadata.rs"]
pub mod metadata;

use std::{sync::Arc, time::Instant};
use anyhow::{anyhow, ensure, Result};
use crate::{db::Db, design::{self, DesignReceipt, DesignStep, DesignStepResult, TableDesign},
    error::{AppError, CmdResult}, execution::{self, Entry}, manual_transaction::OrdinaryLease,
    models::{DbKind, MultiResult}, structure_sync::{self, StructureDiffReport, StructureReviewBinding, StructureSyncReceipt, StructureSyncRequest}};
use metadata::{OwnedPair, OwnedTableSnapshot};

pub struct BoundResult {
    pub result: CmdResult<MultiResult>,
    pub confirmed: bool,
    pub receipt: Option<StructureSyncReceipt>,
}
#[derive(Clone, Copy)]
enum TestMode {
    Normal,
    #[cfg(test)] FailAfterFirstDdl,
    #[cfg(test)] StopAfterFirstDdl,
    #[cfg(test)] LoseCommitAcknowledgement,
}
struct Applied {
    result: Result<MultiResult>,
    receipt: Option<StructureSyncReceipt>,
    destroy: bool,
}
fn before_send() -> anyhow::Error {
    AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送结构 DDL 前，未修改 schema").into()
}
fn effects_unknown(message: impl Into<String>) -> anyhow::Error {
    AppError::coded("EXECUTION_EFFECTS_UNKNOWN", message).into()
}
fn is_unknown(error: &anyhow::Error) -> bool { error.downcast_ref::<AppError>().is_some_and(|error| error.code == "EXECUTION_EFFECTS_UNKNOWN") }
fn confirmed_stop(result: &CmdResult<MultiResult>) -> bool {
    result.as_ref().err().is_some_and(|error| error.code == "EXECUTION_CANCELLED_BEFORE_SEND" || error.code == "STRUCTURE_STOPPED_ROLLED_BACK")
}
fn validate_entry(entry: &Entry, request: &StructureSyncRequest, binding: &StructureReviewBinding, read_only: bool) -> CmdResult<()> {
    if read_only { return Err(AppError::coded("STRUCTURE_READ_ONLY", "目标保存连接是只读，未取 pool/发送 DDL")); }
    if request.allow_drop { return Err(AppError::coded("STRUCTURE_DESTRUCTIVE_UNSUPPORTED", "当前原生受控流程不 DROP/CASCADE/重建表，未取 pool")); }
    metadata::validate_scopes(request, binding).map_err(AppError::from)?;
    structure_sync::validate_review_request(request, binding).map_err(AppError::from)?;
    if entry.binding.principal != binding.principal || entry.binding.owner != binding.owner
        || entry.binding.connection != binding.target_connection_id || entry.binding.namespace != binding.target_schema
        || entry.binding.fingerprint != binding.target_fingerprint {
        return Err(AppError::coded("STRUCTURE_EXECUTION_BINDING_CHANGED", "C1 actual window/page/目标 endpoint 与双端 review 不同，未取 pool"));
    }
    Ok(())
}
fn validate_type(data_type: &str) -> Result<()> {
    let value = data_type.trim();
    let (base, suffix) = value.split_once('(').map_or((value, None), |(base, rest)| (base.trim(), Some(rest)));
    let base = base.split_ascii_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    ensure!(["INT", "INTEGER", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "UNSIGNED BIG INT", "INT2", "INT8", "TEXT", "CHARACTER", "VARCHAR", "VARYING CHARACTER", "NCHAR", "NATIVE CHARACTER", "NVARCHAR", "CLOB", "BLOB", "REAL", "DOUBLE", "DOUBLE PRECISION", "FLOAT", "NUMERIC", "DECIMAL", "BOOLEAN", "DATE", "DATETIME", "TIMESTAMP", "TIME"].contains(&base.as_str()), "STRUCTURE_TYPE_UNSUPPORTED: 类型不是认证原生类型，不能把 metadata 类型文本拼成任意 SQL");
    if let Some(suffix) = suffix {
        let suffix = suffix.strip_suffix(')').ok_or_else(|| anyhow!("STRUCTURE_TYPE_UNSUPPORTED"))?;
        let sizes: Vec<&str> = suffix.split(',').collect();
        ensure!(!sizes.is_empty() && sizes.len() <= 2 && sizes.iter().all(|n| { let n = n.trim(); !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()) }), "STRUCTURE_TYPE_UNSUPPORTED");
    }
    Ok(())
}
fn validate_constant(value: &str) -> Result<()> {
    let value = value.trim();
    ensure!(value.len() <= 256 && !value.is_empty(), "STRUCTURE_DEFAULT_UNSUPPORTED");
    if ["NULL", "TRUE", "FALSE", "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP"].iter().any(|k| value.eq_ignore_ascii_case(k)) { return Ok(()); }
    if value.starts_with('\'') {
        let b = value.as_bytes(); let mut i = 1;
        while i < b.len() {
            if b[i] == b'\'' { i += 1; if i < b.len() && b[i] == b'\'' { i += 1; continue; } ensure!(i == b.len(), "STRUCTURE_DEFAULT_UNSUPPORTED: literal 后还有 SQL"); return Ok(()); }
            i += 1;
        }
        return Err(anyhow!("STRUCTURE_DEFAULT_UNSUPPORTED: 未闭合 literal"));
    }
    let number = value.strip_prefix('+').or_else(|| value.strip_prefix('-')).unwrap_or(value);
    ensure!(!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit() || b == b'.') && number.bytes().filter(|b| *b == b'.').count() <= 1 && number.bytes().any(|b| b.is_ascii_digit()), "STRUCTURE_DEFAULT_UNSUPPORTED: 不执行自定义函数/表达式 default");
    Ok(())
}
fn safe_steps(current: &OwnedTableSnapshot, design: &TableDesign, steps: &[DesignStep]) -> Result<()> {
    ensure!(steps.len() <= 1024, "STRUCTURE_DDL_QUOTA");
    ensure!(design.schema == "main" && design.table == current.identity.table && design.foreign_keys.is_none(), "STRUCTURE_DDL_SCOPE_UNPROVEN");
    for column in design.columns.iter().filter(|column| column.orig_name.is_none()) {
        validate_type(&column.data_type)?;
        ensure!(column.length.as_deref().map_or(true, |v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())) && column.scale.as_deref().map_or(true, |v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())), "STRUCTURE_TYPE_UNSUPPORTED");
        if let Some(default) = &column.default_value { validate_constant(default)?; }
    }
    let table = metadata::ident(&design.table)?;
    let add_prefix = format!("ALTER TABLE {table} ADD COLUMN ");
    let indexes = design.indexes.as_deref().unwrap_or(&[]);
    for step in steps {
        let allowed = step.sql.starts_with(&add_prefix) || indexes.iter().any(|index| {
            if index.name.starts_with("sqlite_") { return false; }
            let Ok(name) = metadata::ident(&index.name) else { return false; };
            let prefix = format!("CREATE {}INDEX {name} ON {table} (", if index.unique { "UNIQUE " } else { "" });
            step.sql.starts_with(&prefix)
        });
        ensure!(allowed, "STRUCTURE_DDL_UNSUPPORTED: 只执行后端重编的增列/普通列索引，拒绝 DROP/rename/cascade/任意 SQL");
    }
    Ok(())
}
fn same<T: serde::Serialize>(a: &T, b: &T) -> Result<bool> { Ok(serde_json::to_value(a)? == serde_json::to_value(b)?) }
fn verify_preserved(before: &OwnedTableSnapshot, after: &OwnedTableSnapshot) -> Result<()> {
    ensure!(before.identity == after.identity, "STRUCTURE_READBACK_IDENTITY_CHANGED");
    ensure!(design::ordered_primary_key(&before.schema)? == design::ordered_primary_key(&after.schema)?, "STRUCTURE_READBACK_PRIMARY_KEY_CHANGED");
    for original in &before.schema.columns {
        let current = after.schema.columns.iter().find(|c| c.name == original.name).ok_or_else(|| anyhow!("STRUCTURE_READBACK_EXTRA_COLUMN_DROPPED"))?;
        ensure!(same(original, current)?, "STRUCTURE_READBACK_EXISTING_COLUMN_CHANGED: {}", original.name);
    }
    for original in &before.schema.indexes {
        let current = after.schema.indexes.iter().find(|i| i.name == original.name).ok_or_else(|| anyhow!("STRUCTURE_READBACK_EXTRA_INDEX_DROPPED"))?;
        ensure!(same(original, current)?, "STRUCTURE_READBACK_EXISTING_INDEX_CHANGED: {}", original.name);
    }
    ensure!(same(&before.schema.foreign_keys, &after.schema.foreign_keys)?, "STRUCTURE_READBACK_FOREIGN_KEY_CHANGED");
    ensure!(same(&before.objects.checks, &after.objects.checks)?, "STRUCTURE_READBACK_CHECK_CHANGED");
    ensure!(same(&before.objects.triggers, &after.objects.triggers)?, "STRUCTURE_READBACK_TRIGGER_CHANGED");
    Ok(())
}
async fn rollback(pair: &mut OwnedPair) -> Result<()> {
    // 停止 progress handler 不能中断必须确认的 ROLLBACK/读回。普通业务 SQL 此后不再发送。
    execution::clear_sqlite(&mut pair.target).await?;
    if let Some(source) = pair.source.as_mut() { execution::clear_sqlite(source).await?; }
    if pair.target_tx {
        crate::sql_history::query("ROLLBACK").execute(&mut *pair.target).await?;
        metadata::ensure_autocommit(&mut pair.target).await?; pair.target_tx = false;
    }
    if pair.source_tx { if let Some(source) = pair.source.as_mut() {
        crate::sql_history::query("ROLLBACK").execute(&mut **source).await?;
        metadata::ensure_autocommit(source).await?; pair.source_tx = false;
    } }
    Ok(())
}
fn rollback_receipt(report: StructureDiffReport, mut steps: Vec<DesignStepResult>, schema: Option<crate::models::TableSchema>, message: String) -> StructureSyncReceipt {
    for step in &mut steps { if step.status == "applied" { step.status = "rolled_back".into(); } }
    StructureSyncReceipt { report, receipt: DesignReceipt { steps, verified: false, unverified: false, mismatches: vec![message], transaction: "rolled_back".into(), schema } }
}
async fn apply_pair(pair: &mut OwnedPair, kind: &DbKind, request: &StructureSyncRequest, binding: &StructureReviewBinding, entry: &Arc<Entry>, mode: TestMode) -> Applied {
    let started = Instant::now();
    let mut saved_report: Option<StructureDiffReport> = None;
    let mut saved_target: Option<OwnedTableSnapshot> = None;
    let mut step_results = Vec::new();
    let result = async {
        if entry.stopping() { return Err(before_send()); }
        pair.begin(true).await?;
        let (source_before, target_before) = pair.snapshots(request).await?;
        let target_before = target_before.ok_or_else(|| anyhow!("STRUCTURE_TARGET_NOT_FOUND"))?;
        let mut report = structure_sync::preview_from_schemas(&source_before.schema, Some(&target_before.schema), kind, request, &source_before.revision, &target_before.revision)?;
        structure_sync::validate_review(request, &report, binding)?;
        structure_sync::bind_review(request, &mut report, binding)?;
        let (final_design, steps) = structure_sync::prepare_reviewed_design(request, &report, binding, &target_before.schema, kind)?;
        safe_steps(&target_before, &final_design, &steps)?;
        ensure!(!steps.is_empty(), "STRUCTURE_NO_CHANGES: 无 DDL 计划，不把空步骤当作应用成功");
        saved_report = Some(report.clone()); saved_target = Some(target_before.clone());
        for (ordinal, step) in steps.iter().enumerate() {
            if entry.stopping() { return Err(AppError::coded("STRUCTURE_STOPPED_ROLLED_BACK", "停止发生在 COMMIT 前，必须确认回滚").into()); }
            let outcome = crate::sql_history::query(&step.sql).execute(&mut *pair.target).await;
            match outcome {
                Ok(_) => step_results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }),
                Err(error) => {
                    step_results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(error.to_string()) });
                    for remaining in steps.iter().skip(ordinal + 1) { step_results.push(DesignStepResult { label: remaining.label.clone(), sql: remaining.sql.clone(), status: "skipped".into(), error: None }); }
                    return Err(error.into());
                }
            }
            #[cfg(test)] if ordinal == 0 {
                if matches!(mode, TestMode::FailAfterFirstDdl) { crate::sql_history::query("SELECT * FROM main.__navkit_missing_structure_test_table__").fetch_all(&mut *pair.target).await?; }
                if matches!(mode, TestMode::StopAfterFirstDdl) { entry.request_stop(false); }
            }
        }
        if entry.stopping() { return Err(AppError::coded("STRUCTURE_STOPPED_ROLLED_BACK", "停止发生在读回/COMMIT 前，必须确认回滚").into()); }
        let target_after = metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await?;
        let mismatches = design::verify(&target_after.schema, &final_design, kind);
        ensure!(mismatches.is_empty(), "STRUCTURE_READBACK_MISMATCH: {}", mismatches.join("；"));
        verify_preserved(&target_before, &target_after)?;
        pair.validate_paths()?;
        let source_after = if let Some(source) = pair.source.as_mut() { metadata::snapshot(source, &pair.source_physical, &request.source_schema, &request.source_table).await? }
            else { metadata::snapshot(&mut pair.target, &pair.target_physical, &request.source_schema, &request.source_table).await? };
        ensure!(source_before.identity == source_after.identity && source_before.base_revision == source_after.base_revision, "STRUCTURE_SOURCE_CHANGED_BEFORE_COMMIT");
        if !pair.same_database() { ensure!(source_before.revision == source_after.revision, "STRUCTURE_SOURCE_CHANGED_BEFORE_COMMIT"); }
        let violation = crate::sql_history::query("PRAGMA main.foreign_key_check").fetch_optional(&mut *pair.target).await?;
        ensure!(violation.is_none(), "STRUCTURE_READBACK_FOREIGN_KEY_VIOLATION: 已拒绝 COMMIT");
        if entry.stopping() { return Err(AppError::coded("STRUCTURE_STOPPED_ROLLED_BACK", "读回期间停止，未发送 COMMIT").into()); }
        // 内部源只持 reserved 锁/读 metadata；目标单次提交，不承诺跨数据库分布式原子事务。
        crate::sql_history::query("COMMIT").execute(&mut *pair.target).await.map_err(|error| effects_unknown(format!("STRUCTURE_COMMIT_UNKNOWN: 未确认 COMMIT，不重放/恢复，销毁 owned 连接：{error}")))?;
        pair.target_tx = false;
        #[cfg(test)] if matches!(mode, TestMode::LoseCommitAcknowledgement) {
            return Err(effects_unknown("STRUCTURE_COMMIT_UNKNOWN: 测试在真实 SQLite COMMIT 成功后丢弃 ACK，绝不发布成功 receipt"));
        }
        if entry.stopping() { return Err(effects_unknown("STRUCTURE_COMMIT_STOP_RACE: 停止与已发送 COMMIT 竞态，不能声称已回滚或重放")); }
        metadata::ensure_autocommit(&mut pair.target).await.map_err(|e| effects_unknown(format!("STRUCTURE_COMMIT_CLEANUP_UNKNOWN: {e:#}")))?;
        let receipt = StructureSyncReceipt { report, receipt: DesignReceipt { steps: step_results.clone(), verified: true, unverified: false, mismatches: Vec::new(), transaction: "committed".into(), schema: Some(target_after.schema) } };
        Ok::<_, anyhow::Error>((MultiResult { sets: Vec::new(), messages: vec!["SQLite 结构 DDL 已提交，并在同 owned 事务中核验列/有序键与既有 CHECK/触发器/外键保留；无跨库原子提交承诺".into()], elapsed_ms: started.elapsed().as_millis() as u64, spool_id: None }, receipt))
    }.await;
    #[cfg(not(test))] let _ = mode;
    match result {
        Ok((result, receipt)) => Applied { result: Ok(result), receipt: Some(receipt), destroy: false },
        Err(error) if is_unknown(&error) => Applied { result: Err(error), receipt: None, destroy: true },
        Err(error) => {
            if let Err(rollback_error) = rollback(pair).await {
                return Applied { result: Err(effects_unknown(format!("STRUCTURE_ROLLBACK_UNKNOWN: 原错误 {error:#}；回滚/清理未确认 {rollback_error:#}，不发布 receipt"))), receipt: None, destroy: true };
            }
            let message = format!("{error:#}");
            let rolled_back_schema = if let Some(before) = &saved_target {
                match metadata::snapshot(&mut pair.target, &pair.target_physical, &request.target_schema, &request.target_table).await {
                    Ok(current) if current.base_revision == before.base_revision && current.identity == before.identity => Some(current.schema),
                    _ => None,
                }
            } else { None };
            let receipt = saved_report.map(|report| rollback_receipt(report, step_results, rolled_back_schema, message.clone()));
            let error = if entry.stopping() && saved_target.is_some() { AppError::coded("STRUCTURE_STOPPED_ROLLED_BACK", format!("原生 DDL 停止且 ROLLBACK/排空已确认：{message}")) }
                else if error.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_CANCELLED_BEFORE_SEND") { AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", message) }
                else { AppError::coded("STRUCTURE_APPLY_FAILED_ROLLED_BACK", format!("未提交结构 DDL；ROLLBACK 已确认：{message}")) };
            Applied { result: Err(error.into()), receipt, destroy: false }
        }
    }
}

pub async fn run(source: Arc<Db>, source_kind: DbKind, target: Arc<Db>, target_kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease,
    request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool) -> BoundResult {
    run_mode(source, source_kind, target, target_kind, entry, ordinary, request, binding, read_only, TestMode::Normal).await
}
async fn run_mode(source: Arc<Db>, source_kind: DbKind, target: Arc<Db>, target_kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease,
    request: StructureSyncRequest, binding: StructureReviewBinding, read_only: bool, mode: TestMode) -> BoundResult {
    if let Err(error) = validate_entry(&entry, &request, &binding, read_only) { return BoundResult { result: Err(error), confirmed: false, receipt: None }; }
    if metadata::sqlite_pool(&source, &source_kind).is_err() || metadata::sqlite_pool(&target, &target_kind).is_err() {
        return BoundResult { result: Err(AppError::coded("STRUCTURE_NATIVE_EXECUTION_UNSUPPORTED", "当前受控 owned 通道只认证原生 SQLite，PG/MySQL/JDBC/H2 尚未实现此通道")), confirmed: false, receipt: None };
    }
    let published = Arc::new(parking_lot::Mutex::new(None::<StructureSyncReceipt>));
    let output = published.clone(); let work_entry = entry.clone();
    let (result, mut confirmed) = execution::supervise(entry.clone(), async move {
        // ordinary/activity 不能随 supervise 的有界 Unknown 回复提前释放；owned future 真排空才 Drop。
        let _ordinary = ordinary;
        if work_entry.stopping() || !work_entry.start() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "排队期间停止，未发送结构 SQL")), true); }
        let (result, proof) = execution::drive(async {
            let source_pool = metadata::sqlite_pool(&source, &source_kind)?; let target_pool = metadata::sqlite_pool(&target, &target_kind)?;
            let mut pair = metadata::acquire_pair(source_pool, target_pool, Some(&work_entry)).await?;
            let applied = apply_pair(&mut pair, &target_kind, &request, &binding, &work_entry, mode).await;
            let cleanup = pair.cleanup(applied.destroy).await;
            if let Err(error) = cleanup { return Err(effects_unknown(format!("STRUCTURE_OWNED_CLEANUP_UNKNOWN: callbacks/源锁/连接排空未确认：{error:#}"))); }
            if !applied.destroy { *output.lock() = applied.receipt; }
            applied.result
        }, &work_entry, || async { Ok(()) }).await;
        let result = result.map_err(AppError::from);
        let proof = proof || confirmed_stop(&result);
        (result, proof)
    }).await;
    confirmed = confirmed || confirmed_stop(&result);
    let can_publish = !entry.stopping() && result.is_ok() || result.as_ref().err().is_some_and(|e| e.code == "STRUCTURE_APPLY_FAILED_ROLLED_BACK" || e.code == "STRUCTURE_STOPPED_ROLLED_BACK");
    let receipt = if can_publish { published.lock().take() } else { None };
    BoundResult { result, confirmed, receipt }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{execution::{Binding, Registry}, execution_contracts::{ExecutionRequest, ExecutionState}, manual_transaction};
    use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

    fn request() -> StructureSyncRequest { StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: "src".into(), target_connection: "target".into(), target_schema: "main".into(), target_table: "dst".into(), columns: Vec::new(), allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() } }
    fn binding() -> StructureReviewBinding { StructureReviewBinding { principal: "window".into(), owner: "page".into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: "main".into(), source_table: "src".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: "main".into(), target_table: "dst".into() } }
    async fn pool() -> SqlitePool { SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap() }
    async fn exec(pool: &SqlitePool, sql: &str) { crate::sql_history::query(sql).execute(pool).await.unwrap(); }
    async fn fixture() -> SqlitePool {
        let pool = pool().await;
        exec(&pool, "CREATE TABLE src(a INT, added TEXT)").await;
        exec(&pool, "CREATE TABLE dst(a INT CHECK(a>=0), extra TEXT DEFAULT 'Keep  Case')").await;
        exec(&pool, "CREATE INDEX extra_idx ON dst(extra,a)").await;
        exec(&pool, "CREATE TABLE audit(value TEXT)").await;
        exec(&pool, "CREATE TRIGGER dst_audit AFTER INSERT ON dst BEGIN INSERT INTO audit(value) VALUES('Trigger  Case'); END").await;
        exec(&pool, "INSERT INTO dst(a,extra) VALUES(1,'original')").await;
        pool
    }
    async fn reviewed(source: Arc<Db>, target: Arc<Db>, gates: &manual_transaction::Registry) -> (StructureSyncRequest, StructureReviewBinding) {
        let mut request = request(); let binding = binding();
        let guard = gates.ordinary(&["source", "target"]).await.unwrap();
        let preview = metadata::preflight(source, DbKind::of("sqlite"), target, DbKind::of("sqlite"), guard, request.clone(), binding.clone(), false).await.unwrap();
        assert!(preview.report.alterable, "{:?}", preview.report.unsupported); assert!(!preview.report.review_digest.is_empty());
        request.source_revision = preview.report.source_revision; request.target_revision = preview.report.target_revision; request.review_digest = preview.report.review_digest;
        (request, binding)
    }
    fn new_entry(request: &StructureSyncRequest, binding: &StructureReviewBinding, timeout: u64) -> Arc<Entry> {
        let registry = Registry::default();
        let binding = Binding { principal: binding.principal.clone(), owner: binding.owner.clone(), connection: binding.target_connection_id.clone(), namespace: binding.target_schema.clone(), fingerprint: binding.target_fingerprint.clone() };
        let mut execution = ExecutionRequest { execution_id: String::new(), connection_id: request.target_connection.clone(), namespace: request.target_schema.clone(), owner_id: binding.owner.clone(), sql: structure_sync::review_request_sql(request).unwrap(), transaction_owner: None, timeout_ms: Some(timeout), origin: Some("structure_sync".into()), execution_grant: None, cancel_before_send: false };
        registry.grant_request(&mut execution, binding.clone()).unwrap(); registry.register(&execution, binding, 0).unwrap()
    }
    async fn observer(pool: &SqlitePool) -> sqlx::pool::PoolConnection<sqlx::Sqlite> {
        tokio::time::timeout(std::time::Duration::from_secs(2), pool.acquire()).await.expect("owned observer 必须能复用已排空的池").unwrap()
    }
    async fn definition(pool: &SqlitePool, table: &str) -> String {
        let mut connection = observer(pool).await;
        let result = crate::sql_history::query_scalar("SELECT sql FROM main.sqlite_schema WHERE name=?").bind(table).fetch_one(&mut *connection).await;
        connection.return_to_pool().await;
        result.unwrap()
    }
    async fn read_data(pool: &SqlitePool) -> (i64, String, Option<String>) {
        let mut connection = observer(pool).await;
        let result = crate::sql_history::query_as("SELECT a,extra,added FROM dst").fetch_one(&mut *connection).await;
        connection.return_to_pool().await;
        result.unwrap()
    }
    async fn call(pool: &SqlitePool, request: StructureSyncRequest, binding: StructureReviewBinding, gates: &manual_transaction::Registry, mode: TestMode) -> (Arc<Entry>, BoundResult) {
        let entry = new_entry(&request, &binding, 10_000); let db = Arc::new(Db::Sqlite(pool.clone()));
        let ordinary = gates.ordinary(&["source", "target"]).await.unwrap();
        let result = run_mode(db.clone(), DbKind::of("sqlite"), db, DbKind::of("sqlite"), entry.clone(), ordinary, request, binding, false, mode).await;
        (entry, result)
    }
    #[test]
    fn metadata_type_and_default_are_not_sql_injection_channels() {
        assert!(validate_type("VARCHAR(32)").is_ok()); assert!(validate_type("DECIMAL(20, 4)").is_ok());
        for t in ["TEXT); DROP TABLE x; --", "TEXT DEFAULT 1", "custom_function()", "INT(2) REFERENCES x(a)"] { assert!(validate_type(t).is_err(), "{t}"); }
        assert!(validate_constant("'Keep  Case'").is_ok()); assert!(validate_constant("'It''s literal'").is_ok());
        for value in ["'a' COLLATE nocase", "dangerous()", "NULL REFERENCES x(a)", "'' || load_extension('x')"] { assert!(validate_constant(value).is_err(), "{value}"); }
    }
    #[tokio::test]
    async fn same_pool_one_connection_commits_preserving_extra_data_index_check_trigger_and_review() {
        let pool = fixture().await; let gates = manual_transaction::Registry::default(); let db = Arc::new(Db::Sqlite(pool.clone()));
        let (request, binding) = reviewed(db.clone(), db, &gates).await; let original = request.clone();
        let source_definition = definition(&pool, "src").await;
        let (entry, result) = call(&pool, request, binding, &gates, TestMode::Normal).await;
        assert!(result.result.is_ok(), "{:?}", result.result.as_ref().err());
        let receipt = result.receipt.as_ref().unwrap(); assert!(receipt.receipt.verified); assert_eq!(receipt.receipt.transaction, "committed");
        assert_eq!(receipt.report.source_revision, original.source_revision); assert_eq!(receipt.report.target_revision, original.target_revision); assert_eq!(receipt.report.review_digest, original.review_digest);
        assert!(receipt.receipt.schema.as_ref().unwrap().columns.iter().any(|c| c.name == "added"));
        assert_eq!(definition(&pool, "src").await, source_definition);
        let data = read_data(&pool).await; assert_eq!(data, (1,"original".into(),None));
        let mut connection = observer(&pool).await;
        let index_count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='extra_idx'").fetch_one(&mut *connection).await.unwrap();
        connection.return_to_pool().await; assert_eq!(index_count,1);
        assert!(definition(&pool,"dst").await.contains("CHECK(a>=0)")); assert!(definition(&pool,"dst_audit").await.contains("Trigger  Case"));
        let outcome = entry.finish(result.result, result.confirmed, false); assert_eq!(outcome.state, ExecutionState::Succeeded); assert!(!outcome.resources_pending);
        assert!(pool.num_idle() > 0); let _lease = gates.ordinary(&["source","target"]).await.unwrap();
    }
    #[tokio::test]
    async fn sql_error_after_real_first_ddl_rolls_back_all_schema_and_keeps_memory_pool() {
        let pool = fixture().await; let gates = manual_transaction::Registry::default(); let db = Arc::new(Db::Sqlite(pool.clone()));
        let (request, binding) = reviewed(db.clone(), db, &gates).await; let before = definition(&pool,"dst").await;
        let (entry,result) = call(&pool,request,binding,&gates,TestMode::FailAfterFirstDdl).await;
        assert!(result.result.is_err()); let receipt = result.receipt.as_ref().unwrap(); assert_eq!(receipt.receipt.transaction,"rolled_back"); assert!(!receipt.receipt.verified);
        assert!(receipt.receipt.steps.iter().any(|s| s.status=="rolled_back")); assert_eq!(definition(&pool,"dst").await,before);
        let outcome=entry.finish(result.result,result.confirmed,false); assert_eq!(outcome.state,ExecutionState::Failed); assert!(!outcome.resources_pending);
        assert!(pool.num_idle() > 0);
    }
    #[tokio::test]
    async fn stop_after_real_first_ddl_is_confirmed_only_after_rollback_and_drain() {
        let pool=fixture().await; let gates=manual_transaction::Registry::default(); let db=Arc::new(Db::Sqlite(pool.clone()));
        let (request,binding)=reviewed(db.clone(),db,&gates).await; let before=definition(&pool,"dst").await;
        let (entry,result)=call(&pool,request,binding,&gates,TestMode::StopAfterFirstDdl).await;
        assert!(result.confirmed); assert_eq!(result.receipt.as_ref().unwrap().receipt.transaction,"rolled_back"); assert_eq!(definition(&pool,"dst").await,before);
        let outcome=entry.finish(result.result,result.confirmed,false); assert_eq!(outcome.state,ExecutionState::Cancelled); assert!(!outcome.resources_pending); let _lease=gates.ordinary(&["source","target"]).await.unwrap();
    }
    #[tokio::test]
    async fn stopped_before_pool_does_not_borrow_held_connection_or_modify_catalog() {
        let pool=fixture().await; let gates=manual_transaction::Registry::default(); let db=Arc::new(Db::Sqlite(pool.clone()));
        let (request,binding)=reviewed(db.clone(),db.clone(),&gates).await; let before=definition(&pool,"dst").await;
        let mut held=pool.acquire().await.unwrap(); let entry=new_entry(&request,&binding,1000); entry.request_stop(false);
        let ordinary=gates.ordinary(&["source","target"]).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(1),run(db.clone(),DbKind::of("sqlite"),db,DbKind::of("sqlite"),entry.clone(),ordinary,request,binding,false)).await.unwrap();
        assert!(result.confirmed); assert!(result.receipt.is_none()); held.return_to_pool().await; assert_eq!(definition(&pool,"dst").await,before);
        let outcome=entry.finish(result.result,result.confirmed,false); assert_eq!(outcome.state,ExecutionState::Cancelled); assert_eq!(outcome.effects,"no_effect");
    }
    #[tokio::test]
    async fn stale_target_or_source_revision_never_sends_ddl() {
        let pool=fixture().await; let gates=manual_transaction::Registry::default(); let db=Arc::new(Db::Sqlite(pool.clone()));
        let (request,binding)=reviewed(db.clone(),db,&gates).await;
        exec(&pool,"ALTER TABLE dst ADD COLUMN concurrent TEXT").await; let before=definition(&pool,"dst").await;
        let (entry,result)=call(&pool,request,binding,&gates,TestMode::Normal).await;
        assert!(result.result.is_err()); assert!(result.receipt.is_none()); assert_eq!(definition(&pool,"dst").await,before); assert!(!before.contains("added"));
        entry.finish(result.result,result.confirmed,false);
    }
    #[tokio::test]
    async fn readonly_and_changed_owner_fail_before_borrowing_held_pool() {
        let pool=fixture().await; let gates=manual_transaction::Registry::default(); let db=Arc::new(Db::Sqlite(pool.clone()));
        let (request,binding)=reviewed(db.clone(),db.clone(),&gates).await;
        let mut held=pool.acquire().await.unwrap(); let entry=new_entry(&request,&binding,1000); let ordinary=gates.ordinary(&["source","target"]).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(1),run(db.clone(),DbKind::of("sqlite"),db.clone(),DbKind::of("sqlite"),entry.clone(),ordinary,request.clone(),binding.clone(),true)).await.unwrap();
        assert_eq!(result.result.unwrap_err().code,"STRUCTURE_READ_ONLY"); assert!(result.receipt.is_none());
        let entry=new_entry(&request,&binding,1000); let mut altered=binding; altered.owner="other-page".into(); let ordinary=gates.ordinary(&["source","target"]).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(1),run(db.clone(),DbKind::of("sqlite"),db,DbKind::of("sqlite"),entry,ordinary,request,altered,false)).await.unwrap();
        assert!(result.result.is_err()); held.return_to_pool().await;
    }
    async fn files() -> (std::path::PathBuf, SqlitePool, SqlitePool) {
        use sqlx::sqlite::SqliteConnectOptions;
        let directory = std::env::temp_dir().join(format!("navkit_structure_execution_{}", uuid::Uuid::new_v4()));
        assert!(directory.is_absolute()); std::fs::create_dir_all(&directory).unwrap();
        let source = SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(directory.join("source.sqlite")).create_if_missing(true).foreign_keys(true)).await.unwrap();
        let target = SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(directory.join("target.sqlite")).create_if_missing(true).foreign_keys(true)).await.unwrap();
        exec(&source,"CREATE TABLE src(a INT,added TEXT)").await;
        exec(&target,"CREATE TABLE dst(a INT CHECK(a>=0),extra TEXT DEFAULT 'Keep  Case')").await;
        exec(&target,"CREATE INDEX extra_idx ON dst(extra,a)").await;
        exec(&target,"INSERT INTO dst(a,extra) VALUES(1,'original')").await;
        (directory,source,target)
    }
    async fn run_files(source: &SqlitePool, target: &SqlitePool, request: StructureSyncRequest, binding: StructureReviewBinding, gates: &manual_transaction::Registry, mode: TestMode) -> (Arc<Entry>, BoundResult) {
        let entry=new_entry(&request,&binding,10_000); let ordinary=gates.ordinary(&["source","target"]).await.unwrap();
        let result=tokio::time::timeout(std::time::Duration::from_secs(10),run_mode(Arc::new(Db::Sqlite(source.clone())),DbKind::of("sqlite"),Arc::new(Db::Sqlite(target.clone())),DbKind::of("sqlite"),entry.clone(),ordinary,request,binding,false,mode)).await.unwrap();
        (entry,result)
    }
    #[tokio::test]
    async fn distinct_actual_files_commit_target_once_and_leave_source_schema_and_data_unchanged() {
        let (directory,source,target)=files().await; let gates=manual_transaction::Registry::default();
        let (request,binding)=reviewed(Arc::new(Db::Sqlite(source.clone())),Arc::new(Db::Sqlite(target.clone())),&gates).await;
        let original=request.clone(); let source_before=definition(&source,"src").await;
        let (entry,result)=run_files(&source,&target,request,binding,&gates,TestMode::Normal).await;
        assert!(result.result.is_ok(),"{:?}",result.result.as_ref().err()); let receipt=result.receipt.as_ref().unwrap(); assert!(receipt.receipt.verified); assert_eq!(receipt.receipt.transaction,"committed");
        assert_eq!(receipt.report.source_revision,original.source_revision); assert_eq!(receipt.report.target_revision,original.target_revision); assert_eq!(receipt.report.review_digest,original.review_digest);
        assert!(definition(&target,"dst").await.contains("added")); assert_eq!(definition(&source,"src").await,source_before);
        let data = read_data(&target).await; assert_eq!(data,(1,"original".into(),None));
        let outcome=entry.finish(result.result,result.confirmed,false); assert_eq!(outcome.state,ExecutionState::Succeeded); assert!(!outcome.resources_pending); assert!(source.num_idle() > 0 && target.num_idle() > 0);
        let released=tokio::time::timeout(std::time::Duration::from_secs(1),gates.ordinary(&["source","target"])).await.unwrap().unwrap(); drop(released);
        source.close().await; target.close().await; std::fs::remove_dir_all(&directory).unwrap();
    }
    #[tokio::test]
    async fn real_commit_with_lost_ack_is_unknown_no_receipt_no_idle_connection_and_no_replay() {
        use sqlx::sqlite::SqliteConnectOptions;
        let (directory,source,target)=files().await; let gates=manual_transaction::Registry::default();
        let (request,binding)=reviewed(Arc::new(Db::Sqlite(source.clone())),Arc::new(Db::Sqlite(target.clone())),&gates).await;
        let (entry,result)=run_files(&source,&target,request,binding,&gates,TestMode::LoseCommitAcknowledgement).await;
        assert_eq!(result.result.as_ref().unwrap_err().code,"EXECUTION_EFFECTS_UNKNOWN"); assert!(!result.confirmed); assert!(result.receipt.is_none()); assert_eq!(target.num_idle(),0); assert_eq!(source.num_idle(),0);
        assert_eq!(target.size(),0,"旧 target owned 连接必须已物理关闭，不能只是仍被借出"); assert_eq!(source.size(),0,"旧 source owned 连接必须已物理关闭");
        let outcome=entry.finish(result.result,result.confirmed,true); assert_eq!(outcome.state,ExecutionState::Unknown); assert_eq!(outcome.transaction_state,"unknown"); assert_eq!(outcome.effects,"possibly_applied"); assert!(!outcome.resources_pending); assert!(outcome.result.is_none());
        let released=tokio::time::timeout(std::time::Duration::from_secs(1),gates.ordinary(&["source","target"])).await.unwrap().unwrap(); drop(released);
        // 独立重新连接观察真 COMMIT 后的 schema/data，绝不把这个证据改成原 C1 success 或重放 DDL。
        let observer=SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(directory.join("target.sqlite")).foreign_keys(true)).await.unwrap();
        let mut connection = self::observer(&observer).await;
        let count:i64=crate::sql_history::query_scalar("SELECT count(*) FROM pragma_table_xinfo('dst') WHERE name='added'").fetch_one(&mut *connection).await.unwrap(); assert_eq!(count,1);
        let data:(i64,String,Option<String>)=crate::sql_history::query_as("SELECT a,extra,added FROM dst").fetch_one(&mut *connection).await.unwrap(); assert_eq!(data,(1,"original".into(),None));
        connection.return_to_pool().await; assert!(observer.num_idle() > 0);
        assert_eq!(target.num_idle(),0); assert_eq!(source.num_idle(),0); assert_eq!(target.size(),0); assert_eq!(source.size(),0);
        observer.close().await; source.close().await; target.close().await; std::fs::remove_dir_all(&directory).unwrap();
    }
}
