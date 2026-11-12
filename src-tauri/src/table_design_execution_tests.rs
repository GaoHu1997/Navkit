//! Real SQLite executor regressions. No fake metadata, no mock transaction/authorization/stop result.
use super::*;
use crate::{execution::Registry, execution_contracts::ExecutionState, manual_transaction};
use sqlx::{sqlite::{SqliteConnectOptions, SqlitePoolOptions}, SqlitePool};

fn binding() -> Binding { Binding { principal: "actual-window".into(), owner: "editor-page".into(), connection: "sqlite-connection".into(), namespace: "main".into(), fingerprint: "a".repeat(64) } }
async fn exec(pool: &SqlitePool, sql: &str) { sqlx::query(sql).execute(pool).await.unwrap(); }
async fn seed(pool: &SqlitePool) {
    exec(pool, "PRAGMA foreign_keys=ON").await;
    exec(pool, "CREATE TABLE parent(a INTEGER NOT NULL,b INTEGER NOT NULL,PRIMARY KEY(a,b))").await;
    exec(pool, "CREATE TABLE t(id INTEGER NOT NULL,part INTEGER NOT NULL,a INTEGER NOT NULL,b INTEGER NOT NULL,n INTEGER NOT NULL,txt TEXT NOT NULL DEFAULT 'Keep  Case',PRIMARY KEY(id,part),CONSTRAINT valid_n CHECK(n>=0 AND length(txt)>0),FOREIGN KEY(a,b) REFERENCES parent(a,b) ON UPDATE CASCADE ON DELETE RESTRICT)").await;
    exec(pool, "CREATE INDEX keep_order ON t(txt,id)").await;
    exec(pool, "CREATE TABLE audit(value TEXT)").await;
    exec(pool, "CREATE TRIGGER t_audit AFTER INSERT ON t BEGIN INSERT INTO audit(value) VALUES('Trigger  Case'); END").await;
    exec(pool, "INSERT INTO parent VALUES(10,20)").await;
    exec(pool, "INSERT INTO t(id,part,a,b,n,txt) VALUES(1,1,10,20,1,'same'),(2,1,10,20,2,'same')").await;
}
async fn memory() -> SqlitePool { let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap(); seed(&pool).await; pool }
async fn opened(pool: &SqlitePool, gates: &manual_transaction::Registry) -> DesignObservation {
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    inspect(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), lease, "main".into(), "t".into(), binding()).await.unwrap()
}
fn add_column(design: &mut TableDesign) {
    design.columns.push(ColumnDesign { name: "added".into(), orig_name: None, data_type: "TEXT".into(), length: None, scale: None, nullable: false, default_value: Some("'Keep  Case'".into()), auto_increment: false, comment: None, unsigned: false, zerofill: false });
}
async fn reviewed(pool: &SqlitePool, gates: &manual_transaction::Registry, design: TableDesign) -> TypedReview {
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    preflight(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), lease, design, binding(), false).await.unwrap()
}
fn registered(review: &TypedReview, cancelled: bool) -> (Arc<Entry>, ExecutionRequest) {
    let registry = Registry::default(); let binding = binding();
    let mut request = ExecutionRequest { execution_id: String::new(), connection_id: binding.connection.clone(), namespace: binding.namespace.clone(), owner_id: binding.owner.clone(), sql: review.request_sql(), transaction_owner: None, timeout_ms: Some(10_000), origin: Some("table_design".into()), execution_grant: None, cancel_before_send: false };
    validate_request(review, &request, &binding).unwrap(); registry.grant_request(&mut request, binding.clone()).unwrap();
    request.cancel_before_send = cancelled;
    let entry = registry.register(&request, binding, 0).unwrap(); (entry, request)
}
async fn call(pool: &SqlitePool, gates: &manual_transaction::Registry, review: TypedReview, mode: TestMode) -> (Arc<Entry>, BoundResult) {
    let (entry, request) = registered(&review, false);
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    let result = run_mode(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), entry.clone(), lease, review, request, false, mode).await;
    (entry, result)
}
async fn definition(pool: &SqlitePool, name: &str) -> String { sqlx::query_scalar("SELECT sql FROM main.sqlite_schema WHERE name=?").bind(name).fetch_one(pool).await.unwrap() }
async fn count(pool: &SqlitePool, sql: &str) -> i64 { sqlx::query_scalar(sql).fetch_one(pool).await.unwrap() }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_owned_save_preserves_composite_keys_constraints_rows_and_bound_review() {
    let pool = memory().await; let gates = manual_transaction::Registry::default(); let observation = opened(&pool, &gates).await;
    assert_eq!(observation.design.primary_key, ["id", "part"]);
    assert_eq!(observation.snapshot.schema.foreign_keys[0].columns, ["a", "b"]);
    assert_eq!(observation.snapshot.schema.foreign_keys[0].ref_columns, ["a", "b"]);
    assert_eq!(observation.snapshot.objects.checks.len(), 1); assert_eq!(observation.snapshot.objects.triggers.len(), 1);
    let mut design = observation.design.clone(); add_column(&mut design);
    design.indexes.as_mut().unwrap().push(IndexDesign { name: "added_order".into(), columns: vec!["part".into(), "id".into(), "added".into()], unique: true, ..Default::default() });
    for bad in ["TEXT); DROP TABLE t; --", "TEXT DEFAULT 1", "TEXT CHECK(0)"] { let mut altered = design.clone(); altered.columns.last_mut().unwrap().data_type = bad.into(); assert!(compile(&observation.snapshot, &altered).is_err()); }
    for bad in ["(SELECT 1)", "dangerous()", "'safe' COLLATE nocase", "CURRENT_TIMESTAMP"] { let mut altered = design.clone(); altered.columns.last_mut().unwrap().default_value = Some(bad.into()); assert!(compile(&observation.snapshot, &altered).is_err()); }
    let mut reordered = design.clone(); reordered.primary_key.reverse(); assert!(compile(&observation.snapshot, &reordered).is_err());
    let mut changed_check = design.clone(); changed_check.checks.as_mut().unwrap()[0].expression.push_str(" OR 1"); assert!(compile(&observation.snapshot, &changed_check).is_err());
    let review = reviewed(&pool, &gates, design).await; let original_review = review.clone();
    assert_eq!(review.baseline().revision, observation.snapshot.revision); assert_eq!(review.design().base_revision.as_deref(), Some(observation.snapshot.revision.as_str()));
    let (sample_entry, sample_request) = registered(&review, true);
    let mut wrong = sample_request.clone(); wrong.sql.push_str(" ALTER TABLE t"); assert!(validate_request(&review, &wrong, &binding()).is_err());
    wrong = sample_request.clone(); wrong.transaction_owner = Some(wrong.owner_id.clone()); assert!(validate_request(&review, &wrong, &binding()).is_err());
    let mut different = binding(); different.principal = "other-window".into(); assert!(validate_request(&review, &sample_request, &different).is_err());
    different = binding(); different.fingerprint = "b".repeat(64); assert!(validate_request(&review, &sample_request, &different).is_err());
    let mut expired = review.clone(); expired.issued_at = Instant::now() - (REVIEW_TTL + Duration::from_secs(1)); assert_eq!(validate_request(&expired, &sample_request, &binding()).unwrap_err().code, "TABLE_DESIGN_REVIEW_EXPIRED");
    sample_entry.finish(Err(coded("EXECUTION_CANCELLED_BEFORE_SEND", "fixture was never sent")), true, false);
    let (denied_entry, denied_request) = registered(&review, false);
    let mut held = pool.acquire().await.unwrap(); let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    let denied = tokio::time::timeout(Duration::from_millis(500), run(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), denied_entry.clone(), lease, review.clone(), denied_request, true)).await.unwrap();
    assert_eq!(denied.result.as_ref().unwrap_err().code, "TABLE_DESIGN_READ_ONLY"); assert!(denied.receipt.is_none()); assert!(!review.used.load(Ordering::Acquire)); metadata::ensure_autocommit(&mut held).await.unwrap(); held.return_to_pool().await;
    let denied = denied_entry.finish(denied.result, denied.confirmed, false); assert_eq!(denied.state, ExecutionState::Failed); assert!(!denied.resources_pending);
    let trigger_before = definition(&pool, "t_audit").await;
    let (entry, result) = call(&pool, &gates, review, TestMode::Normal).await;
    assert!(result.result.is_ok(), "{:?}", result.result.as_ref().err());
    let receipt = result.receipt.as_ref().unwrap(); assert!(receipt.verified); assert!(!receipt.unverified); assert_eq!(receipt.transaction, "committed"); assert_eq!(receipt.steps.len(), 2);
    let snapshot = result.snapshot.as_ref().unwrap(); assert_ne!(snapshot.revision, observation.snapshot.revision);
    assert_eq!(crate::design::ordered_primary_key(&snapshot.schema).unwrap(), ["id", "part"]);
    assert_eq!(snapshot.schema.indexes.iter().find(|i| i.name == "added_order").unwrap().columns, ["part", "id", "added"]);
    assert!(same(&snapshot.schema.foreign_keys, &observation.snapshot.schema.foreign_keys).unwrap());
    assert!(same(&snapshot.objects.checks, &observation.snapshot.objects.checks).unwrap()); assert!(same(&snapshot.objects.triggers, &observation.snapshot.objects.triggers).unwrap());
    assert_eq!(definition(&pool, "t_audit").await, trigger_before);
    assert_eq!(count(&pool, "SELECT count(*) FROM t WHERE added='Keep  Case' AND txt='same'").await, 2);
    assert_eq!(count(&pool, "SELECT count(*) FROM audit WHERE value='Trigger  Case'").await, 2);
    let mut conn = pool.acquire().await.unwrap(); metadata::ensure_autocommit(&mut conn).await.unwrap(); assert!(sqlx::query("PRAGMA main.foreign_key_check").fetch_optional(&mut *conn).await.unwrap().is_none()); conn.return_to_pool().await;
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Succeeded); assert_eq!(finished.transaction_state, "committed"); assert!(!finished.resources_pending);
    let (entry, request) = {
        let registry = Registry::default(); let binding = binding(); let mut request = sample_request; request.execution_grant = None; request.cancel_before_send = false;
        registry.grant_request(&mut request, binding.clone()).unwrap(); let entry = registry.register(&request, binding, 0).unwrap(); (entry, request)
    };
    let held = pool.acquire().await.unwrap(); let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    let replay = tokio::time::timeout(Duration::from_millis(500), run(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), entry.clone(), lease, original_review, request, false)).await.unwrap();
    assert_eq!(replay.result.as_ref().unwrap_err().code, "TABLE_DESIGN_REVIEW_USED"); assert!(replay.receipt.is_none()); entry.finish(replay.result, replay.confirmed, false); drop(held);
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_unique_failure_cancellation_and_changed_catalog_never_leave_partial_ddl() {
    let pool = memory().await; let gates = manual_transaction::Registry::default(); let observation = opened(&pool, &gates).await; let before = definition(&pool, "t").await;
    let mut design = observation.design; add_column(&mut design);
    design.indexes.as_mut().unwrap().push(IndexDesign { name: "duplicate_txt".into(), columns: vec!["txt".into()], unique: true, ..Default::default() });
    let review = reviewed(&pool, &gates, design).await; let (entry, result) = call(&pool, &gates, review, TestMode::Normal).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "TABLE_DESIGN_FAILED_ROLLED_BACK");
    let receipt = result.receipt.as_ref().unwrap(); assert_eq!(receipt.transaction, "rolled_back"); assert!(!receipt.verified);
    assert_eq!(receipt.steps[0].status, "rolled_back"); assert_eq!(receipt.steps[1].status, "failed");
    assert_eq!(definition(&pool, "t").await, before); assert_eq!(count(&pool, "SELECT count(*) FROM sqlite_schema WHERE name='duplicate_txt'").await, 0); assert_eq!(count(&pool, "SELECT count(*) FROM t").await, 2);
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Failed); assert_eq!(finished.transaction_state, "rolled_back"); assert_eq!(finished.effects, "no_effect"); assert!(!finished.resources_pending);
    let mut design = opened(&pool, &gates).await.design; add_column(&mut design); let review = reviewed(&pool, &gates, design).await;
    let (entry, result) = call(&pool, &gates, review, TestMode::StopAfterFirstDdl).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "TABLE_DESIGN_STOPPED_ROLLED_BACK"); assert!(result.confirmed); assert_eq!(result.receipt.as_ref().unwrap().transaction, "rolled_back"); assert_eq!(definition(&pool, "t").await, before);
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Cancelled); assert_eq!(finished.transaction_state, "rolled_back"); assert_eq!(finished.effects, "no_effect"); assert!(!finished.resources_pending);
    let mut design = opened(&pool, &gates).await.design; add_column(&mut design); let review = reviewed(&pool, &gates, design.clone()).await;
    exec(&pool, "CREATE TRIGGER t_changed AFTER UPDATE ON t BEGIN INSERT INTO audit(value) VALUES('concurrent'); END").await;
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    let stale = preflight(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), lease, design, binding(), false).await.unwrap_err(); assert_eq!(stale.code, "TABLE_DESIGN_CONFLICT");
    let (entry, result) = call(&pool, &gates, review, TestMode::Normal).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "TABLE_DESIGN_CONFLICT"); assert_eq!(definition(&pool, "t").await, before);
    assert_eq!(count(&pool, "SELECT count(*) FROM sqlite_schema WHERE name='t_changed'").await, 1);
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Failed); assert_eq!(finished.transaction_state, "rolled_back");
    let observation = opened(&pool, &gates).await; let mut design = observation.design; add_column(&mut design); let review = reviewed(&pool, &gates, design).await;
    let (entry, request) = registered(&review, true); let mut held = pool.acquire().await.unwrap(); let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap();
    let result = tokio::time::timeout(Duration::from_millis(500), run(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), entry.clone(), lease, review, request, false)).await.unwrap();
    assert!(result.confirmed); assert!(result.receipt.is_none()); assert_eq!(result.result.as_ref().unwrap_err().code, "EXECUTION_CANCELLED_BEFORE_SEND"); metadata::ensure_autocommit(&mut held).await.unwrap(); held.return_to_pool().await;
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Cancelled); assert_eq!(finished.effects, "no_effect"); assert!(!finished.resources_pending);
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap(); drop(lease); pool.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_commit_with_lost_ack_is_unknown_and_its_review_cannot_be_replayed() {
    let root = std::env::temp_dir().join(format!("navkit-table-design-owned-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&root).unwrap();
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(SqliteConnectOptions::new().filename(root.join("database.sqlite")).create_if_missing(true).foreign_keys(true)).await.unwrap();
    seed(&pool).await; let gates = manual_transaction::Registry::default(); let mut design = opened(&pool, &gates).await.design; add_column(&mut design);
    let review = reviewed(&pool, &gates, design).await; let replay = review.clone();
    let (entry, result) = call(&pool, &gates, review, TestMode::LoseCommitAcknowledgement).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "EXECUTION_EFFECTS_UNKNOWN"); assert!(!result.confirmed); assert!(result.receipt.is_none()); assert!(result.snapshot.is_none());
    let finished = entry.finish(result.result, result.confirmed, false); assert_eq!(finished.state, ExecutionState::Unknown); assert_eq!(finished.transaction_state, "unknown"); assert!(finished.result.is_none()); assert_eq!(finished.effects, "possibly_applied"); assert!(!finished.resources_pending);
    // The failed-ACK connection was physically destroyed; a new real connection proves the original COMMIT happened.
    assert_eq!(count(&pool, "SELECT count(*) FROM t WHERE added='Keep  Case'").await, 2);
    let mut conn = pool.acquire().await.unwrap(); metadata::ensure_autocommit(&mut conn).await.unwrap(); conn.return_to_pool().await;
    let registry = Registry::default(); let binding = binding();
    let mut request = ExecutionRequest { execution_id: String::new(), connection_id: binding.connection.clone(), namespace: binding.namespace.clone(), owner_id: binding.owner.clone(), sql: replay.request_sql(), transaction_owner: None, timeout_ms: Some(10_000), origin: Some("table_design".into()), execution_grant: None, cancel_before_send: false };
    registry.grant_request(&mut request, binding.clone()).unwrap(); let entry = registry.register(&request, binding, 0).unwrap();
    let lease = gates.ordinary(&["sqlite-connection"]).await.unwrap(); let result = run(Arc::new(Db::Sqlite(pool.clone())), DbKind::of("sqlite"), entry.clone(), lease, replay, request, false).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "TABLE_DESIGN_REVIEW_USED"); assert!(result.receipt.is_none()); entry.finish(result.result, result.confirmed, false);
    pool.close().await;
    let canonical = std::fs::canonicalize(&root).unwrap(); assert!(canonical.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap())); assert!(canonical.file_name().unwrap().to_string_lossy().starts_with("navkit-table-design-owned-")); std::fs::remove_dir_all(canonical).unwrap();
}
