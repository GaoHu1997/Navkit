//! PL19 P0: native SQLite/main user-trigger DROP only.
//! The parent supplies actual Binding/activity/ordinary guards, registers C1 once,
//! checks validate_request against the signed marker and publishes the sole Entry.finish.
//! No CREATE/RENAME/ENABLE/DISABLE fallback and no client SQL/DDL/metadata proof.
use std::{collections::BTreeMap, sync::{Arc, OnceLock}, time::{Duration, Instant}};
use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::{SqliteConnectOptions, SqliteConnection}, SqlitePool};
use crate::{
    db::Db, error::{AppError, CmdResult}, execution::{self, Binding, Entry},
    manual_transaction::OrdinaryLease, models::{DbKind, MultiResult},
    object_catalog::{DatabaseObjectId, revision},
    structure_execution::metadata::{self, PhysicalDatabaseIdentity},
};

const TTL: Duration = Duration::from_secs(120);
const BASELINES: usize = 64;
const PLANS: usize = 64;
const DIRECTORY_ROWS: usize = 4096;
const SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const DRAFT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectMutationOperation { Drop, Rename, Enable, Disable }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMutationBaseline {
    pub baseline_id: String,
    pub object: DatabaseObjectId,
    pub source_fingerprint: String,
    pub base_revision: String,
    /// Generation-qualified SQLite schema identity, not a permanent vendor OID.
    pub backend_object_id: String,
    pub metadata_state: String,
    pub expires_in_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMutationDraft {
    pub baseline_id: String,
    pub object: DatabaseObjectId,
    pub source_fingerprint: String,
    pub base_revision: String,
    pub operation: ObjectMutationOperation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMutationPreview {
    pub plan_id: String,
    pub baseline_id: String,
    pub object: DatabaseObjectId,
    pub source_fingerprint: String,
    pub base_revision: String,
    pub review_digest: String,
    pub request_sql: String,
    pub expires_in_ms: u64,
    pub confirmation_required: bool,
    pub transaction: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMutationWork {
    pub plan_id: String,
    pub review_digest: String,
    pub draft: ObjectMutationDraft,
    pub confirm_drop: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMutationReceipt {
    pub plan_id: String,
    pub baseline_id: String,
    pub object: DatabaseObjectId,
    pub backend_object_id: String,
    pub source_fingerprint: String,
    pub before_revision: String,
    pub after_revision: String,
    pub status: String,
    pub verified: bool,
    pub transaction: String,
    pub mismatches: Vec<String>,
}
pub struct BoundResult {
    pub result: CmdResult<MultiResult>,
    pub confirmed: bool,
    pub receipt: Option<ObjectMutationReceipt>,
    /// Only a proved no-send or same-connection rollback establishes no_effect.
    pub no_effect: bool,
    pub transaction: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SchemaEntry {
    rowid: i64, kind: String, name: String, parent_name: String,
    rootpage: i64, sql: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Snapshot {
    physical: PhysicalDatabaseIdentity,
    schema_cookie: i64,
    target: SchemaEntry,
    parent: SchemaEntry,
    /// Complete main directory, including internal/auto indexes and all siblings.
    directory: Vec<SchemaEntry>,
}
#[derive(Clone)]
struct Opened {
    public: ObjectMutationBaseline,
    binding: Binding,
    snapshot: Arc<Snapshot>,
    // Retain actual options to prevent private :memory: pointer-identity ABA.
    _options: Arc<SqliteConnectOptions>,
    born: Instant,
    bytes: usize,
}
#[derive(Clone)]
struct Plan {
    preview: ObjectMutationPreview,
    draft: ObjectMutationDraft,
    opened: Opened,
    bytes: usize,
}
#[derive(Default)]
struct Cache { baselines: BTreeMap<String, Opened>, plans: BTreeMap<String, Plan> }
#[derive(Default)]
pub struct Registry { cache: parking_lot::Mutex<Cache> }
static GLOBAL: OnceLock<Registry> = OnceLock::new();
pub fn global() -> &'static Registry { GLOBAL.get_or_init(Registry::default) }

fn error(code: &str, message: impl Into<String>) -> AppError { AppError::coded(code, message) }
fn unknown(message: impl Into<String>) -> AppError { error("EXECUTION_EFFECTS_UNKNOWN", message) }
fn is_unknown(result: &CmdResult<MultiResult>) -> bool {
    result.as_ref().err().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN")
}
fn remaining(born: Instant) -> u64 {
    TTL.saturating_sub(born.elapsed()).as_millis().min(u64::MAX as u128) as u64
}
fn canonical_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn identity(object: &DatabaseObjectId, binding: &Binding) -> CmdResult<()> {
    if binding.principal.trim().is_empty() || binding.principal.contains('\0')
        || binding.owner.trim().is_empty() || binding.owner.len() > 128 || binding.owner.contains('\0')
        || binding.connection.is_empty() || binding.connection.contains('\0') || binding.namespace != "main"
        || binding.fingerprint.len() != 64
        || !binding.fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(error("OBJECT_MUTATION_BINDING_INVALID", "缺少真实窗口/页面/端点/main命名空间证明"));
    }
    let parent = object.parent.as_ref().ok_or_else(|| error("OBJECT_PARENT_REQUIRED", "触发器必须携带完整父表身份"))?;
    if object.connection_id != binding.connection || object.schema != "main" || object.catalog.is_some()
        || object.kind != "trigger" || object.identity_arguments.is_some()
        || parent.schema != "main" || parent.kind != "table" {
        return Err(error("OBJECT_MUTATION_SCOPE_UNSUPPORTED", "仅原生SQLite/main非内部用户触发器DROP，完整父表/连接身份不得改变"));
    }
    for name in [&object.name, &parent.name] {
        if name.is_empty() || name.len() > 1024 || name.contains('\0')
            || name.to_ascii_lowercase().starts_with("sqlite_") {
            return Err(error("OBJECT_MUTATION_SYSTEM_OR_INVALID", "内部对象或无效对象名称禁止图形化DROP"));
        }
    }
    Ok(())
}
fn adapter(db: &Db, kind: &DbKind, read_only: bool) -> CmdResult<()> {
    if read_only { return Err(error("READ_ONLY_VIOLATION", "只读连接禁止对象DROP，没有发送DDL")); }
    if !kind.is_sqlite() || !matches!(db, Db::Sqlite(_)) {
        return Err(error("OBJECT_MUTATION_ADAPTER_UNSUPPORTED", "仅原生SQLite触发器DROP已接入，其它厂商/对象操作未实现"));
    }
    Ok(())
}
fn draft_identity(draft: &ObjectMutationDraft, binding: &Binding) -> CmdResult<()> {
    identity(&draft.object, binding)?;
    if draft.operation != ObjectMutationOperation::Drop {
        return Err(error("OBJECT_MUTATION_OPERATION_UNSUPPORTED", "仅DROP已实现；不以重建触发器伪造重命名或启停"));
    }
    if !canonical_id(&draft.baseline_id) || draft.source_fingerprint != binding.fingerprint
        || draft.base_revision.len() != 64 || serde_json::to_vec(draft)?.len() > DRAFT_BYTES {
        return Err(error("OBJECT_MUTATION_BASELINE_INVALID", "必须使用原打开基线和端点，不能补齐/替换旧CAS"));
    }
    Ok(())
}
impl Cache {
    fn prune(&mut self) {
        self.baselines.retain(|_, value| value.born.elapsed() < TTL);
        let baselines = &self.baselines;
        self.plans.retain(|_, value| value.opened.born.elapsed() < TTL
            && baselines.contains_key(&value.draft.baseline_id));
    }
    fn bytes(&self) -> usize {
        self.baselines.values().map(|v| v.bytes).sum::<usize>()
            .saturating_add(self.plans.values().map(|v| v.bytes).sum::<usize>())
    }
}
impl Registry {
    fn opened(&self, draft: &ObjectMutationDraft, binding: &Binding) -> CmdResult<Opened> {
        draft_identity(draft, binding)?;
        let mut cache = self.cache.lock(); cache.prune();
        let opened = cache.baselines.get(&draft.baseline_id)
            .ok_or_else(|| error("OBJECT_MUTATION_BASELINE_EXPIRED", "原打开基线已消费或到期，必须明确重新读取对象"))?;
        if opened.binding != *binding || opened.public.object != draft.object
            || opened.public.source_fingerprint != draft.source_fingerprint
            || opened.public.base_revision != draft.base_revision {
            return Err(error("OBJECT_MUTATION_BASELINE_CHANGED", "窗口/页面/对象/打开时端点或CAS不一致，没有发送DDL"));
        }
        Ok(opened.clone())
    }
    fn insert_baseline(&self, binding: Binding, object: DatabaseObjectId,
        snapshot: Snapshot, options: Arc<SqliteConnectOptions>) -> CmdResult<ObjectMutationBaseline> {
        let base_revision = revision(&snapshot);
        let backend_object_id = format!("sqlite-schema-generation:{}", revision(&(
            &snapshot.physical, snapshot.schema_cookie, &snapshot.target, &snapshot.parent)));
        let public = ObjectMutationBaseline { baseline_id: uuid::Uuid::new_v4().to_string(),
            object, source_fingerprint: binding.fingerprint.clone(), base_revision,
            backend_object_id, metadata_state: "known".into(), expires_in_ms: TTL.as_millis() as u64 };
        let bytes = serde_json::to_vec(&(&snapshot, &binding, &public))?.len();
        if bytes > SNAPSHOT_BYTES { return Err(error("OBJECT_MUTATION_METADATA_LIMIT", "完整对象目录超过8MiB预算，拒绝不完整证明")); }
        let mut cache = self.cache.lock(); cache.prune();
        if cache.baselines.len() >= BASELINES || cache.bytes().saturating_add(bytes) > CACHE_BYTES {
            return Err(error("OBJECT_MUTATION_PLAN_LIMIT", "对象基线缓存已达64项/32MiB预算，请等待到期或关闭旧连接"));
        }
        cache.baselines.insert(public.baseline_id.clone(), Opened { public: public.clone(), binding,
            snapshot: Arc::new(snapshot), _options: options, born: Instant::now(), bytes });
        Ok(public)
    }
    fn plan(&self, work: &ObjectMutationWork, binding: &Binding, require_confirmation: bool) -> CmdResult<Plan> {
        draft_identity(&work.draft, binding)?;
        if !canonical_id(&work.plan_id) { return Err(error("OBJECT_MUTATION_PLAN_INVALID", "必须使用后端签发的不透明计划ID")); }
        if require_confirmation && !work.confirm_drop { return Err(error("OBJECT_MUTATION_CONFIRM_REQUIRED", "触发器DROP需要明确确认")); }
        let mut cache = self.cache.lock(); cache.prune();
        let plan = cache.plans.get(&work.plan_id)
            .ok_or_else(|| error("OBJECT_MUTATION_PLAN_EXPIRED", "DROP计划已消费或到期，禁止自动重放"))?;
        if plan.opened.binding != *binding || plan.draft != work.draft || plan.preview.review_digest != work.review_digest {
            return Err(error("OBJECT_MUTATION_PLAN_SCOPE_CHANGED", "原打开基线/操作/窗口/页面/端点或审核摘要不一致"));
        }
        Ok(plan.clone())
    }
    fn insert_plan(&self, draft: ObjectMutationDraft, binding: &Binding, observed: &Snapshot) -> CmdResult<ObjectMutationPreview> {
        let opened = self.opened(&draft, binding)?;
        if *opened.snapshot != *observed { return Err(error("OBJECT_MUTATION_BASELINE_STALE", "真实目录/物理库/父表/触发器已变化，不能用fresh数据替换原打开CAS")); }
        let mut cache = self.cache.lock(); cache.prune();
        if !cache.baselines.contains_key(&draft.baseline_id) { return Err(error("OBJECT_MUTATION_BASELINE_EXPIRED", "预览期间原打开基线已到期")); }
        if let Some(plan) = cache.plans.values().find(|p| p.draft.baseline_id == draft.baseline_id) {
            if plan.draft != draft || plan.opened.binding != *binding { return Err(error("OBJECT_MUTATION_PLAN_SCOPE_CHANGED", "同一基线已有不同工作计划")); }
            let mut public = plan.preview.clone(); public.expires_in_ms = remaining(opened.born); return Ok(public);
        }
        let plan_id = uuid::Uuid::new_v4().to_string();
        let review_digest = revision(&("navkit-object-drop-v1", &plan_id, &draft, binding, &opened.public.backend_object_id));
        let request_sql = format!("OBJECT_MUTATION_DROP {plan_id} {review_digest}");
        let preview = ObjectMutationPreview { plan_id: plan_id.clone(), baseline_id: draft.baseline_id.clone(),
            object: draft.object.clone(), source_fingerprint: draft.source_fingerprint.clone(),
            base_revision: draft.base_revision.clone(), review_digest, request_sql,
            expires_in_ms: remaining(opened.born), confirmation_required: true, transaction: "transactional".into() };
        let bytes = serde_json::to_vec(&(&draft, &preview))?.len();
        if cache.plans.len() >= PLANS || cache.bytes().saturating_add(bytes) > CACHE_BYTES {
            return Err(error("OBJECT_MUTATION_PLAN_LIMIT", "DROP计划缓存达到64项/32MiB预算"));
        }
        cache.plans.insert(plan_id, Plan { preview: preview.clone(), draft, opened, bytes });
        Ok(preview)
    }
    fn take(&self, work: &ObjectMutationWork, binding: &Binding, marker: &str) -> CmdResult<Plan> {
        self.validate_request(work, binding, marker)?;
        let mut cache = self.cache.lock(); cache.prune();
        let plan = cache.plans.get(&work.plan_id).ok_or_else(|| error("OBJECT_MUTATION_PLAN_EXPIRED", "并发执行已消费此计划"))?;
        // Recheck inside the consuming lock; invalid requests never consume another owner's plan.
        if plan.opened.binding != *binding || plan.draft != work.draft || plan.preview.request_sql != marker
            || plan.preview.review_digest != work.review_digest { return Err(error("OBJECT_MUTATION_PLAN_SCOPE_CHANGED", "一次性消费上下文变化")); }
        let plan = cache.plans.remove(&work.plan_id).expect("checked plan");
        cache.baselines.remove(&work.draft.baseline_id);
        Ok(plan)
    }
    pub fn validate_request(&self, work: &ObjectMutationWork, binding: &Binding, marker: &str) -> CmdResult<()> {
        let plan = self.plan(work, binding, true)?;
        if marker != plan.preview.request_sql { return Err(error("EXECUTION_PAYLOAD_MISMATCH", "已签发C1标识不是此原打开基线的DROP计划，没有发送DDL")); }
        Ok(())
    }
    pub fn request_sql(&self, work: &ObjectMutationWork, binding: &Binding) -> CmdResult<String> {
        Ok(self.plan(work, binding, false)?.preview.request_sql)
    }
    pub fn clear_connection(&self, connection_id: &str) {
        let mut cache = self.cache.lock();
        cache.plans.retain(|_, p| p.opened.binding.connection != connection_id);
        cache.baselines.retain(|_, p| p.binding.connection != connection_id);
    }
}

async fn autocommit(conn: &mut SqliteConnection) -> Result<bool> {
    let mut handle = conn.lock_handle().await?;
    // SAFETY: SQLx owns and locks the actual living SQLite handle.
    Ok(unsafe { libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr()) != 0 })
}
async fn writable(conn: &mut SqliteConnection) -> Result<()> {
    let query_only: i64 = crate::sql_history::query_scalar("PRAGMA query_only").fetch_one(&mut *conn).await?;
    ensure!(query_only == 0, "OBJECT_MUTATION_QUERY_ONLY: 实际连接禁止DDL");
    let mut handle = conn.lock_handle().await?;
    // SAFETY: static NUL-terminated database name, under the worker handle lock.
    let read_only = unsafe { libsqlite3_sys::sqlite3_db_readonly(handle.as_raw_handle().as_ptr(), b"main\0".as_ptr().cast()) };
    ensure!(read_only == 0, "OBJECT_MUTATION_DATABASE_READ_ONLY: main未证明可写");
    Ok(())
}
async fn directory(conn: &mut SqliteConnection, pool: &SqlitePool,
    options: &Arc<SqliteConnectOptions>) -> Result<(PhysicalDatabaseIdentity, i64, Vec<SchemaEntry>)> {
    let physical = metadata::physical(conn, pool, options).await?;
    let before: i64 = crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    let rows: Vec<(i64, String, String, String, i64, Option<String>)> = crate::sql_history::query_as(
        "SELECT rowid,type,name,tbl_name,rootpage,sql FROM main.sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY,rowid LIMIT 4097")
        .fetch_all(&mut *conn).await?;
    ensure!(rows.len() <= DIRECTORY_ROWS, "OBJECT_MUTATION_METADATA_LIMIT: 完整目录超过4096项，拒绝截断证明");
    let entries: Vec<_> = rows.into_iter().map(|(rowid, kind, name, parent_name, rootpage, sql)|
        SchemaEntry { rowid, kind, name, parent_name, rootpage, sql }).collect();
    ensure!(serde_json::to_vec(&entries)?.len() <= SNAPSHOT_BYTES, "OBJECT_MUTATION_METADATA_LIMIT: 完整目录超过8MiB");
    let after: i64 = crate::sql_history::query_scalar("PRAGMA main.schema_version").fetch_one(&mut *conn).await?;
    ensure!(before == after, "OBJECT_MUTATION_METADATA_CHANGED: 读取期间schema cookie改变");
    Ok((physical, before, entries))
}
async fn snapshot(conn: &mut SqliteConnection, pool: &SqlitePool,
    options: &Arc<SqliteConnectOptions>, object: &DatabaseObjectId) -> Result<Snapshot> {
    let (physical, schema_cookie, directory) = directory(conn, pool, options).await?;
    let parent_name = &object.parent.as_ref().ok_or_else(|| anyhow!("OBJECT_PARENT_REQUIRED"))?.name;
    let targets: Vec<_> = directory.iter().filter(|r| r.kind == "trigger" && r.name == object.name && &r.parent_name == parent_name).collect();
    ensure!(targets.len() == 1 && targets[0].sql.as_ref().is_some_and(|s| !s.is_empty()), "OBJECT_MUTATION_TARGET_UNKNOWN: 精确触发器不存在/定义不可读");
    let parents: Vec<_> = directory.iter().filter(|r| r.kind == "table" && &r.name == parent_name).collect();
    ensure!(parents.len() == 1 && parents[0].rootpage > 0 && parents[0].sql.is_some(), "OBJECT_MUTATION_PARENT_UNKNOWN: 父表不是有真实DDL/rootpage的普通表");
    let table_type: Option<String> = crate::sql_history::query_scalar("SELECT type FROM pragma_table_list WHERE schema='main' AND name=? COLLATE BINARY")
        .bind(parent_name).fetch_optional(&mut *conn).await?;
    ensure!(table_type.as_deref() == Some("table"), "OBJECT_MUTATION_PARENT_UNSUPPORTED: virtual/shadow/view父对象未认证");
    Ok(Snapshot { physical, schema_cookie, target: targets[0].clone(), parent: parents[0].clone(), directory })
}

struct OwnedObservation {
    snapshot: Snapshot,
    options: Arc<SqliteConnectOptions>,
    _ordinary: OrdinaryLease,
    _activity: tokio::sync::OwnedRwLockReadGuard<()>,
}
/// Read transactions own both guards and connection even if the metadata IPC future disappears.
async fn read_owned(db: Arc<Db>, kind: DbKind, binding: Binding, object: DatabaseObjectId,
    read_only: bool, ordinary: OrdinaryLease, activity: tokio::sync::OwnedRwLockReadGuard<()>)
    -> CmdResult<OwnedObservation> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    adapter(db.as_ref(), &kind, read_only)?; identity(&object, &binding)?;
    crate::sql_history::spawn(async move {
        let pool = match db.as_ref() { Db::Sqlite(p) => p, _ => unreachable!() };
        let options = pool.connect_options();
        let mut conn = pool.acquire().await.map_err(AppError::from)?;
        match autocommit(&mut conn).await {
            Ok(true) => {},
            value => {
                conn.close_on_drop(); let close = conn.close().await;
                return Err(unknown(format!("元数据连接既有事务或状态未知，未接管；状态={value:?}；关闭={close:?}")));
            }
        }
        if let Err(e) = writable(&mut conn).await {
            conn.return_to_pool().await; return Err(error("OBJECT_MUTATION_READ_ONLY", e.to_string()));
        }
        if let Err(e) = crate::sql_history::query("BEGIN").execute(&mut *conn).await {
            conn.close_on_drop(); let _ = conn.close().await;
            return Err(unknown(format!("对象基线读事务BEGIN/清理未确认：{e}")));
        }
        let observed = snapshot(&mut conn, pool, &options, &object).await;
        let rollback = crate::sql_history::query("ROLLBACK").execute(&mut *conn).await;
        if rollback.is_err() || !matches!(autocommit(&mut conn).await, Ok(true)) {
            conn.close_on_drop(); let _ = conn.close().await;
            return Err(unknown("对象基线读事务ROLLBACK未确认，不发布基线"));
        }
        conn.return_to_pool().await;
        observed.map(|snapshot| OwnedObservation { snapshot, options, _ordinary: ordinary, _activity: activity }).map_err(AppError::from)
    }).await.map_err(|e| error("OBJECT_MUTATION_METADATA_TASK_FAILED", format!("对象元数据拥有任务未返回完整结果：{e}")))?

    }).await
}

/// Parent must recheck saved/runtime Binding before and after this call before publishing.
pub async fn baseline(db: Arc<Db>, kind: DbKind, binding: Binding, object: DatabaseObjectId,
    ordinary: OrdinaryLease, activity: tokio::sync::OwnedRwLockReadGuard<()>, read_only: bool)
    -> CmdResult<ObjectMutationBaseline> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let observed = read_owned(db, kind, binding.clone(), object.clone(), read_only, ordinary, activity).await?;
    // Retain both guards through cache insertion, not merely through SQLite readback.
    global().insert_baseline(binding, object, observed.snapshot, observed.options)

    }).await
}
/// Fresh read only validates the original opened CAS; never replaces it or renews its TTL.
pub async fn preflight(db: Arc<Db>, kind: DbKind, binding: Binding, draft: ObjectMutationDraft,
    ordinary: OrdinaryLease, activity: tokio::sync::OwnedRwLockReadGuard<()>, read_only: bool)
    -> CmdResult<ObjectMutationPreview> {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let _opened = global().opened(&draft, &binding)?;
    let observed = read_owned(db, kind, binding.clone(), draft.object.clone(), read_only, ordinary, activity).await?;
    global().insert_plan(draft, &binding, &observed.snapshot)

    }).await
}
pub fn validate_object(object: &DatabaseObjectId, binding: &Binding) -> CmdResult<()> { identity(object, binding) }
/// Validate the original opened CAS before any metadata IPC is dispatched.
pub fn validate_draft(draft: &ObjectMutationDraft, binding: &Binding) -> CmdResult<()> {
    global().opened(draft, binding).map(|_| ())
}
pub fn request_sql(work: &ObjectMutationWork, binding: &Binding) -> CmdResult<String> { global().request_sql(work, binding) }
pub fn validate_request(work: &ObjectMutationWork, binding: &Binding, marker: &str) -> CmdResult<()> { global().validate_request(work, binding, marker) }

#[derive(Clone)]
enum CommitMode { Normal, #[cfg(test)] LoseAcknowledgement, #[cfg(test)] HoldAfterCommit(Arc<CommitGate>) }
#[cfg(test)]
struct CommitGate { reached: tokio::sync::Semaphore, release: tokio::sync::Semaphore }
async fn commit(conn: &mut SqliteConnection, _mode: CommitMode) -> Result<()> {
    crate::sql_history::query("COMMIT").execute(&mut *conn).await?;
    #[cfg(test)] match _mode {
        CommitMode::LoseAcknowledgement => return Err(anyhow!("test fixture: real SQLite COMMIT succeeded, acknowledgement lost")),
        CommitMode::HoldAfterCommit(gate) => {
            gate.reached.add_permits(1);
            let permit = gate.release.acquire().await.expect("commit fixture gate remains open");
            permit.forget();
        }
        CommitMode::Normal => {},
    }
    Ok(())
}
struct ApplyResult {
    result: CmdResult<MultiResult>, confirmed: bool, receipt: Option<ObjectMutationReceipt>,
    no_effect: bool, transaction: String,
}
impl ApplyResult {
    fn no_send(e: AppError, stopped: bool) -> Self {
        Self { result: Err(e), confirmed: stopped, receipt: None, no_effect: true, transaction: "none".into() }
    }
    fn uncertain(e: AppError) -> Self {
        Self { result: Err(e), confirmed: false, receipt: None, no_effect: false, transaction: "unknown".into() }
    }
}
async fn rollback(conn: &mut SqliteConnection, entry: &Arc<Entry>, cause: AppError) -> ApplyResult {
    if execution::clear_sqlite(conn).await.is_err() || crate::sql_history::query("ROLLBACK").execute(&mut *conn).await.is_err()
        || !matches!(autocommit(conn).await, Ok(true)) {
        return ApplyResult::uncertain(unknown(format!("对象DROP回滚/控制清理未确认；原错误：{}", cause.message)));
    }
    let stopped = entry.stopping();
    ApplyResult { result: Err(error(if stopped { "OBJECT_MUTATION_STOPPED_ROLLED_BACK" } else { "OBJECT_MUTATION_ROLLED_BACK" },
        format!("{}: {}；同拥有连接ROLLBACK已确认，未提交DROP", cause.code, cause.message))),
        confirmed: stopped, receipt: None, no_effect: true, transaction: "rolled_back".into() }
}
async fn apply(conn: &mut SqliteConnection, pool: &SqlitePool, entry: &Arc<Entry>, plan: &Plan, mode: CommitMode) -> ApplyResult {
    if entry.stopping() { return ApplyResult::no_send(error("EXECUTION_CANCELLED_BEFORE_SEND", "发送前停止，没有发送DROP"), true); }
    if !matches!(autocommit(conn).await, Ok(true)) { return ApplyResult::uncertain(unknown("拥有连接已有事务，未回滚/接管未知事务")); }
    if let Err(e) = writable(conn).await { return ApplyResult::no_send(error("OBJECT_MUTATION_READ_ONLY", e.to_string()), false); }
    if let Err(e) = crate::sql_history::query("BEGIN IMMEDIATE").execute(&mut *conn).await {
        if !matches!(autocommit(conn).await, Ok(true)) { return ApplyResult::uncertain(unknown(format!("对象DROP BEGIN结果未知：{e}"))); }
        return ApplyResult::no_send(error("OBJECT_MUTATION_BEGIN_FAILED", format!("BEGIN IMMEDIATE失败，没有发送DROP：{e}")), entry.stopping());
    }
    let options = pool.connect_options();
    let current = match snapshot(conn, pool, &options, &plan.draft.object).await {
        Ok(s) => s, Err(e) => return rollback(conn, entry, error("OBJECT_MUTATION_METADATA_UNKNOWN", e.to_string())).await,
    };
    if current != *plan.opened.snapshot {
        return rollback(conn, entry, error("OBJECT_MUTATION_BASELINE_STALE", "锁内物理身份/cookie/rootpage/完整DDL/父表/siblings不等于原打开基线")).await;
    }
    if entry.stopping() { return rollback(conn, entry, error("OBJECT_MUTATION_STOPPED", "锁内停止，未发送DROP")).await; }
    let quoted = format!("\"{}\"", plan.draft.object.name.replace('"', "\"\""));
    // Exactly one backend-generated, schema-qualified DROP. No IF EXISTS/CASCADE.
    let sql = format!("DROP TRIGGER main.{quoted}");
    if let Err(e) = crate::sql_history::query(&sql).execute(&mut *conn).await {
        return rollback(conn, entry, error("OBJECT_MUTATION_DROP_FAILED", e.to_string())).await;
    }
    let (physical, schema_cookie, after) = match directory(conn, pool, &options).await {
        Ok(value) => value, Err(e) => return rollback(conn, entry, error("OBJECT_MUTATION_READBACK_UNKNOWN", e.to_string())).await,
    };
    let expected: Vec<_> = current.directory.iter().filter(|r| *r != &current.target).cloned().collect();
    if physical != current.physical || schema_cookie == current.schema_cookie || after != expected
        || after.iter().any(|r| r.kind == "trigger" && r.name == plan.draft.object.name)
        || !after.contains(&current.parent) {
        return rollback(conn, entry, error("OBJECT_MUTATION_READBACK_MISMATCH", "DROP后精确absence/父表/其它完整DDL未全部保持")).await;
    }
    if entry.stopping() { return rollback(conn, entry, error("OBJECT_MUTATION_STOPPED", "读回后停止，未提交DROP")).await; }
    let after_revision = revision(&(&physical, schema_cookie, &after));
    if let Err(e) = commit(conn, mode).await { return ApplyResult::uncertain(unknown(format!("对象DROP COMMIT回执丢失/未知：{e}；禁止重放"))); }
    if !matches!(autocommit(conn).await, Ok(true)) { return ApplyResult::uncertain(unknown("对象DROP COMMIT后事务结束未确认")); }
    let receipt = ObjectMutationReceipt { plan_id: plan.preview.plan_id.clone(), baseline_id: plan.draft.baseline_id.clone(),
        object: plan.draft.object.clone(), backend_object_id: plan.opened.public.backend_object_id.clone(),
        source_fingerprint: plan.draft.source_fingerprint.clone(), before_revision: plan.draft.base_revision.clone(),
        after_revision, status: "applied".into(), verified: true, transaction: "committed".into(), mismatches: Vec::new() };
    ApplyResult { result: Ok(MultiResult { sets: Vec::new(), messages: vec!["精确用户触发器DROP、父表/其它DDL读回和COMMIT已确认".into()], elapsed_ms: 0, spool_id: None }),
        confirmed: false, receipt: Some(receipt), no_effect: false, transaction: "committed".into() }
}
fn immediate(e: AppError, stopped: bool) -> BoundResult {
    BoundResult { result: Err(e), confirmed: stopped, receipt: None, no_effect: true, transaction: "none".into() }
}

/// `marker` is the actual signed ExecutionRequest.sql, not a client SQL body.
/// Parent must also verify RuntimeRegistry.matches and saved Binding before calling.
/// This worker owns db/connection/ordinary; C1 already owns activity until real drain.
pub async fn run(db: Arc<Db>, kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease,
    work: ObjectMutationWork, marker: String, read_only: bool) -> BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    run_inner(db, kind, entry, ordinary, work, marker, read_only, CommitMode::Normal).await

    }).await
}
async fn run_inner(db: Arc<Db>, kind: DbKind, entry: Arc<Entry>, ordinary: OrdinaryLease,
    work: ObjectMutationWork, marker: String, read_only: bool, mode: CommitMode) -> BoundResult {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    if let Err(e) = adapter(db.as_ref(), &kind, read_only) { return immediate(e, false); }
    if let Err(e) = validate_request(&work, &entry.binding, &marker) { return immediate(e, false); }
    if entry.stopping() { return immediate(error("EXECUTION_CANCELLED_BEFORE_SEND", "连接池等待前已停止，未发送DROP"), true); }
    let published = Arc::new(parking_lot::Mutex::new((None, false, "none".to_string())));
    let output = published.clone(); let working = entry.clone();
    let (result, confirmed) = execution::supervise(entry.clone(), async move {
        let _ordinary = ordinary;
        let pool = match db.as_ref() { Db::Sqlite(p) => p, _ => unreachable!() };
        let acquired = tokio::select! { biased;
            _ = working.wait_stop() => Err(error("EXECUTION_CANCELLED_BEFORE_SEND", "对象连接池等待已停止，未发送DROP")),
            conn = pool.acquire() => conn.map_err(AppError::from),
        };
        let mut conn = match acquired { Ok(c) => c, Err(e) => { *output.lock() = (None, true, "none".into()); return (Err(e), working.stopping()); } };
        if working.stopping() { conn.return_to_pool().await; *output.lock() = (None, true, "none".into()); return (Err(error("EXECUTION_CANCELLED_BEFORE_SEND", "固定连接准备期间停止，未发送DROP")), true); }
        let plan = match global().take(&work, &working.binding, &marker) {
            Ok(p) => p, Err(e) => { conn.return_to_pool().await; *output.lock() = (None, true, "none".into()); return (Err(e), false); }
        };
        if !working.start() { conn.return_to_pool().await; *output.lock() = (None, true, "none".into()); return (Err(error("EXECUTION_CANCELLED_BEFORE_SEND", "一次性消费后发送前停止，未发送DROP")), true); }
        if let Err(e) = execution::install_sqlite(&mut conn, &working, false).await {
            conn.close_on_drop(); let close = conn.close().await;
            return (Err(unknown(format!("对象DROP控制安装/关闭未确认：{e:#}；关闭={close:?}"))), false);
        }
        let proof = Arc::new(parking_lot::Mutex::new(false)); let proof_work = proof.clone();
        let (result, control_confirmed) = execution::drive(async {
            let report = apply(&mut conn, pool, &working, &plan, mode).await;
            *proof_work.lock() = report.confirmed;
            *output.lock() = (report.receipt, report.no_effect, report.transaction);
            report.result.map_err(anyhow::Error::from)
        }, &working, || async { Ok(()) }).await;
        let mut result = result.map_err(AppError::from);
        if let Err(e) = execution::clear_sqlite(&mut conn).await { result = Err(unknown(format!("对象DROP控制清理未确认：{e:#}"))); }
        if is_unknown(&result) {
            conn.close_on_drop();
            if let Err(e) = conn.close().await { result = Err(unknown(format!("对象DROP物理连接关闭未确认：{e}"))); }
        } else { conn.return_to_pool().await; }
        let confirmed = control_confirmed || *proof.lock();
        (result, confirmed)
    }).await;
    let (receipt, no_effect, transaction) = published.lock().clone();
    let uncertain = is_unknown(&result) || entry.stopping() && transaction == "committed";
    let result = if uncertain && result.is_ok() {
        Err(unknown("触发器DROP可能已提交，但停止与回执交错；不发布成功证明，禁止重放"))
    } else { result };
    BoundResult { receipt: if result.is_ok() && !uncertain { receipt } else { None },
        no_effect: !uncertain && no_effect, transaction: if uncertain { "unknown".into() } else { transaction }, result, confirmed }

    }).await
}

#[cfg(test)]
#[path = "object_mutation_tests.rs"]
mod tests;
