//! IPC-facing PL-25 service. Only opaque, window-bound review IDs cross approve IPC.
//! All scopes are resolved by the same trusted SchedulerExecutor during review and approval.
use super::{clock::*, contracts::*, runner::*, store::SchedulerStore};
use anyhow::{anyhow, ensure, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::Duration as StdDuration};

const REVIEW_TTL_MS: u64 = 120_000;
const MAX_REVIEWS: usize = 64;

#[derive(Debug, Clone, Serialize)]
pub struct SchedulerApprovalAction {
    pub action_id: String, pub template_name: String, pub connection_label: String,
    pub namespace: String, pub read_only: bool, pub details: Vec<String>, pub destructive: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SchedulerApprovalPreview {
    pub plan_id: String, pub review_digest: String, pub job_id: String,
    pub job_revision: String, pub expires_at_utc: DateTime<Utc>,
    pub actions: Vec<SchedulerApprovalAction>,
}
struct PreparedApproval {
    principal: String, preview: SchedulerApprovalPreview, created_mono: u64,
    job: ScheduledJobV2, scopes: Vec<(String, ActionScope)>,
}
/// Journal projection intentionally excludes lease tokens, encrypted template content and owner internals.
#[derive(Debug, Clone, Serialize)]
pub struct SchedulerRunSummary {
    pub id: String, pub job_id: String, pub job_revision: String, pub schedule_revision: String,
    pub scheduled_at_utc: DateTime<Utc>, pub claimed_at_utc: DateTime<Utc>,
    pub started_at_utc: Option<DateTime<Utc>>, pub completed_at_utc: Option<DateTime<Utc>>,
    pub status: RunStatus, pub resources_pending: bool, pub cancel_requested: bool,
    pub actions: Vec<ActionJournal>, pub journal: Vec<JournalEvent>,
}
impl From<ScheduledRun> for SchedulerRunSummary {
    fn from(r: ScheduledRun) -> Self { Self { id:r.id, job_id:r.job_id, job_revision:r.job_revision,
        schedule_revision:r.schedule_revision, scheduled_at_utc:r.scheduled_at_utc,
        claimed_at_utc:r.claimed_at_utc, started_at_utc:r.started_at_utc, completed_at_utc:r.completed_at_utc,
        status:r.status, resources_pending:r.resources_pending, cancel_requested:r.cancel_requested,
        actions:r.actions, journal:r.journal } }
}

pub struct SchedulerService {
    pub store: SchedulerStore,
    pub runner: Arc<SchedulerRunner>,
    clock: Arc<dyn SchedulerClock>,
    executor: Arc<dyn SchedulerExecutor>,
    reviews: Mutex<BTreeMap<String, PreparedApproval>>,
    started: AtomicBool,
}
impl SchedulerService {
    pub fn new(store: SchedulerStore, owner: SchedulerOwner, clock: Arc<dyn SchedulerClock>, executor: Arc<dyn SchedulerExecutor>) -> Self {
        let runner=Arc::new(SchedulerRunner::new(store.clone(),owner,clock.clone(),executor.clone()));
        Self { store, runner, clock, executor, reviews:Mutex::new(BTreeMap::new()), started:AtomicBool::new(false) }
    }
    pub fn list_jobs(&self) -> Result<Vec<JobRecord>> { self.store.list_jobs() }
    pub fn list_templates(&self) -> Result<Vec<ActionTemplateSummary>> { self.store.list_templates() }
    pub fn load_template(&self, id:&str) -> Result<ActionTemplate> { self.store.load_template(id) }
    pub fn save_template(&self, template:ActionTemplate, expected:Option<&str>) -> Result<ActionTemplate> { self.store.save_template(template,expected) }
    pub fn save_job(&self, job:ScheduledJobV2, expected:Option<&str>) -> Result<JobRecord> { self.store.save_job(job,expected,self.clock.now_utc()) }
    pub fn pause_job(&self, id:&str, expected:&str) -> Result<JobRecord> { self.store.pause_job(id,expected,self.clock.now_utc()) }
    pub fn delete_job(&self, id:&str, expected:&str) -> Result<()> { self.store.delete_job(id,expected) }
    pub fn preview_times(&self, schedule:&ScheduleSpec) -> Result<Vec<NextOccurrence>> { preview_next_runs(schedule,self.clock.now_utc(),5) }
    pub fn history(&self, id:Option<&str>) -> Result<Vec<SchedulerRunSummary>> { Ok(self.store.history(id)?.into_iter().map(Into::into).collect()) }
    pub fn cancel_run(&self, id:&str) -> Result<SchedulerRunSummary> { Ok(self.store.request_cancel(id,self.clock.now_utc())?.into()) }
    pub fn reconcile_run(&self, id:&str, expected_job_revision:&str, evidence_digest:&str) -> Result<JobRecord> {
        self.store.acknowledge_run(id,expected_job_revision,evidence_digest,self.clock.now_utc())
    }
    pub fn start(self:&Arc<Self>) -> Result<SchedulerServiceHandle> {
        ensure!(!self.started.swap(true,Ordering::SeqCst),"SCHEDULE_SERVICE_ALREADY_STARTED");
        if let Err(error)=self.store.recover(self.clock.now_utc()) { self.started.store(false,Ordering::SeqCst); return Err(error); }
        Ok(self.runner.clone().start())
    }
    /// Returns a claimed journal projection while the owned Runner continues in the backend.
    /// The same request_id remains consumed even after journal retention removes the old row.
    pub async fn run_now(&self, id:&str, revision:&str, request_id:&str) -> Result<Option<SchedulerRunSummary>> {
        let Some(claim)=self.store.claim_manual(id,revision,request_id,&self.runner.owner,self.clock.now_utc())? else {return Ok(None)};
        let summary=claim.run.clone().into(); self.runner.clone().spawn_claimed(claim)?; Ok(Some(summary))
    }
    pub async fn prepare_approval(&self, id:&str, revision:&str, principal:&str) -> Result<SchedulerApprovalPreview> {
        validate_principal(principal)?;
        let state=self.store.snapshot()?;
        let record=state.jobs.get(id).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING"))?;
        ensure!(record.job.revision==revision && record.blocked_run_id.is_none(),"SCHEDULE_CONFLICT: 任务已变化或存在未核对运行");
        validate_job(&record.job)?;
        let job=record.job.clone(); let plan_id=uuid::Uuid::new_v4().to_string();
        let mut scopes=vec![]; let mut actions=vec![];
        for action in &job.actions {
            let template=self.store.load_template(&action.template_ref)?;
            ensure!(template.kind==action.kind,"SCHEDULE_TEMPLATE_KIND");
            let context=ActionExecutionContext::for_review(&self.runner.owner,&plan_id,&action.id,job.timeout_ms);
            let scope=self.resolve_owned(template.clone(),context).await?;
            ensure_scope_matches_template(&scope,&template)?;
            let mut details=vec![format!("连接资源：{}",scope.connection_ids.join(", ")), format!("端点 SHA-256：{}",scope.endpoint_fingerprint),format!("模板版本：{}",template.revision),format!("真实计划 SHA-256：{}",scope.permission_revision)];
            match &template.configuration { TemplateConfiguration::Sql {sql}=>details.push(sql.clone()),TemplateConfiguration::Tool {options_json}=>details.push(options_json.clone()) }
            let mut destructive=vec![];
            if let Some(limit)=&scope.destructive_limit {
                destructive.push(format!("计划版本：{}",limit.plan_revision));
                if let Some(max)=&limit.max_delete_rows {destructive.push(format!("最多删除 {} 行",max));}
                if limit.allow_overwrite {destructive.push(if template.kind == ActionKind::Sql {"允许下方精确 SQL 更新或删除现有数据；行数和函数/触发器副作用没有可证明的上限，仅批准此模板版本".into()} else {"允许此计划声明的数据写入/冲突覆盖；触发器副作用需要核对".into()});}
                if limit.allow_drop {destructive.push("允许下方精确 SQL 中声明的结构破坏及函数副作用；未确认风险不能启用".into());}
            }
            actions.push(SchedulerApprovalAction {action_id:action.id.clone(),template_name:template.name.clone(),connection_label:scope.connection_id.clone(),namespace:scope.namespace.clone(),read_only:scope.read_only,details,destructive});
            scopes.push((action.id.clone(),scope));
        }
        ensure!(self.current_job(id)?==job,"SCHEDULE_CONFLICT: 核对期间任务变化，请重新预览");
        let now=self.clock.now_utc();
        let mut preview=SchedulerApprovalPreview {plan_id:plan_id.clone(),review_digest:String::new(),job_id:id.into(),job_revision:revision.into(),expires_at_utc:now+Duration::milliseconds(REVIEW_TTL_MS as i64),actions};
        preview.review_digest=digest(&(&preview.plan_id,&job,&scopes,preview.expires_at_utc,&preview.actions))?;
        let mono=self.clock.monotonic_millis(); let mut reviews=self.reviews.lock().map_err(|_|anyhow!("SCHEDULE_REVIEW_UNAVAILABLE"))?;
        reviews.retain(|_,p|p.preview.expires_at_utc>now && mono.saturating_sub(p.created_mono)<REVIEW_TTL_MS);
        ensure!(reviews.len()<MAX_REVIEWS,"SCHEDULE_REVIEW_QUOTA: 请稍后重新预览");
        reviews.insert(plan_id,PreparedApproval {principal:principal.into(),preview:preview.clone(),created_mono:mono,job,scopes}); Ok(preview)
    }
    /// IPC supplies only plan ID and digest. Principal is the actual invoking window, never a client field.
    pub async fn approve_job(&self, plan_id:&str, review_digest:&str, principal:&str) -> Result<JobRecord> {
        validate_principal(principal)?; canonical_uuid(plan_id)?; ensure!(is_digest(review_digest),"SCHEDULE_REVIEW_INVALID");
        let prepared={
            let mut reviews=self.reviews.lock().map_err(|_|anyhow!("SCHEDULE_REVIEW_UNAVAILABLE"))?;
            let found=reviews.get(plan_id).ok_or_else(||anyhow!("SCHEDULE_REVIEW_EXPIRED: 请重新预览"))?;
            ensure!(found.principal==principal && found.preview.review_digest==review_digest,"SCHEDULE_REVIEW_FORBIDDEN");
            // Consume before any await: duplicate/failed approval requires a fresh review.
            reviews.remove(plan_id).ok_or_else(||anyhow!("SCHEDULE_REVIEW_EXPIRED"))?
        };
        ensure!(prepared.preview.expires_at_utc>self.clock.now_utc() && self.clock.monotonic_millis().saturating_sub(prepared.created_mono)<REVIEW_TTL_MS,"SCHEDULE_REVIEW_EXPIRED");
        ensure!(self.current_job(&prepared.job.id)?==prepared.job,"SCHEDULE_CONFLICT: 批准前任务变化");
        let mut current_scopes=vec![];
        for action in &prepared.job.actions {
            let template=self.store.load_template(&action.template_ref)?;
            let context=ActionExecutionContext::for_review(&self.runner.owner,plan_id,&action.id,prepared.job.timeout_ms);
            let scope=self.resolve_owned(template.clone(),context).await?; ensure_scope_matches_template(&scope,&template)?;
            ensure!(prepared.scopes.iter().find(|(id,_)|id==&action.id).is_some_and(|(_,old)|*old==scope),"SCHEDULE_SCOPE_CHANGED: 文件、端点或真实计划变化，请重新预览");
            current_scopes.push((action.id.clone(),scope));
        }
        ensure!(prepared.preview.expires_at_utc>self.clock.now_utc() && self.clock.monotonic_millis().saturating_sub(prepared.created_mono)<REVIEW_TTL_MS,"SCHEDULE_REVIEW_EXPIRED");
        self.store.approve_job(&prepared.job.id,&prepared.job.revision,current_scopes,self.clock.now_utc())
    }
    fn current_job(&self,id:&str)->Result<ScheduledJobV2> { self.store.snapshot()?.jobs.get(id).map(|r|r.job.clone()).ok_or_else(||anyhow!("SCHEDULE_JOB_MISSING")) }
    async fn resolve_owned(&self, template:ActionTemplate, context:ActionExecutionContext)->Result<ActionScope> {
        let executor=self.executor.clone(); let mut task=tokio::spawn(async move {executor.resolve_scope(&template,&context).await});
        match tokio::time::timeout(StdDuration::from_secs(30),&mut task).await {
            Ok(Ok(result))=>result,
            Ok(Err(_))=>Err(anyhow!("SCHEDULE_PREFLIGHT_UNAVAILABLE")),
            Err(_)=>{tokio::spawn(async move {let _=task.await;}); Err(anyhow!("SCHEDULE_PREFLIGHT_TIMEOUT: 尚未批准任何执行；重新预览前请核对连接状态"))}
        }
    }
}
fn validate_principal(principal:&str)->Result<()> { ensure!(!principal.trim().is_empty() && principal.len()<=256 && !principal.chars().any(char::is_control),"SCHEDULE_PRINCIPAL_INVALID"); Ok(()) }
pub(crate) fn ensure_scope_matches_template(scope:&ActionScope,template:&ActionTemplate)->Result<()> {
    scope.validate()?;
    ensure!(scope.template_ref==template.id && scope.template_revision==template.revision && scope.template_digest==digest(template)? && scope.connection_id==template.connection_id && scope.namespace==template.namespace && scope.read_only==template.read_only,"SCHEDULE_SCOPE_INVALID: 真实范围与模板内容不匹配"); Ok(())
}
