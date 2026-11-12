//! Real temporary SQLite integration of the public structure service and the existing C1 engine.
//! Loaded by structure_commands with #[path = "structure_integration_tests.rs"] mod integration_tests.
//! No Webview, alternate grant engine, injected database result, or user database is used.
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};
use crate::{
    commands::{self, AppState},
    controlled_execution::{self, ControlledResponse, ControlledWork},
    db::{self, Db},
    error::CmdResult,
    execution_contracts::{ExecutionRequest, ExecutionState},
    models::{ConnectionConfig, DbKind},
    storage::Store,
    structure_commands,
    structure_execution::metadata::{self, StructurePreflight},
    structure_sync::{self, StructureReviewBinding, StructureSyncRequest},
};
use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, SqlitePool};

const WINDOW: &str = "structure-integration-window";
const PAGE: &str = "structure-integration-page";

struct Fixture {
    state: Arc<AppState>,
    source_observer: SqlitePool,
    target_observer: SqlitePool,
    root: PathBuf,
}
#[derive(Debug, PartialEq, Eq)]
struct CatalogEvidence {
    source_generation: i64,
    target_generation: i64,
    source: Vec<(String, String, String, i64, Option<String>)>,
    target: Vec<(String, String, String, i64, Option<String>)>,
    rows: Vec<(i64, String)>,
    audit: Vec<String>,
}
impl Fixture {
    async fn observer(path: &Path) -> SqlitePool {
        SqlitePoolOptions::new().max_connections(2).connect_with(
            SqliteConnectOptions::new().filename(path).create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal).foreign_keys(true)
        ).await.unwrap()
    }
    fn config(id: &str, path: &Path) -> ConnectionConfig {
        ConnectionConfig { id: id.into(), name: format!("temporary {id}"), kind: DbKind::of("sqlite"),
            host: String::new(), port: 0, user: String::new(), password: String::new(),
            database: "main".into(), file: path.to_string_lossy().into_owned(), params: String::new(),
            group: String::new(), read_only: false, keep_alive: false }
    }
    async fn new(distinct_files: bool, source_index: bool) -> Self {
        let root = std::env::temp_dir().join(format!("navkit-structure-service-{}", uuid::Uuid::new_v4()));
        assert!(root.is_absolute()); std::fs::create_dir(&root).unwrap();
        let source_path = root.join("source.sqlite");
        let target_path = if distinct_files { root.join("target.sqlite") } else { source_path.clone() };
        let source_observer = Self::observer(&source_path).await;
        let target_observer = if distinct_files { Self::observer(&target_path).await } else { source_observer.clone() };
        sqlx::raw_sql("CREATE TABLE src(a INT,added TEXT); INSERT INTO src VALUES(9,'Source  Case')")
            .execute(&source_observer).await.unwrap();
        if source_index {
            sqlx::query("CREATE INDEX src_added_idx ON src(added,a)").execute(&source_observer).await.unwrap();
        }
        sqlx::raw_sql("CREATE TABLE dst(a INT CHECK(a>=0),extra TEXT DEFAULT 'Keep  Case'); \
            CREATE INDEX extra_idx ON dst(extra,a); CREATE TABLE audit(value TEXT); \
            CREATE TRIGGER dst_audit AFTER INSERT ON dst BEGIN INSERT INTO audit VALUES('Trigger  Case'); END; \
            INSERT INTO dst(a,extra) VALUES(1,'original')")
            .execute(&target_observer).await.unwrap();
        let store = Store::empty_at(&root);
        let configurations = vec![Self::config("source", &source_path), Self::config("target", &target_path)];
        store.save_connections(configurations.clone(), vec![]).unwrap();
        let state = Arc::new(AppState { store, registry: Arc::new(db::DbRegistry::default()),
            manual_transactions: Default::default(), activity: Default::default(), executions: Default::default(),
            mutations: Default::default(), connection_runtime: Default::default(), query_results: Default::default() });
        for cfg in configurations {
            let opened = db::open_saved(&db::request_from_config(&cfg, ""), false, &state.store.settings()).await.unwrap();
            let connection = Arc::new(opened);
            let fingerprint = db::runtime::binding_fingerprint_at(&cfg, state.store.data_root()).unwrap();
            state.connection_runtime.publish(&cfg.id, &fingerprint, &connection, None,
                db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
            state.registry.write().insert(cfg.id, connection);
        }
        Self { state, source_observer, target_observer, root }
    }
    fn input(&self) -> StructureSyncRequest {
        StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: "src".into(),
            target_connection: "target".into(), target_schema: "main".into(), target_table: "dst".into(),
            columns: vec![], allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() }
    }
    async fn preflight(&self, input: &StructureSyncRequest) -> CmdResult<StructurePreflight> {
        let _activity = self.state.activity.enter()?;
        let binding = structure_commands::review_binding(&self.state, input, WINDOW, PAGE)?;
        let ordinary = self.state.manual_transactions.ordinary(&[&input.source_connection, &input.target_connection]).await?;
        let source_cfg = commands::saved_config(&self.state, &input.source_connection)?;
        let target_cfg = commands::saved_config(&self.state, &input.target_connection)?;
        let source = commands::scoped_db(&self.state, &input.source_connection, &input.source_schema).await?;
        let target = commands::scoped_db(&self.state, &input.target_connection, &input.target_schema).await?;
        structure_commands::recheck_runtime(&self.state, input, &binding, &source, &target)?;
        let result = tokio::time::timeout(Duration::from_secs(10), metadata::preflight(source.clone(), source_cfg.kind,
            target.clone(), target_cfg.kind, ordinary, input.clone(), binding.clone(), target_cfg.read_only))
            .await.expect("real structure preflight exceeded the fixture budget")?;
        structure_commands::recheck_runtime(&self.state, input, &binding, &source, &target)?;
        Ok(result)
    }
    async fn reviewed(&self) -> (StructureSyncRequest, StructureReviewBinding, StructurePreflight) {
        let mut request = self.input();
        let preflight = self.preflight(&request).await.unwrap();
        assert!(preflight.report.alterable, "{:?}", preflight.report.warnings);
        assert!(!preflight.report.steps.is_empty());
        assert!(!preflight.observation_only);
        assert_eq!(preflight.report.review_digest.len(), 64);
        request.source_revision = preflight.report.source_revision.clone();
        request.target_revision = preflight.report.target_revision.clone();
        request.review_digest = preflight.report.review_digest.clone();
        let binding = structure_commands::review_binding(&self.state, &request, WINDOW, PAGE).unwrap();
        structure_sync::validate_review(&request, &preflight.report, &binding).unwrap();
        assert_eq!(preflight.request_sql, structure_sync::review_request_sql(&request).unwrap());
        (request, binding, preflight)
    }
    fn unsigned(&self, request: &StructureSyncRequest) -> ExecutionRequest {
        ExecutionRequest { execution_id: String::new(), connection_id: request.target_connection.clone(),
            namespace: request.target_schema.clone(), owner_id: PAGE.into(),
            sql: structure_sync::review_request_sql(request).unwrap(), transaction_owner: None,
            timeout_ms: Some(10_000), origin: Some("structure_sync".into()), execution_grant: None, cancel_before_send: false }
    }
    fn signed(&self, request: &StructureSyncRequest) -> ExecutionRequest {
        let mut execution = self.unsigned(request);
        commands::grant_execution_request(&self.state, &mut execution, WINDOW).unwrap();
        assert!(!execution.execution_id.is_empty()); assert!(execution.execution_grant.is_some());
        execution
    }
    async fn execute(&self, execution: ExecutionRequest, request: StructureSyncRequest,
        binding: StructureReviewBinding, principal: &str) -> CmdResult<ControlledResponse> {
        tokio::time::timeout(Duration::from_secs(12), controlled_execution::execute(&self.state, execution, principal,
            ControlledWork::Structure { request, binding })).await.expect("C1 structure service did not drain within the fixture budget")
    }
    async fn evidence(&self) -> CatalogEvidence {
        async fn catalog(pool: &SqlitePool) -> Vec<(String, String, String, i64, Option<String>)> {
            sqlx::query_as("SELECT type,name,tbl_name,rootpage,sql FROM main.sqlite_schema ORDER BY type,name,tbl_name")
                .fetch_all(pool).await.unwrap()
        }
        CatalogEvidence {
            source_generation: sqlx::query_scalar("PRAGMA main.schema_version").fetch_one(&self.source_observer).await.unwrap(),
            target_generation: sqlx::query_scalar("PRAGMA main.schema_version").fetch_one(&self.target_observer).await.unwrap(),
            source: catalog(&self.source_observer).await, target: catalog(&self.target_observer).await,
            rows: sqlx::query_as("SELECT a,extra FROM dst ORDER BY a,extra").fetch_all(&self.target_observer).await.unwrap(),
            audit: sqlx::query_scalar("SELECT value FROM audit ORDER BY rowid").fetch_all(&self.target_observer).await.unwrap(),
        }
    }
    fn update_config(&self, id: &str, change: impl FnOnce(&mut ConnectionConfig)) {
        let mut configs = self.state.store.connections();
        change(configs.iter_mut().find(|cfg| cfg.id == id).unwrap());
        self.state.store.save_connections(configs, vec![]).unwrap();
    }
    async fn cleanup(self) {
        assert!(!self.state.executions.has_active("source")); assert!(!self.state.executions.has_active("target"));
        drop(self.state.manual_transactions.close_all().await.unwrap());
        let connections: Vec<_> = self.state.registry.write().drain().map(|(_, connection)| connection).collect();
        for connection in &connections {
            if let Db::Sqlite(pool) = connection.as_ref() {
                pool.close().await;
                assert!(pool.is_closed());
                assert_eq!(pool.size(), 0, "all registered physical connections must drain");
            }
        }
        self.source_observer.close().await; self.target_observer.close().await;
        assert!(self.source_observer.is_closed() && self.target_observer.is_closed());
        assert_eq!(self.source_observer.size(), 0);
        assert_eq!(self.target_observer.size(), 0);
        let root = self.root;
        drop(connections); drop(self.state);
        // Windows may briefly retain a retiring SQLite worker's file handle after
        // SQLx has acknowledged pool close. Retry only sharing/lock violations;
        // retained resources or any other error must still fail this test.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))
                    && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => panic!("structure fixture retained after closed pools: {}: {error}", root.display()),
            }
        }
        assert!(!root.exists());
    }
}
fn denied(result: CmdResult<ControlledResponse>) -> String {
    match result {
        Err(error) => error.code,
        Ok(response) => {
            assert_eq!(response.outcome.state, ExecutionState::Failed, "{:?}", response.outcome.error);
            assert!(!response.outcome.resources_pending); assert!(response.outcome.result.is_none());
            assert!(response.structure.is_none());
            response.outcome.error.expect("a rejected operation must retain its real error").code
        }
    }
}

#[tokio::test]
async fn service_same_actual_file_add_commits_and_preserves_extra_data_index_check_trigger_and_one_shot_grant() {
    let f = Fixture::new(false, false).await;
    let (request, binding, preview) = f.reviewed().await;
    assert!(preview.same_database); assert_eq!(preview.observation_binding, binding);
    assert_eq!(preview.source_identity.database, preview.target_identity.as_ref().unwrap().database);
    let before = f.evidence().await; let execution = f.signed(&request); let id = execution.execution_id.clone();
    let response = f.execute(execution.clone(), request.clone(), binding.clone(), WINDOW).await.unwrap();
    assert_eq!(response.outcome.execution_id, id); assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error);
    assert_eq!(response.outcome.effects, "completed"); assert_eq!(response.outcome.transaction_state, "committed");
    assert!(!response.outcome.resources_pending); assert!(response.outcome.error.is_none()); assert!(response.outcome.result.is_some());
    let receipt = response.structure.as_ref().unwrap();
    assert!(receipt.receipt.verified); assert!(!receipt.receipt.unverified); assert!(receipt.receipt.mismatches.is_empty());
    assert_eq!(receipt.receipt.transaction, "committed"); assert!(!receipt.receipt.steps.is_empty());
    assert!(receipt.receipt.steps.iter().all(|step| step.status == "applied"));
    assert_eq!(receipt.report.source_revision, request.source_revision); assert_eq!(receipt.report.target_revision, request.target_revision);
    assert_eq!(receipt.report.review_digest, request.review_digest); assert_eq!(receipt.report.review_binding.as_ref(), Some(&binding));
    let schema = receipt.receipt.schema.as_ref().unwrap(); assert!(schema.columns.iter().any(|column| column.name == "added"));
    assert!(schema.columns.iter().any(|column| column.name == "extra"));
    let after = f.evidence().await;
    assert_eq!(before.rows, after.rows); assert_eq!(before.audit, after.audit);
    for object in before.target.iter().filter(|row| row.1 != "dst") { assert!(after.target.contains(object), "lost or changed {}", object.1); }
    assert!(after.target.iter().find(|row| row.1 == "dst").unwrap().4.as_ref().unwrap().contains("CHECK(a>=0)"));
    let values: Vec<(i64, String, Option<String>)> = sqlx::query_as("SELECT a,extra,added FROM dst").fetch_all(&f.target_observer).await.unwrap();
    assert_eq!(values, vec![(1, "original".into(), None)]);
    let actual = commands::execution_binding(&f.state, &commands::saved_config(&f.state, "target").unwrap(), WINDOW, PAGE, "main").unwrap();
    let status = f.state.executions.authorized(&id, &actual).unwrap().status();
    assert_eq!(status.state, ExecutionState::Succeeded); assert!(!status.resources_pending);
    let settled = f.evidence().await;
    assert_eq!(denied(f.execute(execution, request, binding, WINDOW).await), "EXECUTION_ID_DUPLICATE");
    assert_eq!(f.evidence().await, settled);
    let lease = tokio::time::timeout(Duration::from_secs(1), f.state.manual_transactions.ordinary(&["source", "target"]))
        .await.unwrap().unwrap(); drop(lease);
    f.cleanup().await;
}

#[tokio::test]
async fn service_distinct_real_files_adds_ordered_index_and_preserves_original_source_and_target_extras() {
    let f = Fixture::new(true, true).await; let (request, binding, preview) = f.reviewed().await;
    assert!(!preview.same_database); assert_ne!(preview.source_identity.database, preview.target_identity.as_ref().unwrap().database);
    assert!(preview.report.steps.iter().any(|step| step.label.contains("INDEX") || step.sql.to_ascii_uppercase().contains("CREATE INDEX")));
    let before = f.evidence().await;
    let response = f.execute(f.signed(&request), request, binding, WINDOW).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error);
    assert_eq!(response.outcome.transaction_state, "committed"); assert!(!response.outcome.resources_pending);
    let receipt = response.structure.unwrap(); assert!(receipt.receipt.verified);
    let schema = receipt.receipt.schema.unwrap();
    let index = schema.indexes.iter().find(|index| index.name == "src_added_idx").unwrap(); assert_eq!(index.columns, vec!["added", "a"]);
    let actual: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_index_info('src_added_idx') ORDER BY seqno")
        .fetch_all(&f.target_observer).await.unwrap(); assert_eq!(actual, vec!["added", "a"]);
    let after = f.evidence().await; assert_eq!(before.source, after.source); assert_eq!(before.source_generation, after.source_generation);
    assert_eq!(before.rows, after.rows); assert_eq!(before.audit, after.audit);
    assert!(after.target.iter().any(|row| row.1 == "extra_idx"));
    f.cleanup().await;
}

#[tokio::test]
async fn service_same_file_index_name_collision_rolls_back_real_first_add_without_touching_source_or_extra() {
    // A real source index occupies SQLite's database-global index name; never mock a successful clone.
    let f = Fixture::new(false, true).await; let (request, binding, _) = f.reviewed().await;
    let before = f.evidence().await;
    let response = f.execute(f.signed(&request), request, binding, WINDOW).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Failed, "{:?}", response.outcome.error);
    assert_eq!(response.outcome.transaction_state, "rolled_back"); assert_eq!(response.outcome.effects, "no_effect");
    assert!(!response.outcome.resources_pending); assert!(response.outcome.result.is_none());
    let receipt = response.structure.unwrap(); assert!(!receipt.receipt.verified); assert_eq!(receipt.receipt.transaction, "rolled_back");
    assert!(receipt.receipt.steps.iter().any(|step| step.status == "rolled_back"));
    assert!(receipt.receipt.steps.iter().any(|step| step.status == "failed")); assert_eq!(f.evidence().await, before);
    f.cleanup().await;
}

#[tokio::test]
async fn service_source_saved_config_or_real_encrypted_credential_change_invalidates_review_with_zero_ddl() {
    for credential in [false, true] {
        let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await;
        let before = f.evidence().await;
        if credential {
            let encrypted = f.state.store.encrypt_password("changed-test-credential").unwrap();
            assert_eq!(f.state.store.decrypt_password(&encrypted).unwrap(), "changed-test-credential");
            f.update_config("source", |cfg| cfg.password = encrypted);
        } else { f.update_config("source", |cfg| cfg.host = "changed-saved-host".into()); }
        let changed = structure_commands::review_binding(&f.state, &request, WINDOW, PAGE).unwrap();
        assert_ne!(binding.source_fingerprint, changed.source_fingerprint); assert_eq!(binding.target_fingerprint, changed.target_fingerprint);
        let code = denied(f.execute(f.signed(&request), request, binding, WINDOW).await);
        assert!(!code.is_empty()); assert_eq!(f.evidence().await, before);
        f.cleanup().await;
    }
}

#[tokio::test]
async fn service_target_saved_config_change_after_grant_cannot_refresh_old_authorization_or_send_ddl() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await;
    let execution = f.signed(&request); let before = f.evidence().await;
    f.update_config("target", |cfg| cfg.user = "different-saved-user".into());
    let code = denied(f.execute(execution, request, binding, WINDOW).await);
    assert!(code.starts_with("EXECUTION_") || code.starts_with("STRUCTURE_"), "配置变更后必须被拒绝，实际：{code}");
    assert_eq!(f.evidence().await, before); f.cleanup().await;
}

#[tokio::test]
async fn service_target_readonly_after_review_is_rejected_even_with_fresh_correctly_signed_c1_grant() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await;
    let before = f.evidence().await; f.update_config("target", |cfg| cfg.read_only = true);
    assert_ne!(structure_commands::review_binding(&f.state, &request, WINDOW, PAGE).unwrap().target_fingerprint, binding.target_fingerprint);
    assert_eq!(denied(f.execute(f.signed(&request), request, binding, WINDOW).await), "STRUCTURE_CONTEXT_CHANGED");
    assert_eq!(f.evidence().await, before); f.cleanup().await;
}

#[tokio::test]
async fn service_signed_owner_namespace_marker_or_window_cannot_be_modified_after_grant() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await;
    let signed = f.signed(&request); let before = f.evidence().await;
    for part in 0..4 {
        let mut execution = signed.clone();
        match part { 0 => execution.owner_id = "other-page".into(), 1 => execution.namespace = " main".into(),
            2 => execution.sql.push_str(" changed-marker"), _ => {} }
        let principal = if part == 3 { "other-window" } else { WINDOW };
        // 篡改 owner/namespace/marker/window 必须被拒绝；实现先校验结构上下文再校验 grant，
        // 因此两类安全错误码均合法（两者都表示零 DDL 发送）。
        let code = denied(f.execute(execution, request.clone(), binding.clone(), principal).await);
        // 任何拒绝码都满足安全不变量；关键是零 DDL（下方 evidence 比对立证）。
        assert!(code.starts_with("EXECUTION_") || code.starts_with("STRUCTURE_"),
            "篡改后必须被明确拒绝，实际错误码：{code}");
        assert_eq!(f.evidence().await, before);
    }
    // Invalid attempts must not consume the real unchanged owner's grant.
    let response = f.execute(signed, request, binding, WINDOW).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error); f.cleanup().await;
}

#[tokio::test]
async fn service_fresh_grant_for_another_owner_or_namespace_does_not_authorize_the_reviewed_work() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await; let before = f.evidence().await;
    for owner in [true, false] {
        let mut execution = f.unsigned(&request);
        if owner { execution.owner_id = "other-page".into(); } else { execution.namespace = " main".into(); }
        commands::grant_execution_request(&f.state, &mut execution, WINDOW).unwrap();
        assert_eq!(denied(f.execute(execution, request.clone(), binding.clone(), WINDOW).await), "STRUCTURE_CONTEXT_CHANGED");
        assert_eq!(f.evidence().await, before);
    }
    f.cleanup().await;
}

#[tokio::test]
async fn service_missing_corrupted_or_payload_substituted_grant_never_sends_ddl() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await; let before = f.evidence().await;
    for part in 0..3 {
        let mut execution = f.signed(&request);
        match part { 0 => execution.execution_grant = None, 1 => execution.execution_grant = Some("corrupted".into()),
            _ => { execution.sql = "DROP TABLE dst".into(); commands::grant_execution_request(&f.state, &mut execution, WINDOW).unwrap(); } }
        let code = denied(f.execute(execution, request.clone(), binding.clone(), WINDOW).await);
        assert_eq!(code, if part == 2 { "EXECUTION_PAYLOAD_MISMATCH" } else { "EXECUTION_GRANT_INVALID" });
        assert_eq!(f.evidence().await, before);
    }
    f.cleanup().await;
}

#[tokio::test]
async fn service_alias_connections_resolving_to_same_actual_table_are_rejected_by_real_preflight() {
    let f = Fixture::new(false, false).await; let before = f.evidence().await;
    let mut request = f.input(); request.target_table = request.source_table.clone();
    assert_ne!(request.source_connection, request.target_connection);
    let error = f.preflight(&request).await.err().expect("same actual source and target table must be refused");
    assert!(error.message.contains("SAME_PHYSICAL_TABLE"), "{}: {}", error.code, error.message);
    assert_eq!(f.evidence().await, before); assert!(!f.state.executions.has_active("target")); f.cleanup().await;
}

#[tokio::test]
async fn service_nochange_observation_cannot_be_promoted_to_an_executable_review() {
    let f = Fixture::new(false, false).await; let (request, binding, _) = f.reviewed().await;
    let response = f.execute(f.signed(&request), request, binding, WINDOW).await.unwrap();
    assert_eq!(response.outcome.state, ExecutionState::Succeeded, "{:?}", response.outcome.error);
    let input = f.input(); let observation = f.preflight(&input).await.unwrap();
    assert!(observation.report.steps.is_empty()); assert!(observation.request_sql.is_empty());
    assert!(observation.report.review_digest.is_empty()); assert!(observation.observation_only);
    assert_eq!(observation.observation_digest.len(), 64);
    let mut false_review = input; false_review.source_revision = observation.report.source_revision.clone();
    false_review.target_revision = observation.report.target_revision.clone(); false_review.review_digest = observation.observation_digest.clone();
    let actual = structure_commands::review_binding(&f.state, &false_review, WINDOW, PAGE).unwrap();
    assert!(structure_sync::validate_review(&false_review, &observation.report, &actual).is_err());
    let before = f.evidence().await;
    let code = denied(f.execute(f.signed(&false_review), false_review, actual, WINDOW).await);
    assert!(!code.is_empty()); assert_eq!(f.evidence().await, before); f.cleanup().await;
}
