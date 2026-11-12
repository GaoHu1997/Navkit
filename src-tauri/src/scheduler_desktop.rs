//! Desktop owner of the durable scheduler. User work is dispatched through C1/tool services.
//! An IPC caller only reviews an opaque plan; it never supplies an approved endpoint or scope.
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use anyhow::{anyhow, ensure, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use tauri::State;
use crate::{commands::{self, AppState}, error::{AppError, CmdResult}, execution::Binding,
    execution_contracts::{ExecutionOutcome, ExecutionRequest, ExecutionState},
    scheduler::*};
#[path = "scheduler_state_access.rs"] pub(crate) mod state_access;
use state_access::SchedulerStateAccess;

pub struct SchedulerHost {
    pub service: Arc<SchedulerService>,
    handle: Mutex<Option<SchedulerServiceHandle>>,
    migrations: Arc<LegacyMigrationRegistry>,
}
impl SchedulerHost {
    pub fn new(app: tauri::AppHandle, root: PathBuf) -> Self {
        let service = Arc::new(SchedulerService::new(SchedulerStore::new(root),
            SchedulerOwner::new(RunnerMode::Gui), Arc::new(SystemSchedulerClock::default()),
            Arc::new(DesktopExecutor { state: SchedulerStateAccess::Desktop(app), executions: Mutex::new(HashMap::new()) })));
        Self { service, handle: Mutex::new(None), migrations: Arc::new(LegacyMigrationRegistry::default()) }
    }
    pub(crate) fn with_executor(root: PathBuf, owner: SchedulerOwner, clock: Arc<dyn SchedulerClock>, executor: Arc<dyn SchedulerExecutor>) -> Self {
        let service = Arc::new(SchedulerService::new(SchedulerStore::new(root), owner, clock, executor));
        Self { service, handle: Mutex::new(None), migrations: Arc::new(LegacyMigrationRegistry::default()) }
    }
    pub fn start(&self) -> Result<()> {
        let mut slot = self.handle.lock();
        if slot.is_none() { *slot = Some(self.service.start()?); }
        Ok(())
    }
    pub fn request_stop(&self) { self.handle.lock().take(); }
}

/// Kept for Unknown/pending actions until C1 proves the physical work/control have drained.
#[derive(Clone)]
struct ExecutionProof { id: String, binding: Binding }
pub(crate) struct DesktopExecutor {
    state: SchedulerStateAccess,
    executions: Mutex<HashMap<String, ExecutionProof>>,
}
impl DesktopExecutor {
    pub(crate) fn from_state(state: Arc<AppState>) -> Self {
        Self { state: SchedulerStateAccess::Owned(state), executions: Mutex::new(HashMap::new()) }
    }
}
fn action_key(context: &ActionExecutionContext) -> String { format!("{}/{}", context.run_id, context.action_id) }
fn sql_scope(state: &AppState, template: &ActionTemplate) -> Result<ActionScope> {
    validate_template(template)?;
    let TemplateConfiguration::Sql { sql } = &template.configuration else { return Err(anyhow!("SCHEDULE_TEMPLATE_KIND")); };
    ensure!(template.kind == ActionKind::Sql, "SCHEDULE_TEMPLATE_KIND");
    crate::execution_policy::ordinary_native_batch(sql).map_err(|e| anyhow!("{}", e.code))?;
    if template.read_only { crate::execution_policy::readonly_batch(sql).map_err(|e| anyhow!("{}", e.code))?; }
    let cfg = commands::saved_config(state, &template.connection_id).map_err(|e| anyhow!("{}", e.code))?;
    ensure!(matches!(cfg.kind.protocol(), crate::models::Protocol::Sqlite | crate::models::Protocol::Mysql | crate::models::Protocol::Postgres)
        && !cfg.params.starts_with(crate::driver_manager::PARAMS_PREFIX),
        "SCHEDULE_SQL_CHANNEL_UNPROVEN: 此调度 SQL 通道尚未验证取消与排空");
    if cfg.read_only { crate::execution_policy::readonly_batch(sql).map_err(|e| anyhow!("{}", e.code))?; }
    let fingerprint = crate::db::runtime::binding_fingerprint_at(&cfg,state.store.data_root())?;
    let permission_revision = digest(&(2u32, "c1-scheduler-sql", &cfg.kind, cfg.read_only,
        template.read_only, &fingerprint, &template.namespace, sql))?;
    Ok(ActionScope { template_ref: template.id.clone(), template_revision: template.revision.clone(),
        template_digest: digest(template)?, connection_id: cfg.id.clone(), connection_ids: vec![cfg.id],
        endpoint_fingerprint: fingerprint, namespace: template.namespace.clone(), read_only: template.read_only,
        permission_revision: permission_revision.clone(), destructive_limit: sql_destructive_limit(sql, template.read_only || cfg.read_only, &permission_revision) })
}
fn sql_destructive_limit(_sql: &str, read_only: bool, plan_revision: &str) -> Option<DestructiveLimit> {
    if read_only { return None; }
    // An exact SQL template is reviewed, not a generic grant to a database. An unproven
    // read-only statement can call side-effecting functions; never label it risk-free.
    Some(DestructiveLimit { plan_revision: plan_revision.into(), max_delete_rows: None,
        allow_overwrite: true, allow_drop: true })
}
fn receipt(outcome: &ExecutionOutcome, read_only: bool) -> ActionReceipt {
    let clean = !outcome.resources_pending;
    let proved_none = matches!(outcome.effects.as_str(), "none" | "no_effect") || outcome.transaction_state == "rolled_back";
    let unknown = outcome.state == ExecutionState::Unknown || !clean
        || (outcome.state != ExecutionState::Succeeded && !proved_none);
    let status = if unknown { ActionStatus::Unknown } else { match outcome.state {
        ExecutionState::Succeeded => ActionStatus::Succeeded,
        ExecutionState::Cancelled => ActionStatus::Cancelled,
        ExecutionState::TimedOut => ActionStatus::TimedOut,
        _ => ActionStatus::Failed,
    }};
    ActionReceipt { status, verified: !unknown,
        effects: if unknown { ActionEffects::Unknown } else if read_only || proved_none { ActionEffects::None } else { ActionEffects::Committed },
        resources_pending: outcome.resources_pending, execution_id: Some(outcome.execution_id.clone()),
        affected_rows: None, error_code: outcome.error.as_ref().map(|e| safe_code(&e.code)) }
}
#[async_trait]
impl SchedulerExecutor for DesktopExecutor {
    async fn resolve_scope(&self, template: &ActionTemplate, _context: &ActionExecutionContext) -> Result<ActionScope> {
        let state = self.state.get();
        if template.kind == ActionKind::Sql { sql_scope(&state, template) }
        else { Box::pin(crate::scheduler_tools::resolve_scope_state(&state, template)).await }
    }
    async fn execute(&self, template: ActionTemplate, approved: ActionScope, context: ActionExecutionContext) -> Result<ActionReceipt> {
        ensure!(!context.owner_id.starts_with("review:") && context.origin == "job" && context.mode == RunnerMode::Gui,
            "SCHEDULE_EXECUTION_CONTEXT_INVALID");
        if template.kind != ActionKind::Sql { return Box::pin(crate::scheduler_tools::execute_state(self.state.clone(), template, approved, context)).await; }
        let state = self.state.get();
        let current = sql_scope(&state, &template)?;
        ensure!(current == approved, "SCHEDULE_SCOPE_CHANGED");
        let TemplateConfiguration::Sql { sql } = &template.configuration else { return Err(anyhow!("SCHEDULE_TEMPLATE_KIND")); };
        let principal = format!("scheduler:{}", context.instance.instance_id);
        let mut request = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(),
            connection_id: template.connection_id.clone(), namespace: template.namespace.clone(),
            owner_id: context.owner_id.clone(), sql: sql.clone(), transaction_owner: None,
            timeout_ms: Some(context.timeout_ms), origin: Some("job".into()), execution_grant: None,
            cancel_before_send: context.stop_reason().is_some() };
        commands::grant_execution_request(&state, &mut request, &principal).map_err(|e| anyhow!("{}", e.code))?;
        let cfg = commands::saved_config(&state, &template.connection_id).map_err(|e| anyhow!("{}", e.code))?;
        let binding = commands::execution_binding(&state, &cfg, &principal, &context.owner_id, &template.namespace).map_err(|e| anyhow!("{}", e.code))?;
        ensure!(binding.fingerprint == approved.endpoint_fingerprint, "SCHEDULE_SCOPE_CHANGED");
        let id = request.execution_id.clone(); let key = action_key(&context);
        self.executions.lock().insert(key.clone(), ExecutionProof { id: id.clone(), binding: binding.clone() });
        let work = Box::pin(commands::execute_sql_service(&state, request, &principal)); tokio::pin!(work);
        let mut ticks = tokio::time::interval(Duration::from_millis(25)); ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop { tokio::select! {
            value = &mut work => break value,
            _ = ticks.tick() => {
                if context.stop_reason().is_none() { continue; }
                if let Ok(entry) = state.executions.authorized(&id, &binding) {
                    entry.request_stop(matches!(context.stop_reason(), Some(SchedulerStopReason::Deadline)));
                }
            }
        }};
        match result {
            Ok(outcome) => {
                let result = receipt(&outcome, template.read_only || cfg.read_only);
                if !outcome.resources_pending { self.executions.lock().remove(&key); }
                result.validate()?; Ok(result)
            }
            Err(error) => {
                // C1 registration precedes all sends. A missing entry proves this attempt sent no SQL.
                let entry = state.executions.authorized(&id, &binding);
                if matches!(&entry, Err(e) if e.code == "EXECUTION_NOT_FOUND") {
                    self.executions.lock().remove(&key); return Ok(ActionReceipt::failed(&safe_code(&error.code)));
                }
                Ok(ActionReceipt { status: ActionStatus::Unknown, verified: false, effects: ActionEffects::Unknown,
                    resources_pending: true, execution_id: Some(id), affected_rows: None, error_code: Some(safe_code(&error.code)) })
            }
        }
    }
    async fn resources_drained(&self, context: &ActionExecutionContext, result: &ActionReceipt) -> Result<bool> {
        if result.execution_id.is_none() { return crate::scheduler_tools::resources_drained_state(context, result).await; }
        let key = action_key(context);
        let proof = self.executions.lock().get(&key).cloned();
        let Some(proof) = proof else {
            return Box::pin(crate::scheduler_tools::resources_drained_state(context, result)).await;
        };
        let state = self.state.get();
        let Ok(entry) = state.executions.authorized(&proof.id, &proof.binding) else { return Ok(false); };
        let status = entry.status(); let drained = status.state.terminal() && !status.resources_pending;
        if drained { self.executions.lock().remove(&key); } Ok(drained)
    }
}

fn host_error(error: anyhow::Error) -> AppError {
    // Never include raw SQL, file paths or connection secrets in scheduler public errors.
    let text = error.to_string(); let code = text.split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).next().unwrap_or("");
    AppError::coded(&safe_code(code), "调度操作未完成；请核对任务版本、连接范围和运行记录")
}
async fn local<T, F>(host: &SchedulerHost, action: F) -> CmdResult<T>
where T: Send + 'static, F: FnOnce(Arc<SchedulerService>) -> Result<T> + Send + 'static {
    let service = host.service.clone(); tokio::task::spawn_blocking(move || action(service)).await
        .map_err(|_| AppError::coded("SCHEDULE_SERVICE_UNAVAILABLE", "本机调度存储任务未完成"))?.map_err(host_error)
}
#[tauri::command]
pub async fn scheduler_list_jobs(host: State<'_, SchedulerHost>) -> CmdResult<Vec<JobRecord>> { local(&host, |s| s.list_jobs()).await }
#[tauri::command]
pub async fn scheduler_list_templates(host: State<'_, SchedulerHost>) -> CmdResult<Vec<ActionTemplateSummary>> { local(&host, |s| s.list_templates()).await }
#[tauri::command]
pub async fn scheduler_load_template(host: State<'_, SchedulerHost>, id: String) -> CmdResult<ActionTemplate> { local(&host, move |s| s.load_template(&id)).await }
#[tauri::command]
pub async fn scheduler_save_template(state: State<'_, AppState>, host: State<'_, SchedulerHost>, template: ActionTemplate, expected_revision: Option<String>) -> CmdResult<ActionTemplate> {
    state.store.ensure_encryption_key().map_err(AppError::from)?;
    local(&host, move |s| s.save_template(template, expected_revision.as_deref())).await
}
#[tauri::command]
pub async fn scheduler_save_job(state: State<'_, AppState>, host: State<'_, SchedulerHost>, job: ScheduledJobV2, expected_revision: Option<String>) -> CmdResult<JobRecord> {
    state.store.ensure_encryption_key().map_err(AppError::from)?;
    let result = local(&host, move |s| s.save_job(job, expected_revision.as_deref())).await?;
    host.start().map_err(host_error)?; Ok(result)
}
#[tauri::command]
pub async fn scheduler_pause_job(host: State<'_, SchedulerHost>, id: String, revision: String) -> CmdResult<JobRecord> { local(&host, move |s| s.pause_job(&id, &revision)).await }
#[tauri::command]
pub async fn scheduler_delete_job(host: State<'_, SchedulerHost>, id: String, revision: String) -> CmdResult<()> { local(&host, move |s| s.delete_job(&id, &revision)).await }
#[tauri::command]
pub async fn scheduler_history(host: State<'_, SchedulerHost>, job_id: Option<String>) -> CmdResult<Vec<SchedulerRunSummary>> { local(&host, move |s| s.history(job_id.as_deref())).await }
#[tauri::command]
pub async fn scheduler_cancel_run(host: State<'_, SchedulerHost>, run_id: String) -> CmdResult<SchedulerRunSummary> { local(&host, move |s| s.cancel_run(&run_id)).await }
#[tauri::command]
pub async fn scheduler_preview_times(host: State<'_, SchedulerHost>, schedule: ScheduleSpec) -> CmdResult<Vec<NextOccurrence>> { local(&host, move |s| s.preview_times(&schedule)).await }
#[tauri::command]
pub async fn scheduler_prepare_approval(window: tauri::WebviewWindow, host: State<'_, SchedulerHost>, id: String, revision: String) -> CmdResult<SchedulerApprovalPreview> {
    host.service.prepare_approval(&id, &revision, window.label()).await.map_err(host_error)
}
#[tauri::command]
pub async fn scheduler_approve_job(window: tauri::WebviewWindow, host: State<'_, SchedulerHost>, plan_id: String, review_digest: String) -> CmdResult<JobRecord> {
    host.service.approve_job(&plan_id, &review_digest, window.label()).await.map_err(host_error)
}
#[tauri::command]
pub async fn scheduler_run_now(host: State<'_, SchedulerHost>, id: String, revision: String, request_id: String) -> CmdResult<Option<SchedulerRunSummary>> {
    host.service.run_now(&id, &revision, &request_id).await.map_err(host_error)
}

#[tauri::command]
pub async fn scheduler_prepare_legacy_migration(window: tauri::WebviewWindow, host: State<'_, SchedulerHost>, timezone: String) -> CmdResult<LegacyMigrationPreview> {
    let registry = host.migrations.clone(); let store = host.service.store.clone(); let principal = window.label().to_owned();
    tokio::task::spawn_blocking(move || registry.prepare(&store, &timezone, &principal, chrono::Utc::now())).await
        .map_err(|_| AppError::coded("SCHEDULE_SERVICE_UNAVAILABLE", "旧任务迁移预览未完成，原定义保持原样"))?.map_err(host_error)
}
#[tauri::command]
pub async fn scheduler_confirm_legacy_migration(window: tauri::WebviewWindow, state: State<'_, AppState>, host: State<'_, SchedulerHost>, plan_id: String, review_digest: String) -> CmdResult<LegacyMigrationReceipt> {
    state.store.ensure_encryption_key().map_err(AppError::from)?;
    let registry = host.migrations.clone(); let store = host.service.store.clone(); let principal = window.label().to_owned();
    // Once dispatched, storage work is owned independently of a lost IPC acknowledgement.
    tokio::task::spawn_blocking(move || registry.confirm(&store, &plan_id, &review_digest, &principal)).await
        .map_err(|_| AppError::coded("SCHEDULE_SERVICE_UNAVAILABLE", "旧任务迁移结果未确认，请刷新核对本机任务，勿重复提交"))?.map_err(host_error)
}

#[cfg(test)]
#[path = "scheduler_desktop_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_contracts::DeadlineInfo;
    fn outcome(state: ExecutionState, effects: &str, pending: bool) -> ExecutionOutcome {
        ExecutionOutcome { execution_id: uuid::Uuid::new_v4().to_string(), state, result: None, error: None,
            deadline: DeadlineInfo { execution_id: "fixture".into(), budget_ms: Some(100), elapsed_ms: 1, deadline_reached: false, stop_confirmed: false },
            effects: effects.into(), transaction_state: "none".into(), resources_pending: pending }
    }
    #[test] fn failed_sql_with_possible_writes_keeps_unknown() { let r=receipt(&outcome(ExecutionState::Failed,"possible",false),false); assert_eq!(r.status,ActionStatus::Unknown); assert!(!r.verified); r.validate().unwrap(); }
    #[test] fn stopped_sql_is_not_assumed_rolled_back() { for s in [ExecutionState::Cancelled,ExecutionState::TimedOut] { assert_eq!(receipt(&outcome(s,"possible",false),false).status,ActionStatus::Unknown); } }
    #[test] fn confirmed_zero_effect_stop_preserves_status() { let r=receipt(&outcome(ExecutionState::Cancelled,"no_effect",false),false); assert_eq!(r.status,ActionStatus::Cancelled); assert_eq!(r.effects,ActionEffects::None); r.validate().unwrap(); }
    #[test] fn pending_resources_can_never_be_success() { let r=receipt(&outcome(ExecutionState::Succeeded,"none",true),true); assert_eq!(r.status,ActionStatus::Unknown); r.validate().unwrap(); }
    #[test] fn writable_or_side_effecting_sql_is_never_a_risk_free_approval() { for sql in ["DELETE FROM items", "UPDATE items SET qty = 0", "DROP TABLE items", "SELECT arbitrary_user_function()"] { let limit=sql_destructive_limit(sql,false,&"a".repeat(64)).unwrap(); assert!(limit.allow_drop && limit.allow_overwrite); assert!(limit.max_delete_rows.is_none()); assert_eq!(limit.plan_revision,"a".repeat(64)); } }
    #[test] fn proved_readonly_sql_does_not_request_destructive_consent() { assert!(sql_destructive_limit("SELECT 1",true,&"a".repeat(64)).is_none()); }
    #[test] fn select_syntax_is_not_a_database_enforced_readonly_proof() { for sql in ["SELECT 1", "SELECT nextval('sequence')", "SELECT side_effect()"] { assert!(sql_destructive_limit(sql,false,&"a".repeat(64)).is_some()); } }
    #[test] fn readonly_success_has_no_write_effect() { let r=receipt(&outcome(ExecutionState::Succeeded,"possible",false),true); assert_eq!(r.status,ActionStatus::Succeeded); assert_eq!(r.effects,ActionEffects::None); }
}
