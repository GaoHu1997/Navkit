//! Real desktop scheduler -> opaque approval -> C1/tool execution, without a GUI.
//! Register this file as a cfg(test) child of scheduler_desktop.rs; no test executor or SQL grant is mocked.
use super::{DesktopExecutor, SchedulerHost};
use crate::{
    commands::{self, AppState}, db::{self, Db}, execution_contracts::ExecutionState,
    models::{ConnectionConfig, DbKind}, scheduler::*, storage::Store,
};
use sqlx::Connection;
use std::{path::PathBuf, sync::Arc, time::Duration};

const PRINCIPAL: &str = "scheduler-real-review-window";
const PRIVATE_VALUE: &str = "real-desktop-private-value";
const INSERT_SQL: &str = "INSERT INTO items(v) VALUES ('real-desktop-private-value')";

struct Fixture { root: PathBuf, state: Arc<AppState>, host: SchedulerHost, executor: Arc<DesktopExecutor> }
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("navkit-scheduler-desktop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(root.join("test.sqlite")).create_if_missing(true);
        let mut observer = sqlx::SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, v TEXT NOT NULL)").execute(&mut observer).await.unwrap();
        observer.close().await.unwrap();
        let store = Store::empty_at(&root);
        store.ensure_encryption_key().unwrap();
        let cfg = ConnectionConfig {
            id: "scheduler-real-connection".into(), name: "real scheduler SQLite".into(), kind: DbKind::of("sqlite"),
            host: String::new(), port: 0, user: String::new(), password: String::new(), database: "main".into(),
            file: root.join("test.sqlite").to_string_lossy().into_owned(), params: String::new(), group: String::new(),
            read_only: false, keep_alive: false,
        };
        store.save_connections(vec![cfg.clone()], vec![]).unwrap();
        let state = Arc::new(AppState { store, registry: Arc::new(db::DbRegistry::default()),
            manual_transactions: Default::default(), activity: Default::default(), executions: Default::default(),
            mutations: Default::default(), connection_runtime: Default::default(), query_results: Default::default() });
        let db = Arc::new(db::open_saved(&db::request_from_config(&cfg, ""), false, &state.store.settings()).await.unwrap());
        let fingerprint = db::runtime::binding_fingerprint_at(&cfg, state.store.data_root()).unwrap();
        state.connection_runtime.publish(&cfg.id, &fingerprint, &db, None, db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
        state.registry.write().insert(cfg.id, db);
        let executor = Arc::new(DesktopExecutor::from_state(state.clone()));
        let host = SchedulerHost::with_executor(root.clone(), SchedulerOwner::new(RunnerMode::Gui),
            Arc::new(SystemSchedulerClock::default()), executor.clone());
        Self { root, state, host, executor }
    }
    fn sql_template(&self, sql: &str) -> ActionTemplate {
        self.host.service.save_template(ActionTemplate { id: uuid::Uuid::new_v4().to_string(), revision: String::new(),
            name: "real desktop SQL".into(), kind: ActionKind::Sql, connection_id: "scheduler-real-connection".into(),
            namespace: "main".into(), read_only: false, configuration: TemplateConfiguration::Sql { sql: sql.into() } }, None).unwrap()
    }
    fn job(&self, templates: &[ActionTemplate]) -> JobRecord {
        self.job_with_timeout(templates, 10_000)
    }
    fn job_with_timeout(&self, templates: &[ActionTemplate], timeout_ms: u64) -> JobRecord {
        let mut actions = Vec::new();
        for template in templates {
            let depends_on = actions.last().map(|action: &ScheduledAction| vec![action.id.clone()]).unwrap_or_default();
            actions.push(ScheduledAction { id: uuid::Uuid::new_v4().to_string(), kind: template.kind,
                depends_on, template_ref: template.id.clone(), approved_scope_revision: String::new() });
        }
        self.host.service.save_job(ScheduledJobV2 { version: SCHEDULER_VERSION, id: uuid::Uuid::new_v4().to_string(),
            revision: String::new(), name: "real desktop integration".into(), enabled: false,
            execution_mode: ExecutionMode::AppOnline, schedule: ScheduleSpec::Manual,
            misfire: MisfirePolicy::LatestOnce, overlap: OverlapPolicy::Skip, timeout_ms, actions }, None).unwrap()
    }
    async fn approve(&self, record: &JobRecord) -> JobRecord {
        let preview = self.host.service.prepare_approval(&record.job.id, &record.job.revision, PRINCIPAL).await.unwrap();
        assert!(!preview.actions.is_empty());
        assert!(self.host.service.approve_job(&preview.plan_id, &preview.review_digest, "foreign-window").await.is_err());
        let approved = self.host.service.approve_job(&preview.plan_id, &preview.review_digest, PRINCIPAL).await.unwrap();
        assert!(approved.paused_reason.is_none());
        assert!(self.host.service.approve_job(&preview.plan_id, &preview.review_digest, PRINCIPAL).await.is_err());
        approved
    }
    async fn count(&self) -> i64 {
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(self.root.join("test.sqlite"));
        let mut observer = sqlx::SqliteConnection::connect_with(&options).await.unwrap();
        let count = sqlx::query_scalar("SELECT count(*) FROM items").fetch_one(&mut observer).await.unwrap();
        observer.close().await.unwrap();
        count
    }
    async fn terminal(&self, id: &str) -> SchedulerRunSummary {
        // Observe persisted completion, not a fixed delay used as evidence of SQL/resource drainage.
        // 15s 在并行全量下会被 CPU 争抢耗尽（单测约 3.5s）；放宽到 90s 仅作挂死保护。
        tokio::time::timeout(Duration::from_secs(90), async {
            let mut observation = tokio::time::interval(Duration::from_millis(10));
            loop {
                observation.tick().await;
                if let Some(run) = self.host.service.history(None).unwrap().into_iter().find(|run| run.id == id) {
                    if run.status.terminal() && !run.resources_pending { return run; }
                }
            }
        }).await.expect("real desktop scheduler did not settle its durable run")
    }
    async fn cleanup(self) {
        let Self { root, state, host, executor } = self;
        drop(host);
        drop(executor);
        drop(state.manual_transactions.close_all().await.unwrap());
        let connections: Vec<_> = state.registry.write().drain().map(|(_, db)| db).collect();
        for db in connections { if let Db::Sqlite(pool) = db.as_ref() { pool.close().await; assert!(pool.is_closed()); assert_eq!(pool.size(), 0); } }
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn encrypted_job_and_template_opaque_approval_run_real_c1_sql_once_and_record_history() {
    let fixture = Fixture::new().await;
    let template = fixture.sql_template(INSERT_SQL);
    let record = fixture.job(&[template.clone()]);
    let approved = fixture.approve(&record).await;
    assert_eq!(fixture.count().await, 0, "review and approval must not execute SQL");
    let reopened = SchedulerStore::new(fixture.root.clone());
    assert_eq!(reopened.load_template(&template.id).unwrap(), template, "real AES-GCM file must decrypt with original local key");
    assert_eq!(reopened.list_jobs().unwrap()[0], approved);
    for path in [fixture.root.join("scheduler/state.enc"), fixture.root.join(format!("scheduler/template-{}.enc", template.id))] {
        let bytes = std::fs::read(path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(PRIVATE_VALUE), "encrypted scheduler files must not expose SQL literal");
    }
    let request = uuid::Uuid::new_v4().to_string();
    let claimed = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &request).await.unwrap().unwrap();
    let run = fixture.terminal(&claimed.id).await;
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(fixture.count().await, 1);
    assert_eq!(run.actions.len(), 1);
    let receipt = run.actions[0].receipt.as_ref().unwrap();
    assert_eq!(receipt.status, ActionStatus::Succeeded);
    assert!(receipt.verified && !receipt.resources_pending);
    assert_eq!(receipt.effects, ActionEffects::Committed);
    let execution_id = receipt.execution_id.as_deref().expect("real desktop SQL receipt must name its C1 execution");
    let cfg = commands::saved_config(&fixture.state, "scheduler-real-connection").unwrap();
    let binding = commands::execution_binding(&fixture.state, &cfg,
        &format!("scheduler:{}", fixture.host.service.runner.owner.instance_id),
        &format!("job:{}/{}/{}", approved.job.id, run.id, approved.job.actions[0].id), "main").unwrap();
    let status = fixture.state.executions.authorized(execution_id, &binding).unwrap().status();
    assert_eq!(status.state, ExecutionState::Succeeded);
    assert!(!status.resources_pending);
    let history = fixture.host.service.history(Some(&approved.job.id)).unwrap();
    assert_eq!(history.len(), 1); assert_eq!(history[0].id, run.id);
    assert!(!serde_json::to_string(&history).unwrap().contains(PRIVATE_VALUE));
    assert!(fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &request).await.unwrap().is_none());
    assert_eq!(fixture.count().await, 1, "a consumed manual request must never replay SQL");
    fixture.cleanup().await;
}

#[tokio::test]
async fn approved_sql_then_export_dag_writes_real_unique_files_and_never_overwrites_earlier_outputs() {
    let fixture = Fixture::new().await;
    let insert = fixture.sql_template(INSERT_SQL);
    let pattern = fixture.root.join("items.csv");
    std::fs::write(&pattern, b"pre-existing-output-must-remain").unwrap();
    let definition = crate::scheduler_tools::ToolDefinition::Export(crate::models::ExportOptions {
        connection_id: insert.connection_id.clone(), schema: "main".into(), table: "items".into(),
        file: pattern.to_string_lossy().into_owned(), format: "csv".into(), has_header: true,
        delimiter: ",".into(), encoding: "utf-8".into(), where_clause: None, limit: None,
    });
    let export = fixture.host.service.save_template(ActionTemplate {
        id: uuid::Uuid::new_v4().to_string(), revision: String::new(), name: "real desktop export".into(),
        kind: ActionKind::Export, connection_id: insert.connection_id.clone(), namespace: "main".into(),
        read_only: true, configuration: TemplateConfiguration::Tool { options_json: serde_json::to_string(&definition).unwrap() },
    }, None).unwrap();
    // This success-path test includes several durable fsync/crypto operations.
    // Keep cancellation/deadline tests on the original 10s job budget, but give
    // this two-action, real-file DAG a 60s execution budget under CI load.
    let approved = fixture.approve(&fixture.job_with_timeout(&[insert, export], 60_000)).await;
    assert_eq!(approved.job.actions[1].depends_on, vec![approved.job.actions[0].id.clone()]);
    let first = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &uuid::Uuid::new_v4().to_string()).await.unwrap().unwrap();
    let first = fixture.terminal(&first.id).await;
    assert_eq!(first.status, RunStatus::Succeeded, "{:?}", first.actions);
    assert_eq!(fixture.count().await, 1);
    assert_eq!(first.actions.iter().map(|a| a.status).collect::<Vec<_>>(), vec![ActionStatus::Succeeded, ActionStatus::Succeeded]);
    let export_receipt = first.actions[1].receipt.as_ref().unwrap();
    assert!(export_receipt.verified && !export_receipt.resources_pending);
    assert_eq!(export_receipt.affected_rows.as_deref(), Some("1"));
    assert!(export_receipt.execution_id.is_none(), "real export must use the production tool service");
    assert!(first.actions[0].completed_at_utc.unwrap() <= first.actions[1].started_at_utc.unwrap());
    let first_path = fixture.root.join(format!("items-{}.csv", first.id));
    let first_bytes = std::fs::read(&first_path).unwrap();
    let csv = String::from_utf8(first_bytes.clone()).unwrap();
    assert!(csv.contains("id") && csv.contains("v") && csv.contains(PRIVATE_VALUE));
    assert_eq!(csv.matches(PRIVATE_VALUE).count(), 1, "export must observe the preceding committed SQL row");
    let second = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &uuid::Uuid::new_v4().to_string()).await.unwrap().unwrap();
    let second = fixture.terminal(&second.id).await;
    assert_eq!(second.status, RunStatus::Succeeded, "{:?}", second.actions);
    assert_ne!(first.id, second.id);
    assert_eq!(fixture.count().await, 2);
    assert_eq!(second.actions[1].receipt.as_ref().unwrap().affected_rows.as_deref(), Some("2"));
    let second_path = fixture.root.join(format!("items-{}.csv", second.id));
    assert_ne!(first_path, second_path);
    assert_eq!(String::from_utf8(std::fs::read(second_path).unwrap()).unwrap().matches(PRIVATE_VALUE).count(), 2);
    assert_eq!(std::fs::read(&first_path).unwrap(), first_bytes, "a later approved run must not overwrite an earlier run's artifact");
    assert_eq!(std::fs::read(pattern).unwrap(), b"pre-existing-output-must-remain");
    let history = fixture.host.service.history(Some(&approved.job.id)).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|run| run.actions.iter().all(|action| action.receipt.as_ref().is_some_and(|receipt| receipt.verified))));
    fixture.cleanup().await;
}

#[tokio::test]
async fn changed_template_or_actual_saved_endpoint_invalidates_opaque_approval_and_sends_zero_sql() {
    let fixture = Fixture::new().await;
    let template = fixture.sql_template(INSERT_SQL);
    let record = fixture.job(&[template.clone()]);
    let preview = fixture.host.service.prepare_approval(&record.job.id, &record.job.revision, PRINCIPAL).await.unwrap();
    let mut edited = template.clone();
    edited.configuration = TemplateConfiguration::Sql { sql: "INSERT INTO items(v) VALUES ('unreviewed-template')".into() };
    fixture.host.service.save_template(edited, Some(&template.revision)).unwrap();
    let error = fixture.host.service.approve_job(&preview.plan_id, &preview.review_digest, PRINCIPAL).await.unwrap_err();
    assert!(error.to_string().contains("SCHEDULE_SCOPE_CHANGED"), "{error:#}");
    assert!(fixture.host.service.list_jobs().unwrap()[0].approvals.is_empty());
    assert!(fixture.host.service.run_now(&record.job.id, &record.job.revision, &uuid::Uuid::new_v4().to_string()).await.is_err());
    assert_eq!(fixture.count().await, 0);
    assert!(fixture.host.service.history(None).unwrap().is_empty());
    fixture.cleanup().await;

    let fixture = Fixture::new().await;
    let template = fixture.sql_template(INSERT_SQL);
    let approved = fixture.approve(&fixture.job(&[template])).await;
    let preview = fixture.host.service.prepare_approval(&approved.job.id, &approved.job.revision, PRINCIPAL).await.unwrap();
    let other_file = fixture.root.join("different-endpoint.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&other_file).create_if_missing(true);
    let mut other = sqlx::SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, v TEXT NOT NULL)").execute(&mut other).await.unwrap();
    let mut cfg = commands::saved_config(&fixture.state, "scheduler-real-connection").unwrap();
    cfg.file = other_file.to_string_lossy().into_owned();
    // Change the real saved endpoint; no fabricated scope or executor return value is injected.
    fixture.state.store.save_connections(vec![cfg], vec![]).unwrap();
    let error = fixture.host.service.approve_job(&preview.plan_id, &preview.review_digest, PRINCIPAL).await.unwrap_err();
    assert!(error.to_string().contains("SCHEDULE_SCOPE_CHANGED"), "{error:#}");
    let claimed = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &uuid::Uuid::new_v4().to_string()).await.unwrap().unwrap();
    let run = fixture.terminal(&claimed.id).await;
    assert_eq!(run.status, RunStatus::AwaitingConfirmation);
    assert!(!run.resources_pending);
    assert_eq!(run.actions[0].status, ActionStatus::AwaitingConfirmation);
    assert!(run.actions[0].started_at_utc.is_none(), "changed actual scope must be rejected before dispatch journal and C1 send");
    let receipt = run.actions[0].receipt.as_ref().unwrap();
    assert_eq!(receipt.error_code.as_deref(), Some("SCHEDULE_SCOPE_CHANGED"));
    assert_eq!(receipt.effects, ActionEffects::None);
    assert!(receipt.verified && receipt.execution_id.is_none());
    assert_eq!(fixture.count().await, 0);
    let other_count: i64 = sqlx::query_scalar("SELECT count(*) FROM items").fetch_one(&mut other).await.unwrap();
    assert_eq!(other_count, 0, "neither the approved nor replacement endpoint may receive the stale SQL");
    other.close().await.unwrap();
    assert!(!fixture.state.executions.has_active("scheduler-real-connection"));
    assert_eq!(fixture.host.service.list_jobs().unwrap()[0].blocked_run_id.as_deref(), Some(run.id.as_str()));
    fixture.cleanup().await;
}

#[tokio::test]
async fn real_c1_stop_prevents_dependent_sql_and_never_turns_an_uncertain_stop_into_success() {
    let fixture = Fixture::new().await;
    let long = fixture.sql_template("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n");
    let dependent = fixture.sql_template(INSERT_SQL);
    let approved = fixture.approve(&fixture.job(&[long, dependent])).await;
    let claimed = fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &uuid::Uuid::new_v4().to_string()).await.unwrap().unwrap();
    let key = format!("{}/{}", claimed.id, approved.job.actions[0].id);
    let entry = tokio::time::timeout(Duration::from_secs(5), async {
        let mut observation = tokio::time::interval(Duration::from_millis(5));
        loop {
            observation.tick().await;
            let proof = fixture.executor.executions.lock().get(&key).map(|proof| (proof.id.clone(), proof.binding.clone()));
            if let Some((id, binding)) = proof {
                if let Ok(entry) = fixture.state.executions.authorized(&id, &binding) {
                    entry.wait_started().await;
                    assert_eq!(entry.status().state, ExecutionState::Running);
                    return entry;
                }
            }
        }
    }).await.expect("production DesktopExecutor did not start the real C1 query");
    let cancelled = fixture.host.service.cancel_run(&claimed.id).unwrap();
    assert!(cancelled.cancel_requested);
    let run = fixture.terminal(&claimed.id).await;
    assert!(matches!(run.status, RunStatus::Cancelled | RunStatus::Unknown), "{:?}", run);
    assert!(run.cancel_requested);
    assert!(!run.resources_pending);
    let receipt = run.actions[0].receipt.as_ref().unwrap();
    assert_eq!(receipt.execution_id.as_deref(), Some(entry.id.as_str()));
    assert!(matches!(receipt.status, ActionStatus::Cancelled | ActionStatus::Unknown));
    assert!(!receipt.resources_pending);
    assert_eq!(entry.status().state, ExecutionState::Cancelled);
    assert!(!entry.status().resources_pending);
    assert_eq!(run.actions[1].status, ActionStatus::Blocked);
    assert!(run.actions[1].started_at_utc.is_none() && run.actions[1].receipt.is_none());
    assert_eq!(fixture.count().await, 0, "dependent SQL must never run after a stop/unknown upstream");
    if run.status == RunStatus::Unknown {
        assert_eq!(receipt.effects, ActionEffects::Unknown);
        assert!(!receipt.verified);
        assert_eq!(fixture.host.service.list_jobs().unwrap()[0].blocked_run_id.as_deref(), Some(run.id.as_str()));
        assert!(fixture.host.service.run_now(&approved.job.id, &approved.job.revision, &uuid::Uuid::new_v4().to_string()).await.is_err());
    } else { assert!(receipt.verified); assert_eq!(receipt.effects, ActionEffects::None); }
    fixture.cleanup().await;
}
