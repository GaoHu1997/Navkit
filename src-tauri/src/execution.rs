//! 单一执行注册表、单调预算和 pinned 连接控制。终态只在工作/控制清理后发布。
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicU8, Ordering}}, time::{Duration, Instant}};
use anyhow::{anyhow, Result};
use futures::TryStreamExt;
use sqlx::{Column, Connection, Executor, TypeInfo};
use tokio::sync::Notify;
use crate::{db::Db, error::{AppError, CmdResult}, execution_contracts::*, models::{DbKind, MultiResult, ResultColumn, ResultSet}};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Binding { pub principal: String, pub owner: String, pub connection: String, pub namespace: String, pub fingerprint: String }
pub fn audit_sql(sql: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("SQL SHA-256:{:x}; {} bytes（正文和字面量未记录）", Sha256::digest(sql.as_bytes()), sql.len())
}
pub struct Entry {
    pub id: String, pub binding: Binding, pub budget: Option<Duration>,
    inner: parking_lot::Mutex<Inner>, reason: AtomicU8, wake: Notify, changed: Notify,
    activity: parking_lot::Mutex<Option<tokio::sync::OwnedRwLockReadGuard<()>>>,
    /// 本次执行的结果缓存：执行器逐行落盘，界面预览仍然有界，分页与导出都读同一份快照。
    spool: parking_lot::Mutex<Option<Arc<crate::query_spool::Spool>>>,
    query_capture: parking_lot::Mutex<Option<Arc<crate::query_execution::QueryCapture>>>,
    remote_transaction: parking_lot::Mutex<Option<String>>,
}
struct Inner { state: ExecutionState, started: Option<Instant>, elapsed: u64, stop_confirmed: bool, work_settled: bool, finished: Option<Instant>, resources_pending: bool }
pub struct Registry { entries: parking_lot::Mutex<HashMap<String, Arc<Entry>>>, grants: parking_lot::Mutex<crate::execution_grants::Grants>, epoch: Instant }
impl Default for Registry { fn default() -> Self { Self { entries:Default::default(), grants:Default::default(), epoch:Instant::now() } } }
impl Registry {
    /// 真实 typed payload 的服务端摘要；不构造占位 SQL 或改变 SQL 请求字段。
    pub(crate) fn grant_typed_scope(&self, binding: Binding, scope: &str) -> CmdResult<ExecutionGrant> {
        if !scope.starts_with("typed/v1:") || scope.len() > 512 {
            return Err(AppError::coded("EXECUTION_SCOPE_INVALID", "受控任务摘要无效"));
        }
        self.grants.lock().issue(binding, scope, self.epoch.elapsed().as_millis() as u64)
    }
    /// 使用与 SQL 完全相同的一次性 grant、Entry、停止/预算和隔离注册表。
    pub(crate) fn register_typed_scope(&self, id: &str, token: &str, binding: Binding, scope: &str, timeout_ms: Option<u64>, default_secs: u64) -> CmdResult<Arc<Entry>> {
        let parsed = uuid::Uuid::parse_str(id).map_err(|_| AppError::coded("EXECUTION_ID_INVALID", "执行 ID 必须是规范 UUID"))?;
        if parsed.to_string() != id || parsed.is_nil() { return Err(AppError::coded("EXECUTION_ID_INVALID", "执行 ID 必须是非空规范 UUID")); }
        if binding.principal.is_empty() || binding.owner.trim().is_empty() || binding.owner.len() > 256 || binding.namespace.contains('\0') || !scope.starts_with("typed/v1:") || scope.len() > 512 {
            return Err(AppError::coded("EXECUTION_CONTEXT_INVALID", "受控任务上下文或摘要无效"));
        }
        let ms = timeout_ms.unwrap_or(default_secs.saturating_mul(1000));
        if ms > 86_400_000 { return Err(AppError::coded("EXECUTION_BUDGET_INVALID", "查询预算必须为 0..86400000 毫秒")); }
        let mut entries = self.entries.lock();
        Self::retire(&mut entries, Instant::now());
        if entries.contains_key(id) { return Err(AppError::coded("EXECUTION_ID_DUPLICATE", "此执行 ID 已经使用，不能重复执行").operation(id)); }
        if entries.values().filter(|e| { let s=e.status(); !s.state.terminal() || s.resources_pending }).count() >= 128 {
            return Err(AppError::coded("EXECUTION_BUSY", "活动或隔离执行达到并发上限，请等待清理"));
        }
        if entries.values().any(|e| e.binding.connection == binding.connection && e.status().resources_pending) {
            return Err(AppError::coded("EXECUTION_QUARANTINED", "此连接的控制或结果仍在隔离清理，不能发送新 SQL"));
        }
        self.grants.lock().consume(id, Some(token), &binding, scope, self.epoch.elapsed().as_millis() as u64)?;
        let entry = Arc::new(Entry { id: id.into(), binding, budget: (ms != 0).then(|| Duration::from_millis(ms)), inner: parking_lot::Mutex::new(Inner { state: ExecutionState::Queued, started: None, elapsed: 0, stop_confirmed: false, work_settled: false, finished: None, resources_pending: false }), reason: AtomicU8::new(0), wake: Notify::new(), changed: Notify::new(), activity: Default::default(), spool: Default::default(), query_capture: Default::default(), remote_transaction: Default::default() });
        entries.insert(id.into(), entry.clone()); Ok(entry)
    }
    fn request_scope(request: &ExecutionRequest) -> String {
        use sha2::{Digest, Sha256};
        // 身份/令牌由签发器生成；协商后的发送前停止标志不改变已授权工作内容。
        let bytes = serde_json::to_vec(&(&request.sql, &request.transaction_owner, request.timeout_ms, &request.origin)).expect("request scope serialization");
        format!("{:x}", Sha256::digest(bytes))
    }
    pub fn grant_request(&self, req: &mut ExecutionRequest, binding: Binding) -> CmdResult<()> {
        let grant = self.grants.lock().issue(binding, &Self::request_scope(req), self.epoch.elapsed().as_millis() as u64)?;
        req.execution_id = grant.execution_id; req.execution_grant = Some(grant.token); Ok(())
    }
    fn retire(entries: &mut HashMap<String, Arc<Entry>>, now: Instant) {
        entries.retain(|_,entry| { let i=entry.inner.lock(); i.resources_pending || !i.state.terminal() || i.finished.is_some_and(|t| now.duration_since(t)<Duration::from_secs(300)) });
        let mut finished:Vec<_>=entries.iter().filter_map(|(id,e)| { let i=e.inner.lock(); (i.state.terminal() && !i.resources_pending).then_some((id.clone(),i.finished)) }).collect();
        finished.sort_by_key(|(_,time)| *time);
        let excess=finished.len().saturating_sub(2048);
        for (id,_) in finished.into_iter().take(excess) { entries.remove(&id); }
    }
    /// 未终结或终态后仍有在途控制/结果的连接均不能被保活探测或新执行复用。
    pub fn has_active(&self, connection: &str) -> bool {
        self.entries.lock().values().any(|e| { let status = e.status(); e.binding.connection == connection && (!status.state.terminal() || status.resources_pending) })
    }
    pub fn require_quiescent(&self, connection: &str) -> CmdResult<()> {
        if self.entries.lock().values().any(|e| e.binding.connection==connection && e.status().resources_pending) { return Err(AppError::coded("EXECUTION_QUARANTINED", "此连接的控制或结果仍在隔离清理；禁止新执行和连接替换")); } Ok(())
    }
    pub async fn shutdown(&self) -> CmdResult<()> {
        let entries:Vec<_>=self.entries.lock().values().cloned().collect();
        for e in &entries { if !e.status().state.terminal() { e.request_stop(false); } }
        let waiting=async { for e in entries { loop { let notified=e.changed.notified(); tokio::pin!(notified); notified.as_mut().enable(); let s=e.status(); if s.state.terminal() && !s.resources_pending { break; } notified.await; } } };
        tokio::time::timeout(Duration::from_secs(10),waiting).await.map_err(|_| AppError::coded("EXECUTION_CLEANUP_PENDING", "执行资源尚未清理完成，退出被保留；不会丢弃在途控制或声称已回滚"))
    }
    /// Internal existence proof only; never exposes another owner's execution details.
    pub(crate) fn contains_execution(&self, id: &str) -> bool { self.entries.lock().contains_key(id) }
    #[cfg(test)] pub(crate) fn entry_for_test(&self, id: &str) -> Option<Arc<Entry>> { self.entries.lock().get(id).cloned() }
    pub fn register(&self, req: &ExecutionRequest, binding: Binding, default_secs: u64) -> CmdResult<Arc<Entry>> {
        let parsed = uuid::Uuid::parse_str(&req.execution_id).map_err(|_| AppError::coded("EXECUTION_ID_INVALID", "执行 ID 必须是规范 UUID"))?;
        if parsed.to_string() != req.execution_id || parsed.is_nil() { return Err(AppError::coded("EXECUTION_ID_INVALID", "执行 ID 必须是非空规范 UUID")); }
        if req.owner_id.trim().is_empty() || req.owner_id.len() > 256 || req.namespace.contains('\0') { return Err(AppError::coded("EXECUTION_CONTEXT_INVALID", "执行上下文无效")); }
        if req.transaction_owner.as_ref().is_some_and(|o| o != &req.owner_id) { return Err(AppError::coded("EXECUTION_OWNER_MISMATCH", "事务所有者必须与执行页一致")); }
        if binding.owner != req.owner_id || binding.connection != req.connection_id || binding.namespace != req.namespace {
            return Err(AppError::coded("EXECUTION_CONTEXT_INVALID", "执行授权上下文与请求不匹配，没有发送 SQL"));
        }
        let ms = req.timeout_ms.unwrap_or(default_secs.saturating_mul(1000));
        if ms > 86_400_000 { return Err(AppError::coded("EXECUTION_BUDGET_INVALID", "查询预算必须为 0..86400000 毫秒")); }
        let mut entries = self.entries.lock();
        Self::retire(&mut entries,Instant::now());
        if entries.contains_key(&req.execution_id) { return Err(AppError::coded("EXECUTION_ID_DUPLICATE", "此执行 ID 已经使用，不能重复执行").operation(&req.execution_id)); }
        if entries.values().filter(|e| !e.status().state.terminal() || e.status().resources_pending).count()>=128 { return Err(AppError::coded("EXECUTION_BUSY", "活动或隔离执行达到并发上限，请等待清理")); }
        if entries.values().any(|e| e.binding.connection == req.connection_id && e.status().resources_pending) {
            return Err(AppError::coded("EXECUTION_QUARANTINED", "此连接的控制或结果仍在隔离清理，不能发送新 SQL"));
        }
        self.grants.lock().consume(&req.execution_id, req.execution_grant.as_deref(), &binding, &Self::request_scope(req), self.epoch.elapsed().as_millis() as u64)?;
        let entry = Arc::new(Entry { id: req.execution_id.clone(), binding, budget: (ms != 0).then(|| Duration::from_millis(ms)), inner: parking_lot::Mutex::new(Inner { state: ExecutionState::Queued, started: None, elapsed: 0, stop_confirmed: false, work_settled: false, finished:None, resources_pending:false }), reason: AtomicU8::new(0), wake: Notify::new(), changed:Notify::new(), activity:Default::default(), spool:Default::default(), query_capture:Default::default(), remote_transaction:Default::default() });
        if req.cancel_before_send { entry.request_stop(false); }
        entries.insert(entry.id.clone(), entry.clone()); Ok(entry)
    }
    pub fn authorized(&self, id: &str, binding: &Binding) -> CmdResult<Arc<Entry>> {
        let entry = self.entries.lock().get(id).cloned().ok_or_else(|| AppError::coded("EXECUTION_NOT_FOUND", "执行记录不存在"))?;
        if &entry.binding != binding { return Err(AppError::coded("EXECUTION_FORBIDDEN", "执行所有者、连接版本或命名空间不匹配")); }
        Ok(entry)
    }
}
impl Entry {
    pub(crate) fn own_activity(&self, guard: tokio::sync::OwnedRwLockReadGuard<()>) { *self.activity.lock()=Some(guard); }
    /// 挂上结果缓存；take 只在执行终结时调用，避免 spool 无限期占用临时文件。
    pub(crate) fn attach_spool(&self, cache: Arc<crate::query_spool::Spool>) { *self.spool.lock() = Some(cache); }
    pub(crate) fn spool(&self) -> Option<Arc<crate::query_spool::Spool>> { self.spool.lock().clone() }
    pub(crate) fn take_spool(&self) -> Option<Arc<crate::query_spool::Spool>> { self.spool.lock().take() }
    pub(crate) fn attach_query_capture(&self, capture: Arc<crate::query_execution::QueryCapture>) { *self.query_capture.lock() = Some(capture); }
    pub(crate) fn query_capture(&self) -> Option<Arc<crate::query_execution::QueryCapture>> { self.query_capture.lock().clone() }
    pub(crate) fn record_remote_transaction(&self, value: &str) {
        assert!(["committed", "rolled_back", "none", "unknown"].contains(&value), "invalid verified remote transaction state");
        *self.remote_transaction.lock() = Some(value.to_owned());
    }
    pub(crate) fn quarantine(&self) { self.inner.lock().resources_pending=true; self.changed.notify_waiters(); }
    pub(crate) fn drained(&self) { let terminal={ let mut i=self.inner.lock(); i.resources_pending=false; i.state.terminal() }; if terminal { self.activity.lock().take(); } self.changed.notify_waiters(); }
    pub(crate) async fn wait_started(&self) {
        loop { let notified=self.changed.notified(); tokio::pin!(notified); notified.as_mut().enable(); if self.inner.lock().started.is_some() { return; } notified.await; }
    }
    pub async fn wait_stop(&self) {
        // 停止是广播事实；drive、池等待和监督器不能争抢一个 Notify permit。
        loop { let notified=self.changed.notified(); tokio::pin!(notified); notified.as_mut().enable(); if self.reason.load(Ordering::SeqCst) != 0 { return; } notified.await; }
    }
    pub fn start(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.state.terminal() || inner.work_settled || self.reason.load(Ordering::SeqCst) != 0 { return false; }
        if inner.started.is_none() { inner.started = Some(Instant::now()); }
        inner.state = ExecutionState::Running; self.changed.notify_waiters(); true
    }
    pub fn request_stop(&self, timeout: bool) -> CancelAcknowledgement {
        let mut inner = self.inner.lock();
        let accepted = !inner.state.terminal() && !inner.work_settled;
        if accepted && self.reason.compare_exchange(0, if timeout { 2 } else { 1 }, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            inner.state = ExecutionState::CancelRequested; self.wake.notify_one(); self.changed.notify_waiters();
        }
        CancelAcknowledgement { execution_id: self.id.clone(), accepted, state: inner.state, message: if accepted { "已请求停止，等待数据库确认；不代表已取消或回滚" } else { "数据库工作已经结束，正在清理或已经终结；没有发送控制请求" }.into() }
    }
    pub fn stopping(&self) -> bool {
        if self.reason.load(Ordering::SeqCst) != 0 { return true; }
        let expired = { let inner = self.inner.lock(); !inner.work_settled && self.budget.zip(inner.started).is_some_and(|(budget, start)| start.elapsed() >= budget) };
        if expired { self.request_stop(true); } expired
    }
    pub fn status(&self) -> ExecutionStatus { let inner = self.inner.lock(); ExecutionStatus { execution_id: self.id.clone(), state: inner.state, deadline: self.deadline(&inner), resources_pending:inner.resources_pending } }
    fn remaining_budget_ms(&self) -> Option<u64> { let inner = self.inner.lock(); self.budget.map(|b| b.saturating_sub(inner.started.map_or(Duration::ZERO, |s| s.elapsed())).as_nanos().div_ceil(1_000_000).max(1) as u64) }
    /// 结果已排空时冻结停止资格；终态仍等控制和连接清理后再发布。
    pub(crate) fn work_settled(&self) {
        { let mut inner = self.inner.lock(); if !inner.state.terminal() { inner.elapsed = inner.started.map_or(0, |s| s.elapsed().as_millis() as u64); } inner.work_settled = true; }
        self.changed.notify_waiters();
    }
    pub(crate) async fn wait_work_settled(&self) {
        loop { let notified = self.changed.notified(); tokio::pin!(notified); notified.as_mut().enable(); if self.inner.lock().work_settled { return; } notified.await; }
    }
    fn deadline(&self, inner: &Inner) -> DeadlineInfo {
        DeadlineInfo { execution_id: self.id.clone(), budget_ms: self.budget.map(|d| d.as_millis() as u64), elapsed_ms: if inner.state.terminal() || inner.work_settled { inner.elapsed } else { inner.started.map_or(0, |s| s.elapsed().as_millis() as u64) }, deadline_reached: self.reason.load(Ordering::SeqCst) == 2, stop_confirmed: inner.stop_confirmed }
    }
    pub fn finish(&self, result: CmdResult<MultiResult>, confirmed: bool, transaction: bool) -> ExecutionOutcome {
        let mut inner = self.inner.lock();
        if !inner.work_settled { inner.elapsed = inner.started.map_or(0, |s| s.elapsed().as_millis() as u64); }
        let reason = self.reason.load(Ordering::SeqCst);
        let uncertain = result.as_ref().err().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN");
        let state = if uncertain { ExecutionState::Unknown } else if reason == 0 { if result.is_ok() { ExecutionState::Succeeded } else { ExecutionState::Failed } }
            else if confirmed { if reason == 2 { ExecutionState::TimedOut } else { ExecutionState::Cancelled } }
            else { ExecutionState::Unknown };
        assert!(!inner.state.terminal(), "一个执行只能终结一次");
        inner.state = state; inner.stop_confirmed = reason != 0 && confirmed;
        inner.finished=Some(Instant::now());
        if !inner.resources_pending { self.activity.lock().take(); } self.changed.notify_waiters();
        let remote_transaction = self.remote_transaction.lock().clone();
        let no_effect = confirmed && result.as_ref().err().is_some_and(|e| e.code == "EXECUTION_CANCELLED_BEFORE_SEND")
            || state != ExecutionState::Unknown && matches!(remote_transaction.as_deref(), Some("rolled_back" | "none"));
        let (value, error) = match result {
            Ok(v) if state == ExecutionState::Succeeded => (Some(v), None),
            Ok(_) => (None, Some(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", "停止与数据库完成存在竞态，不能把迟到结果声称为成功或已回滚").operation(&self.id))),
            Err(mut e) => {
                match state { ExecutionState::Cancelled => e.code = "CANCELLED".into(), ExecutionState::TimedOut => e.code = "QUERY_TIMEOUT".into(), ExecutionState::Unknown => e.code = "EXECUTION_EFFECTS_UNKNOWN".into(), _ => {} }
                (None, Some(e.operation(&self.id)))
            }
        };
        // 按最终终态处理缓存，不能只看数据库返回 Ok：停止/完成竞态仍可能是 Unknown。
        // 完成时解除 Entry 的强引用；正在排空的任务持有自己的 Arc，最后释放时再清文件。
        if let Some(cache) = self.take_spool() {
            if state != ExecutionState::Succeeded { crate::query_spool::drop_spool(&cache.id); }
        }
        if state != ExecutionState::Succeeded { if let Some(capture) = self.query_capture() { capture.release(); } }
        ExecutionOutcome { execution_id: self.id.clone(), state, effects: if no_effect { "no_effect" } else if value.is_some() { "completed" } else if reason != 0 || state == ExecutionState::Unknown { "possibly_applied" } else { "possibly_partial" }.into(), result: value, error, deadline: self.deadline(&inner), transaction_state: if state == ExecutionState::Unknown && (transaction || remote_transaction.is_some()) { "unknown" } else if let Some(remote) = remote_transaction.as_deref() { remote } else if transaction && state != ExecutionState::Succeeded { "rollback_required" } else if transaction { "active" } else { "none" }.into(), resources_pending:inner.resources_pending }
    }
}

pub fn capabilities(db: Option<&Db>, kind: &DbKind, read_only: bool, native_candidate: bool) -> Capabilities {
    let sqlite = matches!(db, Some(Db::Sqlite(_))) || db.is_none() && native_candidate && kind.is_sqlite();
    let native = matches!(db, Some(Db::Sqlite(_) | Db::Mysql(_) | Db::Postgres(_))) || db.is_none() && native_candidate && matches!(kind.protocol(), crate::models::Protocol::Sqlite | crate::models::Protocol::Mysql | crate::models::Protocol::Postgres);
    let tunnel = matches!(db, Some(Db::Http(http)) if http.tunnel_runtime().is_some());
    let cap = |level: &str, reason: &str| Capability { level: level.into(), reason: reason.into() };
    if tunnel {
        return Capabilities { version: 1, protocol: "navkit-sql-tunnel/1".into(), read_only,
            arbitrary_sql: cap("limited", "单条受支持SELECT或参数化DML；服务端完整解析并核对命名空间，DDL/脚本/函数/手动事务未认证"),
            cancel: cap("limited", "独立HTTPS控制通道请求固定远端会话与执行；只有已验证回滚回执才确认取消"),
            deadline: cap("limited", "C1单调预算与远端语句期限；控制或完整流缺失时隔离会话并报告unknown"),
            manual_transaction: cap("unsupported", "参考隧道尚未认证跨请求手动事务"),
            safe_browse: cap("limited", "仅浏览已授权命名空间内的实体表；元数据错误不当作空目录") };
    }
    Capabilities { version: 1, protocol: format!("{:?}", kind.protocol()).to_lowercase(), read_only,
        arbitrary_sql: if read_only { cap("limited", if sqlite { "仅已证明的实体表列投影和常量 SELECT；函数/表达式/视图不执行" } else { "此驱动尚不能证明任意 SQL 无副作用；只读 SQL 拒绝，保留结构化浏览" }) } else { cap("supported", "保存连接的后端权限决定执行资格") },
        cancel: if native { cap(if sqlite { "supported" } else { "limited" }, if sqlite { "固定 SQLite VM progress handler；事务中断后只允许回滚" } else { "固定物理连接 + 独立定向控制；需要服务器取消权限，失败报告 unknown" }) } else { cap("unsupported", "此适配器未验证安全的独立控制通道；不能宣称停止服务端执行") },
        deadline: if native { cap(if sqlite { "supported" } else { "limited" }, if sqlite { "批次/结果读取使用单调预算；0 禁用应用预算。busy 锁等待可能延迟停止，未确认时返回 unknown" } else { "批次/结果读取使用单调预算；PG 使用剩余 statement_timeout，定向控制失败返回 unknown" }) } else { cap("limited", "原有驱动请求超时仍存在；不能证明服务端停止，应用执行预算不能强制取消") },
        manual_transaction: if native && !read_only { cap("supported", "现有固定事务连接，取消后 rollback-required") } else { cap("unsupported", "当前连接不支持手动写事务") },
        safe_browse: if read_only { cap("limited", "保留结构化浏览；复杂视图/虚表/自定义类型的副作用尚未逐驱动证明，不能把任意对象浏览视作纯读保证") } else { cap("supported", "使用现有结构化对象/表数据浏览入口") } }
}

/// 工作和独立控制都必须真实结束后才能释放 pinned 连接；有界对外等待由 supervise 完成。
pub(crate) async fn drive<F, C, CF>(work: F, entry: &Arc<Entry>, control: C) -> (Result<MultiResult>, bool)
where F: std::future::Future<Output=Result<MultiResult>>, C: FnOnce() -> CF, CF: std::future::Future<Output=Result<()>> {
    tokio::pin!(work);
    let remaining = { let inner = entry.inner.lock(); entry.budget.map(|b| b.saturating_sub(inner.started.expect("started").elapsed())) };
    let deadline = async { match remaining { Some(d) => tokio::time::sleep(d).await, None => std::future::pending::<()>().await } };
    tokio::pin!(deadline);
    let outcome = tokio::select! {
        biased;
        result = &mut work => {
            // 不依据完成后才观察到的 elapsed 或外部 57014 推断本次取消。
            let requested = entry.reason.load(Ordering::SeqCst) != 0;
            entry.work_settled();
            let confirmed = requested && is_interrupted(&result);
            (result, confirmed)
        }
        _ = entry.wait_stop() => {
            // join 不会丢弃已发送查询/控制 future，也不会等控制时停止消费结果。
            let (controlled, result) = tokio::join!(control(), &mut work);
            let confirmed = controlled.is_ok() && is_interrupted(&result);
            (result, confirmed)
        }
        _ = &mut deadline => {
            entry.request_stop(true);
            let (controlled, result) = tokio::join!(control(), &mut work);
            let confirmed = controlled.is_ok() && is_interrupted(&result);
            (result, confirmed)
        }
    };
    entry.work_settled();
    outcome
}

pub(crate) async fn supervise<F>(entry: Arc<Entry>, work: F) -> (CmdResult<MultiResult>, bool)
where F: std::future::Future<Output=(CmdResult<MultiResult>, bool)> + Send + 'static {
    supervise_with_grace(entry, work, Duration::from_secs(10)).await
}
pub(crate) async fn supervise_with_grace<F>(entry: Arc<Entry>, work: F, grace: Duration) -> (CmdResult<MultiResult>, bool)
where F: std::future::Future<Output=(CmdResult<MultiResult>, bool)> + Send + 'static {
    let mut task = crate::sql_history::spawn(work);
    let deadline = async {
        if let Some(budget) = entry.budget {
            entry.wait_started().await;
            let remaining = { let inner = entry.inner.lock(); budget.saturating_sub(inner.started.expect("started").elapsed()) };
            tokio::time::sleep(remaining).await;
            entry.request_stop(true);
            // 已排空的工作拒绝停止；此时只等待真正的任务清理结果。
            entry.wait_stop().await;
        } else { std::future::pending::<()>().await; }
    };
    tokio::pin!(deadline);
    let settled = tokio::select! {
        biased;
        result = &mut task => Some(result),
        _ = entry.wait_stop() => None,
        // 排空结果后若归还/销毁连接挂起，也必须有界报告 Unknown 并保留资源。
        _ = entry.wait_work_settled() => None,
        _ = &mut deadline => None,
    };
    let settled = match settled {
        Some(result) => Some(result),
        None => match tokio::time::timeout(grace, &mut task).await { Ok(result) => Some(result), Err(_) => None },
    };
    match settled {
        Some(Ok(result)) => result,
        Some(Err(error)) => {
            entry.quarantine();
            (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("执行工作任务异常结束，资源清理无法证明，连接保持隔离：{error}"))), false)
        }
        None => {
            entry.quarantine();
            let draining = entry.clone();
            // 只转移拥有任务，不 abort/丢弃 Future。迟到结果不能将 Unknown 改成成功。
            crate::sql_history::spawn(async move {
                if task.await.is_ok() { draining.work_settled(); draining.drained(); }
            });
            (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", "停止后的有界等待已结束；数据库工作/控制仍在隔离排空，写入结果未知，禁止重放")), false)
        }
    }
}
fn is_interrupted(result: &Result<MultiResult>) -> bool {
    let Some(error) = result.as_ref().err().and_then(|e| e.downcast_ref::<sqlx::Error>()).and_then(|e| e.as_database_error()) else { return false; };
    // MySQL DatabaseError.code() 是 SQLSTATE，1317 必须用实际驱动的 number() 读取。
    if let Some(mysql) = error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>() { return mysql.number() == 1317; }
    error.code().is_some_and(|c| matches!(c.as_ref(), "9" | "57014"))
}

fn columns(names: Vec<String>, types: Vec<String>) -> Vec<ResultColumn> {
    names.into_iter().zip(types).map(|(name, data_type)| ResultColumn { name, data_type, nullable: true, primary_key: false, auto_increment: false }).collect()
}
const RESULT_PREVIEW_ROWS: usize = 10_000;
const RESULT_PREVIEW_BYTES: usize = 50 * 1024 * 1024;

/// 显示预算不是执行预算：停止保留行后仍须消费完整响应，不能将截断预览
/// 当成写入状态未知，也不能在事务连接上留下未读结果。每个结果保留连续前缀。
struct ResultPreview {
    rows: Vec<Vec<Option<String>>>,
    total: u64,
    full: bool,
}
impl ResultPreview {
    fn new() -> Self { Self { rows: Vec::new(), total: 0, full: false } }
    fn observe(&mut self) { self.total += 1; }
    fn retain(&mut self, values: Vec<Option<String>>, remaining: &mut usize) {
        let bytes = std::mem::size_of::<Vec<Option<String>>>()
            + values.capacity() * std::mem::size_of::<Option<String>>()
            + values.iter().flatten().map(String::capacity).sum::<usize>();
        if self.rows.len() >= RESULT_PREVIEW_ROWS || bytes > *remaining {
            self.full = true;
            return;
        }
        *remaining -= bytes;
        self.rows.push(values);
        self.full = self.rows.len() >= RESULT_PREVIEW_ROWS;
    }
    fn message(&self) -> String {
        if self.total > self.rows.len() as u64 {
            format!("查询已完成，共 {} 行；当前仅显示前 {} 行（预览上限 10000 行 / 50 MiB）。数据库响应已完整读取；复制、筛选和导出仅包含当前预览，查看其余数据请使用 LIMIT / OFFSET 分页查询。", self.total, self.rows.len())
        } else { String::new() }
    }
}

macro_rules! batch {
    ($fn:ident, $conn:ty, $values:path, $split:path $(, $prepare:path)?) => {
        pub(crate) async fn $fn(conn: &mut $conn, sql: &str, entry: &Arc<Entry>) -> Result<MultiResult> {
            let start = Instant::now(); let mut sets = Vec::new(); let mut messages = Vec::new();
            // 结果缓存由命令层在执行前挂到 Entry 上；界面预览仍然有界，但要缓存完整结果。
            let cache = entry.spool();
            // 预算覆盖整个批次，防止多结果集绕过内存上限。
            let mut remaining = RESULT_PREVIEW_BYTES;
            for stmt in $split(sql).into_iter().filter(|s| crate::db::is_executable(s)) {
                if entry.stopping() { return Err(anyhow!("执行已请求停止；剩余批次没有发送")); }
                let statement_start = Instant::now();
                $($prepare(conn, entry).await?;)?
                let d = crate::sql_history::describe(&mut *conn,&stmt).await?;
                let names: Vec<_> = d.columns().iter().map(|c| c.name().to_string()).collect();
                let types: Vec<_> = d.columns().iter().map(|c| c.type_info().name().to_string()).collect();
                if names.is_empty() {
                    let affected = crate::sql_history::query(&stmt).execute(&mut *conn).await?.rows_affected();
                    messages.push(format!("{} 行受影响（取消不保证回滚）", affected));
                    sets.push(ResultSet { columns: vec![], rows: vec![], affected, elapsed_ms: statement_start.elapsed().as_millis() as u64, ok: true, message: String::new(), has_rows: false, primary_keys: vec![], total: 0 });
                } else {
                    let target = cache.as_ref().map(|c| c.begin_set(sets.len(), &names, &types));
                    let mut stream = crate::sql_history::query(&stmt).fetch(&mut *conn);
                    let mut preview = ResultPreview::new();
                    while let Some(row) = stream.try_next().await? {
                        preview.observe();
                        // 同一行的值只能转换一次：先写缓存，再按预览预算保留。
                        let values = $values(&row, names.len());
                        if let Some(target) = &target { target.push(&values); }
                        if !preview.full { preview.retain(values, &mut remaining); }
                        // 定期让出执行器，防止已经缓冲的行 + 同步落盘饿死独立取消/超时分支。
                        if preview.total % 128 == 0 { tokio::task::yield_now().await; }
                    }
                    if let Some(target) = &target { target.finish(); }
                    let message = match target.as_ref() {
                        Some(target) if target.truncated() => format!(
                            "查询已完成，共 {} 行；结果缓存不完整（{}），可读取 {} 行；完整导出将拒绝不完整缓存，不会重新执行 SQL。",
                            preview.total, target.stop_reason().unwrap_or_else(|| "未知原因".into()), target.total()),
                        Some(_) if preview.total > preview.rows.len() as u64 => format!(
                            "查询已完成，共 {} 行；当前返回前 {} 行的内存预览。查询页可分页读取本次执行缓存，导出默认包含全部行，不会重新执行 SQL。",
                            preview.total, preview.rows.len()),
                        _ => preview.message(),
                    };
                    if !message.is_empty() { messages.push(format!("结果 {}：{}", sets.len() + 1, message)); }
                    sets.push(ResultSet { columns: columns(names, types), rows: preview.rows, affected: 0, elapsed_ms: statement_start.elapsed().as_millis() as u64, ok: true, message, has_rows: true, primary_keys: vec![], total: preview.total });
                }
            }
            let mut result = MultiResult { sets, messages, elapsed_ms: start.elapsed().as_millis() as u64, spool_id: None };
            if let Some(cache) = cache { cache.finish_all(); result.spool_id = Some(cache.id.clone()); }
            Ok(result)
        }
    }
}
batch!(sqlite_batch, sqlx::SqliteConnection, crate::values::sqlite_row_values, crate::db::split_sqlite);
batch!(mysql_batch, sqlx::MySqlConnection, crate::values::my_row_values, crate::db::split_statements);
batch!(pg_batch, sqlx::PgConnection, crate::values::pg_row_values, crate::db::split_statements, prepare_pg_deadline);

async fn pg_statement_timeout(conn: &mut sqlx::PgConnection) -> Result<i64> {
    Ok(crate::sql_history::query_scalar("SELECT setting::bigint FROM pg_catalog.pg_settings WHERE name='statement_timeout'").fetch_one(conn).await?)
}
async fn set_pg_timeout(conn: &mut sqlx::PgConnection, milliseconds: i64) -> Result<()> {
    // 整批绑定会话；事务内失败时不再发送恢复 SQL，普通连接在完成后销毁。
    crate::sql_history::query("SELECT pg_catalog.set_config('statement_timeout', $1, false)").bind(format!("{milliseconds}ms")).execute(conn).await?;
    Ok(())
}
pub(crate) async fn prepare_pg_deadline(conn: &mut sqlx::PgConnection, entry: &Arc<Entry>) -> Result<()> {
    let Some(remaining) = entry.remaining_budget_ms() else { return Ok(()); };
    let existing = pg_statement_timeout(conn).await?;
    let timeout = if existing > 0 { (remaining as i64).min(existing) } else { remaining as i64 };
    set_pg_timeout(conn, timeout).await
}

/// 固定 sqlite handle 生命周期；无跨线程裸指针中断。authorizer 不允许任何函数身份。
pub(crate) async fn install_sqlite(conn: &mut sqlx::SqliteConnection, entry: &Arc<Entry>, read_only: bool) -> Result<()> {
    if read_only {
        crate::sql_history::query("PRAGMA query_only=ON").execute(&mut *conn).await?;
        let flag: i64 = crate::sql_history::query_scalar("PRAGMA query_only").fetch_one(&mut *conn).await?;
        if flag != 1 { return Err(anyhow!("SQLite query_only 未建立")); }
    }
    let mut handle = conn.lock_handle().await?;
    let e = entry.clone(); handle.set_progress_handler(1000, move || !e.stopping());
    if read_only {
        // SAFETY: 持有 SQLx worker handle 锁；callback 不持有用户数据或分配/调用 SQLite。
        let rc = unsafe { libsqlite3_sys::sqlite3_set_authorizer(handle.as_raw_handle().as_ptr(), Some(read_authorizer), std::ptr::null_mut()) };
        if rc != libsqlite3_sys::SQLITE_OK { return Err(anyhow!("SQLite authorizer 安装失败")); }
    }
    Ok(())
}
unsafe extern "C" fn read_authorizer(_: *mut std::ffi::c_void, action: i32, _: *const std::ffi::c_char, _: *const std::ffi::c_char, database: *const std::ffi::c_char, source: *const std::ffi::c_char) -> i32 {
    // SQLite 提供 callback 参数的有效 NUL 终止字符串。禁止 temp/attached 对象遮蔽已验证 main 表。
    let main = !database.is_null() && std::ffi::CStr::from_ptr(database).to_bytes() == b"main";
    if source.is_null() && (action == libsqlite3_sys::SQLITE_SELECT || action == libsqlite3_sys::SQLITE_READ && main) { libsqlite3_sys::SQLITE_OK } else { libsqlite3_sys::SQLITE_DENY }
}
pub(crate) async fn clear_sqlite(conn: &mut sqlx::SqliteConnection) -> Result<()> {
    let mut h = conn.lock_handle().await?; h.remove_progress_handler();
    let rc = unsafe { libsqlite3_sys::sqlite3_set_authorizer(h.as_raw_handle().as_ptr(), None, std::ptr::null_mut()) };
    if rc != libsqlite3_sys::SQLITE_OK { return Err(anyhow!("SQLite authorizer 移除失败")); }
    Ok(())
}
pub(crate) async fn sqlite_run(conn: &mut sqlx::SqliteConnection, sql: &str, entry: &Arc<Entry>, read_only: bool) -> (Result<MultiResult>, bool) {
    if entry.stopping() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    if !entry.start() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    if read_only {
        // 固定读快照：预审实体表和实际 prepare/读取不会跨 schema generation。
        if let Err(e) = crate::sql_history::query("BEGIN DEFERRED").execute(&mut *conn).await { return (Err(e.into()), false); }
    }
    let outcome = async {
        if read_only {
            let plan = crate::execution_policy::readonly_batch(sql)?;
            for table in plan.tables {
                let def: Option<(String, String)> = crate::sql_history::query_as("SELECT type, sql FROM main.sqlite_schema WHERE name = ? AND type='table'").bind(&table).fetch_optional(&mut *conn).await?;
                if !def.is_some_and(|(kind, sql)| kind == "table" && !sql.to_ascii_uppercase().contains("VIRTUAL TABLE")) { return Err(anyhow!("只读 SQL 只能读取已验证 main 实体表，不能读取视图/虚表/附加库")); }
            }
        }
        install_sqlite(conn, entry, read_only).await?;
        if !entry.start() { return Ok((Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "元数据/回调准备期间停止，没有发送用户 SQL").into()), true)); }
        Ok::<_, anyhow::Error>(drive(crate::query_execution::sqlite_batch(conn, sql, entry), entry, || async { Ok(()) }).await)
    }.await.unwrap_or_else(|e| (Err(e), false));
    let cleanup = async {
        clear_sqlite(conn).await?;
        if read_only { crate::sql_history::query("ROLLBACK").execute(&mut *conn).await?; }
        Ok::<_, anyhow::Error>(())
    }.await;
    if let Err(e) = cleanup { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("SQLite 清理失败，连接必须销毁：{e:#}")).into()), false); }
    outcome
}
pub(crate) async fn mysql_run(conn: &mut sqlx::MySqlConnection, pool: &sqlx::MySqlPool, sql: &str, entry: &Arc<Entry>) -> (Result<MultiResult>, bool) {
    if entry.stopping() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    if !entry.start() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    let target = match crate::sql_history::query_scalar::<_, u64>("SELECT CONNECTION_ID()").fetch_one(&mut *conn).await { Ok(n) => n, Err(e) => return (Err(e.into()), false) };
    let opts = pool.connect_options();
    drive(mysql_batch(conn, sql, entry), entry, || async move {
        let mut control = sqlx::MySqlConnection::connect_with(&opts).await?;
        let result = crate::sql_history::query(&format!("KILL QUERY {target}")).execute(&mut control).await;
        let closed = control.close().await;
        result?; closed?; Ok::<_, anyhow::Error>(())
    }).await
}
pub(crate) async fn pg_run(conn: &mut sqlx::PgConnection, pool: &sqlx::PgPool, sql: &str, entry: &Arc<Entry>, transaction: bool) -> (Result<MultiResult>, bool) {
    if entry.stopping() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    if !entry.start() { return (Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "停止发生在发送前，没有执行用户 SQL").into()), true); }
    let target = match crate::sql_history::query_scalar::<_, i32>("SELECT pg_catalog.pg_backend_pid()").fetch_one(&mut *conn).await { Ok(n) => n, Err(e) => return (Err(e.into()), false) };
    let opts = pool.connect_options();
    drive(async {
        let original = if transaction && entry.budget.is_some() { Some(pg_statement_timeout(conn).await?) } else { None };
        let result = pg_batch(conn, sql, entry).await;
        if result.is_ok() && !entry.stopping() {
            if let Some(original) = original { set_pg_timeout(conn, original).await?; }
        }
        result
    }, entry, || async move {
        let mut control = sqlx::PgConnection::connect_with(&opts).await?;
        let stopped = crate::sql_history::query_scalar::<_, bool>("SELECT pg_catalog.pg_cancel_backend($1)").bind(target).fetch_one(&mut control).await;
        let closed = control.close().await;
        let stopped = stopped?; closed?;
        if !stopped { return Err(anyhow!("PostgreSQL 没有确认定向取消")); }
        Ok::<_, anyhow::Error>(())
    }).await
}

async fn acquire<DB: sqlx::Database>(pool: &sqlx::Pool<DB>, entry: &Arc<Entry>) -> CmdResult<sqlx::pool::PoolConnection<DB>> {
    tokio::select! {
        biased;
        _ = entry.wait_stop() => Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "池等待已取消，没有发送 SQL")),
        result = pool.acquire() => result.map_err(AppError::from),
    }
}

/// 普通执行每批固定一条物理连接；SQLite VM 排空/回调移除后还池，远端定向控制连接销毁。
pub async fn run(db: &Db, kind: &DbKind, sql: &str, entry: &Arc<Entry>, read_only: bool) -> (CmdResult<MultiResult>, bool) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let owned = match db {
        Db::Sqlite(pool) => Db::Sqlite(pool.clone()),
        Db::Mysql(pool) => Db::Mysql(pool.clone()),
        Db::Postgres(pool) => Db::Postgres(pool.clone()),
        _ => return (Err(AppError::coded("EXECUTION_ADAPTER_REQUIRED", "该适配器使用统一 command 服务内的真实 legacy 路线")), false),
    };
    let sql = sql.to_string(); let kind = kind.clone(); let working = entry.clone();
    supervise(entry.clone(), async move { run_pinned(&owned, &kind, &sql, &working, read_only).await }).await

    }).await
}
/// The owning lifecycle lease lives in the same supervised worker as SQL and physical cleanup.
pub(crate) async fn run_owned(db: Arc<Db>, kind: DbKind, sql: String, entry: Arc<Entry>, read_only: bool, lease: crate::manual_transaction::Lease) -> (CmdResult<MultiResult>, bool) {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let working = entry.clone();
    supervise(entry, async move {
        let _lease = lease;
        run_pinned(db.as_ref(), &kind, &sql, &working, read_only).await
    }).await

    }).await
}
async fn run_pinned(db: &Db, _kind: &DbKind, sql: &str, entry: &Arc<Entry>, read_only: bool) -> (CmdResult<MultiResult>, bool) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    if read_only && !matches!(db, Db::Sqlite(_)) { return (Err(AppError::coded("READ_ONLY_UNPROVEN", "当前驱动只读任意 SQL 无法证明安全，已拒绝；请使用结构化浏览")), false); }
    let result = match db {
        Db::Sqlite(pool) => match acquire(pool, entry).await { Ok(mut c) => {
            let outcome = sqlite_run(&mut c, sql, entry, read_only).await;
            if outcome.0.as_ref().err().is_some_and(|e| e.downcast_ref::<AppError>().is_some_and(|e| e.code == "EXECUTION_EFFECTS_UNKNOWN")) {
                c.close_on_drop();
                if let Err(e) = c.close().await { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("SQLite 物理连接清理失败：{e}"))), false); }
            } else {
                // SQLx 0.8.6 的实际公开 API：等待 ping/归还完成，避免 Drop 的后台归还跨过终态。
                c.return_to_pool().await;
            }
            outcome
        }, Err(e) => { let unsent = e.code == "EXECUTION_CANCELLED_BEFORE_SEND"; (Err(e.into()), unsent) } },
        Db::Mysql(pool) => match acquire(pool, entry).await { Ok(mut c) => {
            c.close_on_drop(); let outcome = mysql_run(&mut c, pool, sql, entry).await;
            if let Err(e) = c.close().await { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("MySQL 物理连接清理失败：{e}"))), false); }
            outcome
        }, Err(e) => { let unsent = e.code == "EXECUTION_CANCELLED_BEFORE_SEND"; (Err(e.into()), unsent) } },
        Db::Postgres(pool) => match acquire(pool, entry).await { Ok(mut c) => {
            c.close_on_drop(); let outcome = pg_run(&mut c, pool, sql, entry, false).await;
            if let Err(e) = c.close().await { return (Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("PostgreSQL 物理连接清理失败：{e}"))), false); }
            outcome
        }, Err(e) => { let unsent = e.code == "EXECUTION_CANCELLED_BEFORE_SEND"; (Err(e.into()), unsent) } },
        _ => return (Err(AppError::coded("EXECUTION_ADAPTER_REQUIRED", "该适配器使用统一 command 服务内的真实 legacy 路线")), false),
    };
    (result.0.map_err(AppError::from), result.1)

    }).await
}
