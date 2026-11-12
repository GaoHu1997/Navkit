//! Real temporary SQLite fixtures for object_mutation; no shared product changes.
use super::*;
use crate::{activity::ActivityGate, execution::Registry as ExecutionRegistry,
    execution_contracts::{ExecutionRequest, ExecutionState}, object_catalog::ObjectParent};
use sqlx::sqlite::{SqliteJournalMode, SqlitePoolOptions};
use std::path::PathBuf;

/// Exact production services, Store fingerprint/runtime admission and C1 grant path.
struct ProductionFixture { f: Fixture, state: Arc<crate::commands::AppState> }
impl ProductionFixture {
    async fn new() -> Self {
        let mut f = Fixture::new().await;
        let store = crate::storage::Store::empty_at(&f.root);
        let cfg = crate::models::ConnectionConfig {
            id: f.binding.connection.clone(), name: "真实对象DROP服务测试".into(), kind: DbKind::of("sqlite"),
            host: String::new(), port: 0, user: String::new(), password: String::new(), database: "main".into(),
            file: f.root.join("objects.sqlite").to_string_lossy().into_owned(), params: String::new(),
            group: String::new(), read_only: false, keep_alive: false,
        };
        store.save_connections(vec![cfg.clone()], vec![]).unwrap();
        let state = Arc::new(crate::commands::AppState {
            store, registry: Arc::new(crate::db::DbRegistry::default()), manual_transactions: Default::default(),
            activity: Default::default(), executions: Default::default(), mutations: Default::default(),
            connection_runtime: Default::default(), query_results: Default::default(),
        });
        let db = Arc::new(crate::db::open_saved(&crate::db::request_from_config(&cfg, ""), false, &state.store.settings()).await.unwrap());
        f.binding = crate::commands::execution_binding(&state, &cfg, "actual-window", "object-page", "main").unwrap();
        state.connection_runtime.publish(&cfg.id, &f.binding.fingerprint, &db, None,
            crate::db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
        state.registry.write().insert(cfg.id, db);
        Self { f, state }
    }
    async fn prepared(&self) -> (ObjectMutationBaseline, ObjectMutationWork, String) {
        let opened = crate::object_commands::read_object_mutation_baseline_service(&self.state,
            &self.f.binding.principal, self.f.object(), &self.f.binding.owner).await.unwrap();
        assert_eq!(opened.source_fingerprint, self.f.binding.fingerprint);
        let draft = ObjectMutationDraft { baseline_id: opened.baseline_id.clone(), object: opened.object.clone(),
            source_fingerprint: opened.source_fingerprint.clone(), base_revision: opened.base_revision.clone(),
            operation: ObjectMutationOperation::Drop };
        let preview = crate::object_commands::preview_object_mutation_service(&self.state,
            &self.f.binding.principal, draft.clone(), &self.f.binding.owner).await.unwrap();
        (opened, ObjectMutationWork { plan_id: preview.plan_id, review_digest: preview.review_digest, draft, confirm_drop: true }, preview.request_sql)
    }
    fn signed(&self, marker: &str) -> ExecutionRequest {
        let mut request = ExecutionRequest { execution_id: String::new(), connection_id: self.f.binding.connection.clone(),
            namespace: "main".into(), owner_id: self.f.binding.owner.clone(), sql: marker.into(),
            transaction_owner: None, timeout_ms: Some(5_000), origin: Some("object_mutation".into()),
            execution_grant: None, cancel_before_send: false };
        crate::commands::grant_execution_request(&self.state, &mut request, &self.f.binding.principal).unwrap(); request
    }
    async fn execute(&self, work: ObjectMutationWork, request: ExecutionRequest) -> CmdResult<crate::object_commands::ObjectMutationResponse> {
        tokio::time::timeout(Duration::from_secs(15), crate::object_commands::execute_object_mutation_service(
            &self.state, &self.f.binding.principal, work, request)).await.expect("production object DROP failed to drain")
    }
    async fn cleanup(self) {
        assert!(!self.state.executions.has_active(&self.f.binding.connection));
        drop(self.state.manual_transactions.close_all().await.unwrap());
        let connections: Vec<_> = self.state.registry.write().drain().map(|(_, db)| db).collect();
        for connection in connections { if let Db::Sqlite(pool) = connection.as_ref() {
            pool.close().await; assert!(pool.is_closed()); assert!(pool.acquire().await.is_err());
        } }
        drop(self.state); self.f.cleanup().await;
    }
}

#[tokio::test]
async fn production_ipc_services_use_saved_endpoint_c1_grant_and_verified_real_drop() {
    let p = ProductionFixture::new().await; let (opened, work, marker) = p.prepared().await;
    let request = p.signed(&marker); let execution_id = request.execution_id.clone();
    let response = p.execute(work.clone(), request.clone()).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error);
    assert_eq!(response.outcome.transaction_state, "committed"); assert!(!response.outcome.resources_pending);
    let receipt = response.receipt.unwrap(); assert!(receipt.verified);
    assert_eq!(receipt.before_revision, opened.base_revision); assert_eq!(receipt.backend_object_id, opened.backend_object_id);
    assert!(p.state.executions.entry_for_test(&execution_id).is_some()); assert!(!p.f.present().await);
    p.f.sql("INSERT INTO parent(value) VALUES('production after')").await;
    assert_eq!(p.f.scalar("SELECT count(*) FROM audit WHERE label='target'").await, 0);
    assert_eq!(p.f.scalar("SELECT count(*) FROM audit WHERE label='sibling'").await, 1);
    assert_eq!(p.f.scalar("SELECT count(*) FROM kept_view").await, 1);
    assert!(p.execute(work, request).await.is_err(), "consumed plan cannot replay through IPC service"); p.cleanup().await;
}

#[tokio::test]
async fn production_validate_before_registration_preserves_valid_grant_and_original_plan() {
    let p = ProductionFixture::new().await; let (_, work, marker) = p.prepared().await; let request = p.signed(&marker);
    for part in 0..4 {
        let mut wrong = work.clone();
        match part { 0 => wrong.confirm_drop = false, 1 => wrong.draft.object.parent.as_mut().unwrap().name = "audit".into(),
            2 => wrong.review_digest = "a".repeat(64), _ => wrong.draft.source_fingerprint = "b".repeat(64) }
        assert!(p.execute(wrong, request.clone()).await.is_err());
        assert!(p.state.executions.entry_for_test(&request.execution_id).is_none(), "invalid work must not consume grant/register Entry");
        assert!(p.f.present().await);
    }
    let response = p.execute(work, request).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error); assert!(response.receipt.unwrap().verified);
    p.cleanup().await;
}

#[tokio::test]
async fn production_missing_or_tampered_grant_cannot_send_or_consume_drop_plan() {
    let p = ProductionFixture::new().await; let (_, work, marker) = p.prepared().await; let request = p.signed(&marker);
    for token in [None, Some("tampered-grant".to_string())] {
        let mut wrong = request.clone(); wrong.execution_grant = token;
        let error = match p.execute(work.clone(), wrong).await { Err(e) => e, Ok(_) => panic!("invalid grant accepted") };
        assert_eq!(error.code, "EXECUTION_GRANT_INVALID"); assert!(p.f.present().await);
        validate_request(&work, &p.f.binding, &marker).unwrap();
    }
    let response = p.execute(work, request).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error); p.cleanup().await;
}

#[tokio::test]
async fn production_sibling_directory_change_rejects_original_opened_cas_without_refresh() {
    let p = ProductionFixture::new().await; let (_, work, marker) = p.prepared().await;
    p.f.sql("CREATE INDEX external_directory_change ON audit(label)").await; let before = p.f.scalar("PRAGMA main.schema_version").await;
    let error = crate::object_commands::preview_object_mutation_service(&p.state, &p.f.binding.principal,
        work.draft.clone(), &p.f.binding.owner).await.unwrap_err(); assert_eq!(error.code, "OBJECT_MUTATION_BASELINE_STALE");
    let response = p.execute(work.clone(), p.signed(&marker)).await.unwrap();
    assert_ne!(response.outcome.state, ExecutionState::Succeeded); assert_eq!(response.outcome.effects, "no_effect");
    assert_eq!(response.outcome.transaction_state, "rolled_back"); assert!(response.receipt.is_none()); assert!(p.f.present().await);
    assert_eq!(p.f.scalar("PRAGMA main.schema_version").await, before);
    assert_eq!(p.f.scalar("SELECT count(*) FROM main.sqlite_schema WHERE name='external_directory_change'").await, 1);
    assert!(validate_request(&work, &p.f.binding, &marker).is_err()); p.cleanup().await;
}

#[tokio::test]
async fn production_window_page_and_saved_endpoint_changes_never_consume_original_grant() {
    let p = ProductionFixture::new().await; let (_, work, marker) = p.prepared().await; let request = p.signed(&marker);
    let foreign = crate::object_commands::execute_object_mutation_service(&p.state, "foreign-window", work.clone(), request.clone()).await;
    assert!(foreign.is_err()); assert!(p.f.present().await);
    let mut foreign_page = request.clone(); foreign_page.owner_id = "foreign-page".into();
    assert!(p.execute(work.clone(), foreign_page).await.is_err());
    let original = p.state.store.connections(); let mut rebound = original.clone();
    rebound[0].file = p.f.root.join("different-physical.sqlite").to_string_lossy().into_owned();
    p.state.store.save_connections(rebound, vec![]).unwrap();
    assert!(p.execute(work.clone(), request.clone()).await.is_err()); assert!(p.f.present().await);
    assert!(!p.f.root.join("different-physical.sqlite").exists(), "endpoint change must be rejected before opening/sending to replacement");
    let mut readonly = original.clone(); readonly[0].read_only = true;
    p.state.store.save_connections(readonly, vec![]).unwrap();
    let error = crate::object_commands::read_object_mutation_baseline_service(&p.state, &p.f.binding.principal, p.f.object(), &p.f.binding.owner).await.unwrap_err();
    assert_eq!(error.code, "READ_ONLY_VIOLATION");
    assert!(p.state.executions.entry_for_test(&request.execution_id).is_none());
    p.state.store.save_connections(original, vec![]).unwrap();
    let response = p.execute(work, request).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error); p.cleanup().await;
}

#[tokio::test]
async fn unknown_after_real_commit_keeps_background_connection_and_guards_until_drain() {
    let f = Fixture::new().await; let (_, work, marker) = f.prepared().await; let entry = f.entry(&marker, false);
    let gate = Arc::new(CommitGate { reached: tokio::sync::Semaphore::new(0), release: tokio::sync::Semaphore::new(0) });
    let ordinary = f.manual.ordinary(&[&f.binding.connection]).await.unwrap();
    let task = tokio::spawn(run_inner(f.db.clone(), DbKind::of("sqlite"), entry.clone(), ordinary,
        work.clone(), marker.clone(), false, CommitMode::HoldAfterCommit(gate.clone())));
    let reached = tokio::time::timeout(Duration::from_secs(5), gate.reached.acquire()).await.expect("real COMMIT not reached").unwrap();
    reached.forget(); entry.request_stop(false);
    let result = tokio::time::timeout(Duration::from_secs(15), task).await.expect("bounded supervision did not return").unwrap();
    assert_eq!(result.result.as_ref().unwrap_err().code, "EXECUTION_EFFECTS_UNKNOWN");
    assert!(!result.no_effect); assert!(result.receipt.is_none()); assert_eq!(result.transaction, "unknown");
    let outcome = entry.finish(result.result, result.confirmed, true);
    assert_eq!(outcome.state, ExecutionState::Unknown); assert!(outcome.resources_pending);
    assert!(f.manual.try_ordinary(&[&f.binding.connection]).await.unwrap().is_none());
    assert!(f.activity.exclusive().is_err()); assert!(f.pool.try_acquire().is_none());
    assert!(validate_request(&work, &f.binding, &marker).is_err(), "unknown cannot replay while worker owns resources");
    gate.release.add_permits(1); f.executions.shutdown().await.unwrap();
    assert_eq!(entry.status().state, ExecutionState::Unknown, "late drain cannot rewrite terminal outcome");
    assert!(!entry.status().resources_pending); assert!(f.activity.exclusive().is_ok());
    assert!(f.manual.try_ordinary(&[&f.binding.connection]).await.unwrap().is_some()); assert!(!f.present().await);
    f.sql("INSERT INTO parent(value) VALUES('after unknown drain')").await;
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='target'").await, 0);
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='sibling'").await, 1); f.cleanup().await;
}

const TARGET: &str = "target \" ; -- trigger";
fn q(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
fn trigger_sql() -> String {
    format!("CREATE TRIGGER {} AFTER INSERT ON parent BEGIN INSERT INTO audit(label) VALUES('target'); END", q(TARGET))
}
struct Fixture {
    root: PathBuf,
    options: SqliteConnectOptions,
    pool: SqlitePool,
    db: Arc<Db>,
    binding: Binding,
    manual: crate::manual_transaction::Registry,
    activity: ActivityGate,
    executions: ExecutionRegistry,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("navkit-object-drop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("objects.sqlite");
        let options = SqliteConnectOptions::new().filename(&path).create_if_missing(true)
            .foreign_keys(true).journal_mode(SqliteJournalMode::Delete);
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options.clone()).await.unwrap();
        let db = Arc::new(Db::Sqlite(pool.clone()));
        let binding = Binding { principal: "actual-window".into(), owner: "object-page".into(),
            connection: uuid::Uuid::new_v4().to_string(), namespace: "main".into(),
            fingerprint: revision(&path.to_string_lossy().to_string()) };
        let fixture = Self { root, options, pool, db, binding, manual: Default::default(),
            activity: Default::default(), executions: Default::default() };
        fixture.sql("CREATE TABLE parent(id INTEGER PRIMARY KEY, value TEXT)").await;
        fixture.sql("CREATE TABLE audit(label TEXT)").await;
        fixture.sql(&trigger_sql()).await;
        fixture.sql("CREATE TRIGGER sibling AFTER INSERT ON parent BEGIN INSERT INTO audit(label) VALUES('sibling'); END").await;
        fixture.sql("CREATE VIEW kept_view AS SELECT id,value FROM parent").await;
        fixture
    }
    fn object(&self) -> DatabaseObjectId {
        DatabaseObjectId { connection_id: self.binding.connection.clone(), catalog: None,
            schema: "main".into(), kind: "trigger".into(), name: TARGET.into(), identity_arguments: None,
            parent: Some(ObjectParent { schema: "main".into(), kind: "table".into(), name: "parent".into() }) }
    }
    async fn sql(&self, sql: &str) {
        let mut conn = self.pool.acquire().await.unwrap();
        sqlx::query(sql).execute(&mut *conn).await.unwrap();
        conn.return_to_pool().await;
    }
    async fn scalar(&self, sql: &str) -> i64 {
        let mut conn = self.pool.acquire().await.unwrap();
        let value = sqlx::query_scalar::<_, i64>(sql).fetch_one(&mut *conn).await.unwrap();
        conn.return_to_pool().await; value
    }
    async fn present(&self) -> bool {
        let mut conn = self.pool.acquire().await.unwrap();
        let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND name=? AND tbl_name='parent'")
            .bind(TARGET).fetch_one(&mut *conn).await.unwrap();
        conn.return_to_pool().await; count == 1
    }
    async fn opened(&self) -> ObjectMutationBaseline {
        let ordinary = self.manual.ordinary(&[&self.binding.connection]).await.unwrap();
        baseline(self.db.clone(), DbKind::of("sqlite"), self.binding.clone(), self.object(),
            ordinary, self.activity.enter().unwrap(), false).await.unwrap()
    }
    async fn prepared(&self) -> (ObjectMutationBaseline, ObjectMutationWork, String) {
        let opened = self.opened().await;
        let draft = ObjectMutationDraft { baseline_id: opened.baseline_id.clone(), object: opened.object.clone(),
            source_fingerprint: opened.source_fingerprint.clone(), base_revision: opened.base_revision.clone(),
            operation: ObjectMutationOperation::Drop };
        let ordinary = self.manual.ordinary(&[&self.binding.connection]).await.unwrap();
        let preview = preflight(self.db.clone(), DbKind::of("sqlite"), self.binding.clone(), draft.clone(),
            ordinary, self.activity.enter().unwrap(), false).await.unwrap();
        let work = ObjectMutationWork { plan_id: preview.plan_id, review_digest: preview.review_digest,
            draft, confirm_drop: true };
        let marker = request_sql(&work, &self.binding).unwrap();
        assert_eq!(marker, preview.request_sql);
        (opened, work, marker)
    }
    fn entry(&self, marker: &str, cancelled: bool) -> Arc<Entry> {
        let mut request = ExecutionRequest { execution_id: String::new(), connection_id: self.binding.connection.clone(),
            namespace: self.binding.namespace.clone(), owner_id: self.binding.owner.clone(), sql: marker.into(),
            transaction_owner: None, timeout_ms: Some(5_000), origin: Some("object-drop-test".into()),
            execution_grant: None, cancel_before_send: false };
        self.executions.grant_request(&mut request, self.binding.clone()).unwrap();
        request.cancel_before_send = cancelled;
        let entry = self.executions.register(&request, self.binding.clone(), 30).unwrap();
        entry.own_activity(self.activity.enter().unwrap()); entry
    }
    async fn execute(&self, work: ObjectMutationWork, marker: String, entry: Arc<Entry>, mode: CommitMode) -> BoundResult {
        let ordinary = self.manual.ordinary(&[&self.binding.connection]).await.unwrap();
        run_inner(self.db.clone(), DbKind::of("sqlite"), entry, ordinary, work, marker, false, mode).await
    }
    async fn cleanup(self) {
        global().clear_connection(&self.binding.connection);
        let Self { root, pool, db, .. } = self;
        drop(db); pool.close().await;
        // 关键不变量：关闭态拒绝新连接；物理连接数取决于其它 Arc 持有者释放时序（Windows 异步回收）。
        assert!(pool.is_closed()); assert!(pool.acquire().await.is_err());
        drop(pool);
        // A single strict deletion: no retries, sleeps or softening Windows handle failures.
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn sqlite_drop_quoted_trigger_preserves_parent_all_other_ddl_and_real_behavior() {
    let f = Fixture::new().await;
    f.sql("INSERT INTO parent(value) VALUES('before')").await;
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='target'").await, 1);
    let (opened, work, marker) = f.prepared().await;
    let ordinary = f.manual.ordinary(&[&f.binding.connection]).await.unwrap();
    let repeated = preflight(f.db.clone(), DbKind::of("sqlite"), f.binding.clone(), work.draft.clone(),
        ordinary, f.activity.enter().unwrap(), false).await.unwrap();
    assert_eq!(repeated.plan_id, work.plan_id);
    assert!(repeated.expires_in_ms <= opened.expires_in_ms);
    let entry = f.entry(&marker, false);
    let result = f.execute(work.clone(), marker.clone(), entry.clone(), CommitMode::Normal).await;
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert!(!result.no_effect); assert_eq!(result.transaction, "committed");
    let receipt = result.receipt.unwrap();
    assert!(receipt.verified); assert_eq!(receipt.status, "applied");
    assert_eq!(receipt.before_revision, opened.base_revision);
    assert_eq!(receipt.backend_object_id, opened.backend_object_id);
    assert_ne!(receipt.after_revision, receipt.before_revision);
    let outcome = entry.finish(result.result, result.confirmed, true);
    assert_eq!(outcome.state, ExecutionState::Succeeded); assert!(!outcome.resources_pending);
    assert!(!f.present().await);
    f.sql("INSERT INTO parent(value) VALUES('after')").await;
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='target'").await, 1);
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='sibling'").await, 2);
    assert_eq!(f.scalar("SELECT count(*) FROM kept_view").await, 2);
    assert!(validate_request(&work, &f.binding, &marker).is_err(), "one-shot plan must reject replay");
    f.cleanup().await;
}

#[tokio::test]
async fn stale_same_name_same_ddl_recreation_rolls_back_without_deleting_replacement() {
    let f = Fixture::new().await;
    let (_, work, marker) = f.prepared().await;
    let before = f.scalar("PRAGMA main.schema_version").await;
    f.sql(&format!("DROP TRIGGER main.{}", q(TARGET))).await;
    f.sql(&trigger_sql()).await;
    let after_external = f.scalar("PRAGMA main.schema_version").await;
    assert_ne!(before, after_external);
    let entry = f.entry(&marker, false);
    let result = f.execute(work.clone(), marker.clone(), entry.clone(), CommitMode::Normal).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "OBJECT_MUTATION_ROLLED_BACK");
    assert!(result.result.as_ref().unwrap_err().message.contains("OBJECT_MUTATION_BASELINE_STALE"));
    assert!(result.no_effect); assert_eq!(result.transaction, "rolled_back"); assert!(result.receipt.is_none());
    let outcome = entry.finish(result.result, result.confirmed, true);
    assert_ne!(outcome.state, ExecutionState::Succeeded); assert!(!outcome.resources_pending);
    assert!(f.present().await);
    assert_eq!(f.scalar("PRAGMA main.schema_version").await, after_external);
    f.sql("INSERT INTO parent(value) VALUES('replacement survives')").await;
    assert_eq!(f.scalar("SELECT count(*) FROM audit WHERE label='target'").await, 1);
    assert!(validate_request(&work, &f.binding, &marker).is_err());
    f.cleanup().await;
}

#[tokio::test]
async fn foreign_binding_tampered_draft_unconfirmed_and_wrong_marker_never_consume_plan() {
    let f = Fixture::new().await;
    let (_, work, marker) = f.prepared().await;
    for field in 0..5 {
        let mut foreign = f.binding.clone();
        match field { 0 => foreign.principal.push('x'), 1 => foreign.owner.push('x'),
            2 => foreign.connection.push('x'), 3 => foreign.namespace = "temp".into(),
            _ => foreign.fingerprint = "f".repeat(64) }
        assert!(validate_request(&work, &foreign, &marker).is_err());
    }
    let mut wrong = work.clone(); wrong.draft.object.parent.as_mut().unwrap().name = "audit".into();
    assert!(validate_request(&wrong, &f.binding, &marker).is_err());
    wrong = work.clone(); wrong.draft.base_revision = "b".repeat(64);
    assert!(validate_request(&wrong, &f.binding, &marker).is_err());
    wrong = work.clone(); wrong.confirm_drop = false;
    assert_eq!(validate_request(&wrong, &f.binding, &marker).unwrap_err().code, "OBJECT_MUTATION_CONFIRM_REQUIRED");
    for operation in [ObjectMutationOperation::Rename, ObjectMutationOperation::Enable, ObjectMutationOperation::Disable] {
        wrong = work.clone(); wrong.draft.operation = operation;
        assert_eq!(validate_request(&wrong, &f.binding, &marker).unwrap_err().code, "OBJECT_MUTATION_OPERATION_UNSUPPORTED");
    }
    assert_eq!(validate_request(&work, &f.binding, "DROP TRIGGER main.sibling").unwrap_err().code, "EXECUTION_PAYLOAD_MISMATCH");
    validate_request(&work, &f.binding, &marker).unwrap();
    assert!(f.present().await);
    f.cleanup().await;
}

#[tokio::test]
async fn cancelled_before_send_keeps_trigger_schema_cookie_and_releases_owned_guards() {
    let f = Fixture::new().await;
    let (_, work, marker) = f.prepared().await;
    let before = f.scalar("PRAGMA main.schema_version").await;
    let entry = f.entry(&marker, true);
    let result = f.execute(work, marker, entry.clone(), CommitMode::Normal).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "EXECUTION_CANCELLED_BEFORE_SEND");
    assert!(result.no_effect); assert!(result.confirmed); assert!(result.receipt.is_none());
    assert_eq!(result.transaction, "none");
    let outcome = entry.finish(result.result, result.confirmed, true);
    assert_eq!(outcome.state, ExecutionState::Cancelled); assert!(!outcome.resources_pending);
    assert!(f.present().await); assert_eq!(f.scalar("PRAGMA main.schema_version").await, before);
    assert!(f.manual.try_ordinary(&[&f.binding.connection]).await.unwrap().is_some());
    assert!(f.activity.exclusive().is_ok());
    f.cleanup().await;
}

#[tokio::test]
async fn real_drop_commit_lost_ack_returns_unknown_without_receipt_and_closes_connection() {
    let f = Fixture::new().await;
    let (_, work, marker) = f.prepared().await;
    let entry = f.entry(&marker, false);
    let result = f.execute(work.clone(), marker.clone(), entry.clone(), CommitMode::LoseAcknowledgement).await;
    assert_eq!(result.result.as_ref().unwrap_err().code, "EXECUTION_EFFECTS_UNKNOWN");
    assert!(!result.no_effect); assert_eq!(result.transaction, "unknown"); assert!(result.receipt.is_none());
    assert_eq!(f.pool.size(), 0); assert_eq!(f.pool.num_idle(), 0);
    let outcome = entry.finish(result.result, result.confirmed, true);
    assert_eq!(outcome.state, ExecutionState::Unknown); assert_eq!(outcome.transaction_state, "unknown");
    assert!(!outcome.resources_pending, "worker awaited physical close; no fabricated drain");
    assert!(f.manual.try_ordinary(&[&f.binding.connection]).await.unwrap().is_some());
    assert!(f.activity.exclusive().is_ok());
    let observer = SqlitePoolOptions::new().max_connections(1).connect_with(f.options.clone()).await.unwrap();
    let mut conn = observer.acquire().await.unwrap();
    let count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND name=?")
        .bind(TARGET).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(count, 0, "independent physical connection proves the actual DROP committed");
    let sibling = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM main.sqlite_schema WHERE type='trigger' AND name='sibling'")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(sibling, 1);
    conn.return_to_pool().await; observer.close().await;
    assert!(observer.is_closed()); assert_eq!(observer.size(), 0); drop(observer);
    assert!(validate_request(&work, &f.binding, &marker).is_err(), "unknown result cannot replay consumed baseline/plan");
    f.cleanup().await;
}

#[tokio::test]
async fn opened_baseline_does_not_refresh_after_expiry_or_accept_system_parent() {
    let f = Fixture::new().await;
    let opened = f.opened().await;
    {
        let mut cache = global().cache.lock();
        cache.baselines.get_mut(&opened.baseline_id).unwrap().born = Instant::now() - (TTL + Duration::from_secs(1));
    }
    let draft = ObjectMutationDraft { baseline_id: opened.baseline_id, object: opened.object,
        source_fingerprint: opened.source_fingerprint, base_revision: opened.base_revision,
        operation: ObjectMutationOperation::Drop };
    let ordinary = f.manual.ordinary(&[&f.binding.connection]).await.unwrap();
    let error = preflight(f.db.clone(), DbKind::of("sqlite"), f.binding.clone(), draft,
        ordinary, f.activity.enter().unwrap(), false).await.unwrap_err();
    assert_eq!(error.code, "OBJECT_MUTATION_BASELINE_EXPIRED");
    for name in ["sqlite_sequence", "SQLITE_internal", "bad\0name"] {
        let mut object = f.object(); object.parent.as_mut().unwrap().name = name.into();
        assert!(identity(&object, &f.binding).is_err());
    }
    assert!(f.present().await);
    f.cleanup().await;
}
