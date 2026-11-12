//! Real SQLite/durable-host integration for scheduler_structure. No mock executor or C1 stub.
//! Parent registers this only as a child module of scheduler_structure, after production delegation.
use super::*;
use std::{path::{Path, PathBuf}, sync::Arc};
use crate::{db::{self, Db}, models::{ConnectionConfig, DbKind}, scheduler_desktop::{DesktopExecutor, SchedulerHost}, storage::Store};
use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, SqlitePool};

struct Fixture { state: Arc<AppState>, host: SchedulerHost, source: SqlitePool, target: SqlitePool, root: PathBuf }
#[derive(Debug, PartialEq, Eq)]
struct Evidence {
    source_generation: i64, target_generation: i64,
    source: Vec<(String, String, String, i64, Option<String>)>,
    target: Vec<(String, String, String, i64, Option<String>)>,
    rows: Vec<(i64, String)>, audit: Vec<String>,
}
impl Fixture {
    fn config(id: &str, path: &Path) -> ConnectionConfig {
        ConnectionConfig { id: id.into(), name: id.into(), kind: DbKind::of("sqlite"), host: String::new(), port: 0,
            user: String::new(), password: String::new(), database: "main".into(), file: path.to_string_lossy().into_owned(),
            params: String::new(), group: String::new(), read_only: false, keep_alive: false }
    }
    async fn pool(path: &Path) -> SqlitePool {
        SqlitePoolOptions::new().max_connections(2).connect_with(SqliteConnectOptions::new().filename(path)
            .create_if_missing(true).journal_mode(SqliteJournalMode::Wal).foreign_keys(true)).await.unwrap()
    }
    async fn new(distinct: bool) -> Self {
        let root = std::env::temp_dir().join(format!("navkit-scheduled-structure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let source_path = root.join("source.sqlite");
        let target_path = if distinct { root.join("target.sqlite") } else { source_path.clone() };
        let source = Self::pool(&source_path).await;
        let target = if distinct { Self::pool(&target_path).await } else { source.clone() };
        sqlx::raw_sql("CREATE TABLE src(a INT,added TEXT); INSERT INTO src VALUES(9,'Source  Case')")
            .execute(&source).await.unwrap();
        sqlx::raw_sql("CREATE TABLE dst(a INT CHECK(a>=0),extra TEXT DEFAULT 'Keep  Case'); CREATE INDEX extra_idx ON dst(extra,a); CREATE TABLE audit(value TEXT); CREATE TRIGGER dst_audit AFTER INSERT ON dst BEGIN INSERT INTO audit VALUES('Trigger  Case'); END; INSERT INTO dst(a,extra) VALUES(1,'original')")
            .execute(&target).await.unwrap();
        let store = Store::empty_at(&root); store.ensure_encryption_key().unwrap();
        let configs = vec![Self::config("source", &source_path), Self::config("target", &target_path)];
        store.save_connections(configs.clone(), vec![]).unwrap();
        let state = Arc::new(AppState { store, registry: Arc::new(db::DbRegistry::default()), manual_transactions: Default::default(),
            activity: Default::default(), executions: Default::default(), mutations: Default::default(),
            connection_runtime: Default::default(), query_results: Default::default() });
        for config in configs {
            let connection = Arc::new(db::open_saved(&db::request_from_config(&config, ""), false, &state.store.settings()).await.unwrap());
            let fp = db::runtime::binding_fingerprint_at(&config, state.store.data_root()).unwrap();
            state.connection_runtime.publish(&config.id, &fp, &connection, None, db::runtime::keepalive_policy(&config).unwrap()).unwrap();
            state.registry.write().insert(config.id, connection);
        }
        let host = SchedulerHost::with_executor(root.clone(), SchedulerOwner::new(RunnerMode::Gui),
            Arc::new(SystemSchedulerClock::default()), Arc::new(DesktopExecutor::from_state(state.clone())));
        Self { state, host, source, target, root }
    }
    fn options(&self) -> StructureSyncRequest {
        StructureSyncRequest { source_connection: "source".into(), source_schema: "main".into(), source_table: "src".into(),
            target_connection: "target".into(), target_schema: "main".into(), target_table: "dst".into(), columns: vec![],
            allow_drop: false, source_revision: String::new(), target_revision: String::new(), review_digest: String::new() }
    }
    fn template(&self, options: &StructureSyncRequest) -> ActionTemplate {
        self.host.service.save_template(ActionTemplate { id: uuid::Uuid::new_v4().to_string(), revision: String::new(),
            name: "owned structure template".into(), kind: ActionKind::StructureSync, connection_id: "target".into(),
            namespace: "main".into(), read_only: false, configuration: TemplateConfiguration::Tool {
                options_json: serde_json::to_string(&serde_json::json!({"kind":"structure_sync","options":options})).unwrap() } }, None).unwrap()
    }
    async fn approved(&self, template: &ActionTemplate) -> JobRecord {
        let record = self.host.service.save_job(ScheduledJobV2 { version: 2, id: uuid::Uuid::new_v4().to_string(),
            revision: String::new(), name: "real owned structure run".into(), enabled: false, execution_mode: ExecutionMode::AppOnline,
            schedule: ScheduleSpec::Manual, misfire: MisfirePolicy::LatestOnce, overlap: OverlapPolicy::Skip, timeout_ms: 15_000,
            actions: vec![ScheduledAction { id: uuid::Uuid::new_v4().to_string(), kind: ActionKind::StructureSync,
                depends_on: vec![], template_ref: template.id.clone(), approved_scope_revision: String::new() }] }, None).unwrap();
        let plan = tokio::time::timeout(Duration::from_secs(8), self.host.service.prepare_approval(&record.job.id,
            &record.job.revision, "actual-scheduler-test-window")).await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(8), self.host.service.approve_job(&plan.plan_id,
            &plan.review_digest, "actual-scheduler-test-window")).await.unwrap().unwrap()
    }
    async fn finished(&self, id: &str) -> SchedulerRunSummary {
        tokio::time::timeout(Duration::from_secs(18), async {
            loop {
                if let Some(run) = self.host.service.history(None).unwrap().into_iter().find(|run| run.id == id) {
                    if run.status.terminal() && !run.resources_pending { return run; }
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.expect("real scheduler/C1 structure operation did not settle; possible ordinary self-lock")
    }
    async fn evidence(&self) -> Evidence {
        async fn read(pool: &SqlitePool) -> (i64, Vec<(String, String, String, i64, Option<String>)>) {
            let mut connection = pool.acquire().await.unwrap();
            let generation = sqlx::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *connection).await.unwrap();
            let catalog = sqlx::query_as("SELECT type,name,tbl_name,rootpage,sql FROM main.sqlite_schema ORDER BY type,name,tbl_name")
                .fetch_all(&mut *connection).await.unwrap(); connection.return_to_pool().await; (generation, catalog)
        }
        let (source_generation, source) = read(&self.source).await; let (target_generation, target) = read(&self.target).await;
        let mut connection = self.target.acquire().await.unwrap();
        let rows = sqlx::query_as("SELECT a,extra FROM dst ORDER BY a,extra").fetch_all(&mut *connection).await.unwrap();
        let audit = sqlx::query_scalar("SELECT value FROM audit ORDER BY rowid").fetch_all(&mut *connection).await.unwrap();
        connection.return_to_pool().await; Evidence { source_generation, target_generation, source, target, rows, audit }
    }
    async fn cleanup(self) {
        self.host.request_stop(); self.state.executions.shutdown().await.unwrap();
        drop(self.state.manual_transactions.close_all().await.unwrap());
        let connections: Vec<_> = self.state.registry.write().drain().map(|(_, connection)| connection).collect();
        for connection in connections { if let Db::Sqlite(pool) = connection.as_ref() { pool.close().await; } }
        self.source.close().await; self.target.close().await;
        drop(self.host); drop(self.state); std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn stable_business_scope_survives_review_owner_changes_and_real_host_adds_through_c1_without_self_lock() {
    let fixture = Fixture::new(false).await; let options = fixture.options(); let template = fixture.template(&options);
    let activity = fixture.state.activity.enter().unwrap();
    let review = observe(&fixture.state, &options, "review:first-window", "review:first-owner").await.unwrap();
    let runtime = observe(&fixture.state, &options, "scheduler:actual-instance", "job:actual-runtime-owner").await.unwrap();
    assert_ne!(review.binding, runtime.binding); assert_ne!(review.preflight.report.review_digest, runtime.preflight.report.review_digest);
    assert_ne!(review.preflight.request_sql, runtime.preflight.request_sql);
    assert_eq!(scope(&template, &options, &review).unwrap(), scope(&template, &options, &runtime).unwrap());
    assert_eq!(semantic_revision(&options, &review).unwrap(), semantic_revision(&options, &runtime).unwrap()); drop(activity);
    let approved = fixture.approved(&template).await; let before = fixture.evidence().await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let claimed = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &request_id).await.unwrap().unwrap();
    let run = fixture.finished(&claimed.id).await;
    assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.actions); assert!(!run.resources_pending);
    let action = &run.actions[0]; let receipt = action.receipt.as_ref().unwrap();
    assert_eq!(action.status, ActionStatus::Succeeded); assert!(receipt.verified); assert_eq!(receipt.effects, ActionEffects::Committed);
    let execution_id = receipt.execution_id.as_ref().expect("actual C1 execution identity must be retained");
    let principal = format!("scheduler:{}", fixture.host.service.runner.owner.instance_id);
    let owner = format!("job:{}/{}/{}", approved.job.id, run.id, approved.job.actions[0].id);
    let cfg = commands::saved_config(&fixture.state, "target").unwrap();
    let binding = commands::execution_binding(&fixture.state, &cfg, &principal, &owner, "main").unwrap();
    let entry = fixture.state.executions.authorized(execution_id, &binding).unwrap();
    assert_eq!(entry.status().state, ExecutionState::Succeeded); assert!(!entry.status().resources_pending);
    assert!(!fixture.state.executions.has_active("target")); assert!(!fixture.state.executions.has_active("source"));
    let after = fixture.evidence().await; assert_eq!(after.rows, before.rows); assert_eq!(after.audit, before.audit);
    assert!(after.target_generation > before.target_generation);
    for object in before.target.iter().filter(|object| object.1 != "dst") { assert!(after.target.contains(object), "lost {}", object.1); }
    let ddl = after.target.iter().find(|object| object.1 == "dst").unwrap().4.as_ref().unwrap();
    assert!(ddl.contains("added") && ddl.contains("CHECK(a>=0)") && ddl.contains("extra"));
    let mut observer = fixture.target.acquire().await.unwrap();
    let values: Vec<(i64, String, Option<String>)> = sqlx::query_as("SELECT a,extra,added FROM dst").fetch_all(&mut *observer).await.unwrap();
    observer.return_to_pool().await; assert_eq!(values, vec![(1, "original".into(), None)]); assert!(fixture.target.num_idle() > 0);
    assert!(fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &request_id).await.unwrap().is_none());
    assert_eq!(fixture.evidence().await, after); assert_eq!(fixture.host.service.history(Some(&approved.job.id)).unwrap().len(), 1);
    let durable = SchedulerStore::new(fixture.root.clone()).history(Some(&approved.job.id)).unwrap();
    assert_eq!(durable.len(), 1); assert_eq!(durable[0].status, RunStatus::Succeeded); assert_eq!(durable[0].id, run.id);
    fixture.cleanup().await;
}

#[tokio::test]
async fn changed_source_with_unchanged_target_rejects_original_approval_without_ddl_or_automatic_replay() {
    let fixture = Fixture::new(true).await; let options = fixture.options(); let template = fixture.template(&options);
    let activity = fixture.state.activity.enter().unwrap();
    let original = observe(&fixture.state, &options, "review:first-window", "review:first-owner").await.unwrap(); drop(activity);
    let approved = fixture.approved(&template).await;
    let original_scope = approved.approvals.get(&approved.job.actions[0].id).unwrap().scope.clone();
    sqlx::query("ALTER TABLE src ADD COLUMN external_change TEXT").execute(&fixture.source).await.unwrap();
    let activity = fixture.state.activity.enter().unwrap();
    let current = observe(&fixture.state, &options, "scheduler:actual-instance", "job:actual-owner").await.unwrap(); drop(activity);
    assert_eq!(original.preflight.report.target_revision, current.preflight.report.target_revision);
    assert_ne!(original.preflight.report.source_revision, current.preflight.report.source_revision);
    assert_ne!(scope(&template, &options, &current).unwrap(), original_scope);
    let before = fixture.evidence().await; let request_id = uuid::Uuid::new_v4().to_string();
    let claimed = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &request_id).await.unwrap().unwrap();
    let run = fixture.finished(&claimed.id).await;
    assert_eq!(run.status, RunStatus::AwaitingConfirmation, "{:?}", run.actions); assert!(!run.resources_pending);
    let receipt = run.actions[0].receipt.as_ref().unwrap(); assert_eq!(receipt.status, ActionStatus::AwaitingConfirmation);
    assert_eq!(receipt.effects, ActionEffects::None); assert!(receipt.verified && receipt.execution_id.is_none());
    assert_eq!(fixture.evidence().await, before);
    assert!(!fixture.state.executions.has_active("target")); assert!(!fixture.state.executions.has_active("source"));
    let current_job = fixture.host.service.list_jobs().unwrap().into_iter().find(|job| job.job.id == approved.job.id).unwrap();
    assert_eq!(current_job.blocked_run_id.as_deref(), Some(run.id.as_str())); assert!(current_job.queued.is_none()); assert_eq!(current_job.approvals, approved.approvals);
    assert!(fixture.host.service.run_now(&approved.job.id, &current_job.job.revision, &uuid::Uuid::new_v4().to_string()).await.is_err());
    assert_eq!(fixture.host.service.history(Some(&approved.job.id)).unwrap().len(), 1); assert_eq!(fixture.evidence().await, before);
    fixture.cleanup().await;
}
