//! 稳定的 v2 调度契约；模板配置单独加密，运行历史不保存 SQL/路径/秘密/原始错误。
use anyhow::{bail, ensure, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEDULER_VERSION: u32 = 2;
pub const MAX_JOBS: usize = 512;
pub const MAX_ACTIONS: usize = 64;
pub const MAX_TEMPLATES: usize = 1024;
pub const MAX_HISTORY: usize = 200;
pub const MAX_JOURNAL_EVENTS: usize = 256;
pub const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TEMPLATE_BYTES: usize = 16 * 1024 * 1024;
pub const LEASE_SECONDS: i64 = 30;
pub const TICK_LATE_TOLERANCE_MS: i64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleSpec {
    Manual,
    Interval { seconds: u64, anchor_utc: DateTime<Utc> },
    Calendar { cron: String, timezone: String, dst_gap: DstGap, dst_fold: DstFold },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DstGap { Skip }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DstFold { First }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode { AppOnline, OfflineOptIn }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerMode { Gui, Headless }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicy { Skip, LatestOnce }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy { Skip, QueueOne }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind { Sql, Backup, Export, Import, DataSync, StructureSync }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScheduledAction {
    pub id: String,
    pub kind: ActionKind,
    pub depends_on: Vec<String>,
    pub template_ref: String,
    /// 由受信后端审批写入，save_job 不接受客户端注入的新批准。
    #[serde(default)] pub approved_scope_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScheduledJobV2 {
    pub version: u32,
    pub id: String,
    pub revision: String,
    pub name: String,
    pub enabled: bool,
    pub execution_mode: ExecutionMode,
    pub schedule: ScheduleSpec,
    pub misfire: MisfirePolicy,
    pub overlap: OverlapPolicy,
    pub timeout_ms: u64,
    pub actions: Vec<ScheduledAction>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateConfiguration {
    Sql { sql: String },
    Tool { options_json: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionTemplate {
    pub id: String,
    pub revision: String,
    pub name: String,
    pub kind: ActionKind,
    pub connection_id: String,
    pub namespace: String,
    pub read_only: bool,
    pub configuration: TemplateConfiguration,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionTemplateSummary {
    pub id: String, pub revision: String, pub name: String, pub kind: ActionKind,
    pub connection_id: String, pub namespace: String, pub read_only: bool,
}
impl From<ActionTemplate> for ActionTemplateSummary {
    fn from(t: ActionTemplate) -> Self { Self { id:t.id, revision:t.revision, name:t.name, kind:t.kind, connection_id:t.connection_id, namespace:t.namespace, read_only:t.read_only } }
}
/// 当前端点与真实工具预览产生的授权边界；必须由主进程 resolver 计算。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionScope {
    pub template_ref: String,
    pub template_revision: String,
    pub template_digest: String,
    pub connection_id: String,
    /// 包括数据同步的源/目标等全部连接；单连接互斥按整个集合计算。
    pub connection_ids: Vec<String>,
    pub endpoint_fingerprint: String,
    pub namespace: String,
    pub read_only: bool,
    /// 当前真实工具计划的审批摘要，必须覆盖文件摘要/映射/覆盖策略/TLS profile。
    pub permission_revision: String,
    /// 对数据删除/覆盖/结构破坏步骤的计划边界；None 不批准任何破坏性变化。
    pub destructive_limit: Option<DestructiveLimit>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DestructiveLimit {
    pub plan_revision: String,
    pub max_delete_rows: Option<String>,
    pub allow_overwrite: bool,
    pub allow_drop: bool,
}
impl ActionScope {
    pub fn revision(&self) -> Result<String> { self.validate()?; digest(self) }
    pub fn validate(&self) -> Result<()> {
        canonical_uuid(&self.template_ref)?; canonical_uuid(&self.template_revision)?;
        ensure!(is_digest(&self.template_digest) && is_digest(&self.endpoint_fingerprint) && is_digest(&self.permission_revision), "SCHEDULE_SCOPE_INVALID: 模板/端点/权限摘要必须是 SHA-256");
        nonempty_text(&self.connection_id, 128, "连接身份")?;
        let ids: BTreeSet<_> = self.connection_ids.iter().collect();
        ensure!(!ids.is_empty() && ids.len() == self.connection_ids.len() && ids.len() <= 8 && ids.contains(&self.connection_id), "SCHEDULE_SCOPE_INVALID: 必须声明唯一且完整的连接资源集合");
        for id in &self.connection_ids { nonempty_text(id, 128, "连接身份")?; }
        bounded_text(&self.namespace, 1024, "命名空间")?;
        if let Some(limit) = &self.destructive_limit {
            ensure!(is_digest(&limit.plan_revision), "SCHEDULE_SCOPE_INVALID: 破坏性计划必须绑定真实 revision");
            if let Some(n) = &limit.max_delete_rows { ensure!(!n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()) && n.parse::<u64>().is_ok(), "SCHEDULE_SCOPE_INVALID: 删除阈值必须是精确非负整数字符串"); }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeApproval {
    pub action_id: String,
    pub scope: ActionScope,
    pub scope_revision: String,
    pub approved_at_utc: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyJobReference {
    pub legacy_id: String,
    pub last_run_local_text: Option<String>,
    pub last_status: Option<String>,
    pub source_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuedOccurrence {
    pub schedule_revision: String,
    pub scheduled_at_utc: DateTime<Utc>,
    pub request_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobRecord {
    pub job: ScheduledJobV2,
    pub schedule_revision: String,
    pub next_run_utc: Option<DateTime<Utc>>,
    pub queued: Option<QueuedOccurrence>,
    pub blocked_run_id: Option<String>,
    pub paused_reason: Option<String>,
    pub approvals: BTreeMap<String, ScopeApproval>,
    pub legacy: Option<LegacyJobReference>,
    pub last_occurrence_utc: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerOwner {
    pub instance_id: String,
    pub process_id: u32,
    pub mode: RunnerMode,
}
impl SchedulerOwner {
    pub fn new(mode: RunnerMode) -> Self { Self { instance_id: uuid::Uuid::new_v4().to_string(), process_id: std::process::id(), mode } }
    pub fn validate(&self) -> Result<()> { canonical_uuid(&self.instance_id)?; ensure!(self.process_id > 0, "SCHEDULE_OWNER_INVALID: 无效进程身份"); Ok(()) }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunLease {
    pub owner: SchedulerOwner,
    pub token: String,
    pub expires_at_utc: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus { Claimed, Running, Succeeded, Failed, Cancelled, TimedOut, Partial, Unknown, AwaitingConfirmation }
impl RunStatus { pub fn terminal(self) -> bool { !matches!(self, Self::Claimed | Self::Running) } }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus { Pending, Started, Succeeded, Failed, Cancelled, TimedOut, Partial, Unknown, AwaitingConfirmation, Blocked }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionEffects { None, Committed, Possible, Unknown }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionReceipt {
    pub status: ActionStatus,
    pub verified: bool,
    pub effects: ActionEffects,
    pub resources_pending: bool,
    pub execution_id: Option<String>,
    pub affected_rows: Option<String>,
    /// 只允许 ASCII 错误码，绝不把原始数据库/文件错误放进 journal。
    pub error_code: Option<String>,
}
impl ActionReceipt {
    pub fn failed(code: &str) -> Self { Self { status: ActionStatus::Failed, verified: true, effects: ActionEffects::None, resources_pending: false, execution_id: None, affected_rows: None, error_code: Some(safe_code(code)) } }
    pub fn validate(&self) -> Result<()> {
        ensure!(!matches!(self.status, ActionStatus::Pending | ActionStatus::Started | ActionStatus::Blocked), "SCHEDULE_RECEIPT_INVALID: 动作没有真实终态");
        if let Some(id) = &self.execution_id { nonempty_text(id,128,"执行身份")?; }
        if let Some(rows) = &self.affected_rows { ensure!(!rows.is_empty() && rows.len() <= 20 && rows.bytes().all(|b| b.is_ascii_digit()) && rows.parse::<u64>().is_ok(), "SCHEDULE_RECEIPT_INVALID: 受影响行数不是精确非负整数"); }
        if let Some(code) = &self.error_code { ensure!(safe_code(code) == *code, "SCHEDULE_RECEIPT_INVALID: journal 只接受脱敏错误码"); }
        if !self.verified || matches!(self.effects, ActionEffects::Possible | ActionEffects::Unknown) { ensure!(self.status == ActionStatus::Unknown || self.status == ActionStatus::Partial, "SCHEDULE_RECEIPT_INVALID: 未确认的效果必须保持 Unknown/Partial"); }
        if self.resources_pending { ensure!(self.status != ActionStatus::Succeeded, "SCHEDULE_RECEIPT_INVALID: 在途资源不能成功"); }
        if self.status == ActionStatus::Succeeded { ensure!(self.verified && !self.resources_pending && matches!(self.effects,ActionEffects::None|ActionEffects::Committed), "SCHEDULE_RECEIPT_INVALID: 未核验/资源未排空的动作不能成功"); }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionJournal {
    pub action_id: String,
    pub status: ActionStatus,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub receipt: Option<ActionReceipt>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEvent {
    pub at_utc: DateTime<Utc>,
    pub kind: String,
    pub action_id: Option<String>,
    pub code: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduledRun {
    pub id: String,
    pub job_id: String,
    pub job_revision: String,
    pub schedule_revision: String,
    pub occurrence_key: String,
    pub scheduled_at_utc: DateTime<Utc>,
    pub claimed_at_utc: DateTime<Utc>,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub lease: RunLease,
    pub cancel_requested: bool,
    pub resources_pending: bool,
    pub connection_ids: Vec<String>,
    pub actions: Vec<ActionJournal>,
    pub journal: Vec<JournalEvent>,
    /// 冻结的真实任务版本和批准；配置改变不会就地修改正在执行的步骤。
    pub job_snapshot: ScheduledJobV2,
    pub approvals: BTreeMap<String, ScopeApproval>,
}
#[derive(Debug, Clone)]
pub struct ClaimedRun { pub run: ScheduledRun }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerState {
    pub version: u32,
    pub revision: String,
    pub jobs: BTreeMap<String, JobRecord>,
    pub runs: Vec<ScheduledRun>,
    #[serde(default)] pub consumed_manual_requests: BTreeSet<String>,
    pub imported_legacy_digests: BTreeSet<String>,
    pub offline_plans: BTreeMap<String, super::offline::OfflineInstallPlan>,
}
impl Default for SchedulerState { fn default() -> Self { Self { version:SCHEDULER_VERSION, revision:uuid::Uuid::new_v4().to_string(), jobs:BTreeMap::new(), runs:vec![], consumed_manual_requests:BTreeSet::new(), imported_legacy_digests:BTreeSet::new(), offline_plans:BTreeMap::new() } } }

pub fn canonical_uuid(id: &str) -> Result<()> { let parsed = uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("SCHEDULE_ID_INVALID: 身份必须是规范 UUID"))?; ensure!(parsed.to_string() == id && parsed.get_variant() == uuid::Variant::RFC4122 && parsed.get_version().is_some(), "SCHEDULE_ID_INVALID: 身份必须是规范 UUID"); Ok(()) }
pub fn is_digest(s: &str) -> bool { s.len()==64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
pub fn digest<T: Serialize>(value: &T) -> Result<String> { Ok(format!("{:x}",Sha256::digest(serde_json::to_vec(value)?))) }
pub fn safe_code(code: &str) -> String { if !code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b==b'_') { code.into() } else { "SCHEDULE_ACTION_FAILED".into() } }
fn bounded_text(value: &str, max: usize, name: &str) -> Result<()> { ensure!(value.len() <= max && !value.chars().any(char::is_control),"SCHEDULE_INVALID: {}过长或含控制字符",name); Ok(()) }
fn nonempty_text(value: &str, max: usize, name: &str) -> Result<()> { bounded_text(value,max,name)?; ensure!(!value.trim().is_empty(),"SCHEDULE_INVALID: {}不能为空",name); Ok(()) }

pub fn validate_template(template: &ActionTemplate) -> Result<()> {
    canonical_uuid(&template.id)?; if !template.revision.is_empty() { canonical_uuid(&template.revision)?; }
    nonempty_text(&template.name, 256,"模板名")?; nonempty_text(&template.connection_id,128,"连接身份")?; bounded_text(&template.namespace,1024,"命名空间")?;
    match (&template.kind,&template.configuration) {
        (ActionKind::Sql,TemplateConfiguration::Sql { sql }) => ensure!(!sql.trim().is_empty() && sql.len() <= MAX_TEMPLATE_BYTES/2,"SCHEDULE_TEMPLATE_INVALID: SQL 模板不能为空或超长"),
        (ActionKind::Sql,_) | (_,TemplateConfiguration::Sql {..}) => bail!("SCHEDULE_TEMPLATE_INVALID: 动作类别与配置不一致"),
        (_,TemplateConfiguration::Tool { options_json }) => {
            ensure!(options_json.len() <= MAX_TEMPLATE_BYTES/2,"SCHEDULE_TEMPLATE_INVALID: 工具配置超长");
            let _: Box<serde_json::value::RawValue> = serde_json::from_str(options_json).map_err(|_| anyhow::anyhow!("SCHEDULE_TEMPLATE_INVALID: 工具配置 JSON 无效"))?;
            let value: serde_json::Value = serde_json::from_str(options_json)?;
            ensure!(value.is_object(),"SCHEDULE_TEMPLATE_INVALID: 工具配置必须是对象");
            fn secrets(value: &serde_json::Value, depth: usize) -> bool {
                if depth > 48 { return true; }
                match value {
                    serde_json::Value::Object(fields) => fields.iter().any(|(k,v)| { let k=k.to_ascii_lowercase().replace(['-','_'],""); matches!(k.as_str(),"password"|"pwd"|"token"|"privatekey"|"secret"|"accesskey"|"connectionstring"|"sshpassword"|"keypassphrase") || secrets(v,depth+1) }),
                    serde_json::Value::Array(values) => values.iter().any(|v| secrets(v,depth+1)), _ => false,
                }
            }
            ensure!(!secrets(&value,0),"SCHEDULE_TEMPLATE_SECRET: 工具模板只允许本机稳定引用，不能嵌入口令或令牌");
        }
    }
    Ok(())
}
pub fn validate_job(job: &ScheduledJobV2) -> Result<Vec<String>> {
    ensure!(job.version == SCHEDULER_VERSION,"SCHEDULE_VERSION_UNSUPPORTED: 未支持的调度版本");
    canonical_uuid(&job.id)?; if !job.revision.is_empty() { canonical_uuid(&job.revision)?; }
    nonempty_text(&job.name,256,"任务名")?;
    ensure!((1..=86_400_000).contains(&job.timeout_ms),"SCHEDULE_TIMEOUT_INVALID: 总执行预算必须是 1–86400000 毫秒");
    ensure!(!job.actions.is_empty() && job.actions.len() <= MAX_ACTIONS,"SCHEDULE_DAG_INVALID: 任务链须有 1–64 个动作");
    super::clock::validate_schedule(&job.schedule)?;
    let mut ids = BTreeSet::new();
    for action in &job.actions { canonical_uuid(&action.id)?; canonical_uuid(&action.template_ref)?; ensure!(ids.insert(action.id.clone()),"SCHEDULE_DAG_INVALID: 重复动作身份"); ensure!(action.depends_on.len() <= MAX_ACTIONS && (action.approved_scope_revision.is_empty() || is_digest(&action.approved_scope_revision)),"SCHEDULE_DAG_INVALID: 依赖/批准摘要无效"); }
    let mut pending: BTreeMap<String,BTreeSet<String>> = BTreeMap::new();
    for action in &job.actions {
        let deps: BTreeSet<_> = action.depends_on.iter().cloned().collect();
        ensure!(deps.len() == action.depends_on.len() && !deps.contains(&action.id) && deps.iter().all(|id| ids.contains(id)),"SCHEDULE_DAG_INVALID: 自依赖、重复依赖或不存在的动作"); pending.insert(action.id.clone(),deps);
    }
    let mut order=Vec::new();
    while !pending.is_empty() {
        let ready: Vec<_> = job.actions.iter().filter(|a| pending.get(&a.id).is_some_and(BTreeSet::is_empty)).map(|a| a.id.clone()).collect();
        ensure!(!ready.is_empty(),"SCHEDULE_DAG_CYCLE: 任务链不能包含环");
        for id in ready { pending.remove(&id); for deps in pending.values_mut() { deps.remove(&id); } order.push(id); }
    }
    Ok(order)
}
pub fn schedule_revision(job: &ScheduledJobV2) -> Result<String> {
    let mut safe = job.clone(); safe.name.clear(); safe.revision.clear(); for a in &mut safe.actions { a.approved_scope_revision.clear(); }
    digest(&safe)
}
