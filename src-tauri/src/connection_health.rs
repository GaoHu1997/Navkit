//! Idle health probes: generation-scoped tickets, monotonic cadence, and owned leases until real drain.
use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::Serialize;
use std::{collections::HashMap, future::Future, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeepAlivePolicy {
    pub interval_secs: u64, pub timeout_secs: u64, pub failure_threshold: u32, pub enabled: bool,
    pub validation_error: Option<String>,
}
impl KeepAlivePolicy {
    pub fn try_parse(parameters: &str, enabled: bool) -> Result<Self> {
        let mut out = Self { interval_secs: 60, timeout_secs: 5, failure_threshold: 2, enabled, validation_error: None };
        let mut seen = HashMap::new();
        for (name, value) in crate::tls_options::params::parse(parameters)? {
            let key = match name.as_str() {
                "keepalive" | "keep_alive" => "enabled", "keepalive_interval" | "keepalive_interval_secs" => "interval",
                "keepalive_timeout" | "keepalive_timeout_secs" => "timeout", "keepalive_failures" => "failures",
                _ if name.starts_with("keepalive") || name.starts_with("keep_alive") => bail!("KEEPALIVE_OPTION_UNSUPPORTED: 未支持的保活参数"), _ => continue,
            };
            let number = if key == "enabled" { u64::from(crate::tls_options::params::bool_value(&value, &name)?) }
                else { value.parse::<u64>().map_err(|_| anyhow::anyhow!("KEEPALIVE_OPTION_INVALID: 保活数值必须是正整数"))? };
            if seen.insert(key, number).map(|previous| previous != number).unwrap_or(false) { bail!("KEEPALIVE_OPTION_CONFLICT: 保活参数别名冲突"); }
            match key { "enabled" => out.enabled = number == 1, "interval" => out.interval_secs = number, "timeout" => out.timeout_secs = number,
                "failures" => out.failure_threshold = u32::try_from(number).map_err(|_| anyhow::anyhow!("KEEPALIVE_OPTION_INVALID: 连续失败次数超限"))?, _ => unreachable!() }
        }
        if !(10..=3600).contains(&out.interval_secs) || !(1..=60).contains(&out.timeout_secs) || out.timeout_secs > out.interval_secs || !(1..=10).contains(&out.failure_threshold) {
            bail!("KEEPALIVE_OPTION_INVALID: 间隔须10至3600秒、超时须1至60秒且不超过间隔、失败阈值须1至10");
        }
        Ok(out)
    }
    /// Compatibility entry: invalid configuration is DISABLED and diagnostic, never silently clamped.
    pub fn parse(parameters: &str, enabled: bool) -> Self {
        Self::try_parse(parameters, enabled).unwrap_or_else(|_| Self { interval_secs: 60, timeout_secs: 5, failure_threshold: 2, enabled: false, validation_error: Some("KEEPALIVE_OPTION_INVALID".into()) })
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionHealthView {
    pub connection_id: String, pub state: String, pub last_checked_at: Option<String>, pub last_error: Option<String>,
    pub probe_skipped_reason: Option<String>, pub interval_secs: u64, pub timeout_secs: u64, pub enabled: bool,
    pub generation: Option<String>, pub resources_pending: bool, pub next_due_in_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Generation { pub connection_id: String, pub fingerprint: String, pub generation: String }
struct Entry {
    binding: Generation, policy: KeepAlivePolicy, supported: bool, state: String, failures: u32,
    checked: Option<String>, error: Option<String>, skipped: Option<String>, last_started: Instant,
    attempt: Option<String>, pending: bool,
}
#[derive(Default, Clone)]
pub struct Registry { inner: Arc<Mutex<HashMap<String, Entry>>> }
pub struct ProbeTicket { registry: Arc<Mutex<HashMap<String, Entry>>>, binding: Generation, attempt: String, timeout: Duration, timed_out: Arc<AtomicBool> }
pub fn global() -> &'static Registry { static HEALTH: std::sync::OnceLock<Registry> = std::sync::OnceLock::new(); HEALTH.get_or_init(Registry::default) }
impl Registry {
    pub fn register_connection(&self, id: &str, fingerprint: &str, policy: KeepAlivePolicy, supported: bool) -> Result<Generation> {
        self.register_at(id, fingerprint, policy, supported, Instant::now())
    }
    fn register_at(&self, id: &str, fingerprint: &str, policy: KeepAlivePolicy, supported: bool, now: Instant) -> Result<Generation> {
        if id.is_empty() || id.len() > 128 || fingerprint.len() != 64 || !fingerprint.bytes().all(|b| b.is_ascii_hexdigit()) { bail!("KEEPALIVE_BINDING_INVALID: 连接ID或端点指纹无效"); }
        if policy.validation_error.is_some() { bail!("KEEPALIVE_OPTION_INVALID: 无效策略不能注册探测"); }
        let binding = Generation { connection_id: id.into(), fingerprint: fingerprint.into(), generation: uuid::Uuid::new_v4().to_string() };
        self.inner.lock().insert(id.into(), Entry { binding: binding.clone(), policy, supported, state: "unknown".into(), failures: 0, checked: None, error: None, skipped: None, last_started: now, attempt: None, pending: false });
        Ok(binding)
    }
    /// Call only AFTER a nonblocking ordinary lease and quiescent check. This never waits or queues a probe.
    pub fn try_begin(&self, binding: &Generation, principal: &str, owner: &str) -> Result<Option<ProbeTicket>> { self.try_begin_at(binding, principal, owner, Instant::now()) }
    fn try_begin_at(&self, binding: &Generation, principal: &str, owner: &str, now: Instant) -> Result<Option<ProbeTicket>> {
        if principal.is_empty() || principal.len() > 128 || owner.is_empty() || owner.len() > 128 { bail!("KEEPALIVE_OWNER_INVALID: 探测必须绑定真实窗口和调用owner"); }
        let mut map = self.inner.lock(); let entry = map.get_mut(&binding.connection_id).ok_or_else(|| anyhow::anyhow!("KEEPALIVE_CONNECTION_CLOSED"))?;
        if &entry.binding != binding { bail!("KEEPALIVE_GENERATION_CHANGED: 连接代次或端点已变化"); }
        let reason = if !entry.policy.enabled { Some("disabled") } else if !entry.supported { Some("unsupported") }
            else if entry.attempt.is_some() { Some("probe_resources_pending") } else if now.saturating_duration_since(entry.last_started) < Duration::from_secs(entry.policy.interval_secs) { Some("interval_not_due") } else { None };
        if let Some(reason) = reason { entry.skipped = Some(reason.into()); return Ok(None); }
        let attempt = uuid::Uuid::new_v4().to_string(); entry.last_started = now; entry.attempt = Some(attempt.clone()); entry.skipped = None;
        Ok(Some(ProbeTicket { registry: self.inner.clone(), binding: binding.clone(), attempt, timeout: Duration::from_secs(entry.policy.timeout_secs), timed_out: Arc::new(AtomicBool::new(false)) }))
    }
    pub fn record_busy(&self, binding: &Generation, reason: &str) {
        let mut map = self.inner.lock(); if let Some(entry) = map.get_mut(&binding.connection_id) { if &entry.binding == binding { entry.skipped = Some(match reason { "manual_transaction" => "manual_transaction", "execution_pending" => "execution_pending", _ => "connection_busy" }.into()); } }
    }
    pub fn view_generation(&self, binding: &Generation) -> Result<ConnectionHealthView> {
        let map = self.inner.lock(); let entry = map.get(&binding.connection_id).ok_or_else(|| anyhow::anyhow!("KEEPALIVE_CONNECTION_CLOSED"))?;
        if &entry.binding != binding { bail!("KEEPALIVE_GENERATION_CHANGED"); } Ok(view_entry(entry, Instant::now()))
    }
    pub fn view(&self, id: &str, interval_secs: u64) -> ConnectionHealthView {
        self.inner.lock().get(id).map(|entry| view_entry(entry, Instant::now())).unwrap_or_else(|| ConnectionHealthView { connection_id: id.into(), state: "unknown".into(), last_checked_at: None, last_error: None, probe_skipped_reason: Some("connection_not_registered".into()), interval_secs, timeout_secs: 5, enabled: false, generation: None, resources_pending: false, next_due_in_ms: None })
    }
    pub fn clear_generation(&self, binding: &Generation) { let mut map = self.inner.lock(); if map.get(&binding.connection_id).map(|entry| &entry.binding == binding).unwrap_or(false) { map.remove(&binding.connection_id); } }
    pub fn clear(&self, id: &str) { self.inner.lock().remove(id); }
    /// Deprecated id-only callbacks cannot mutate a generation-bound health record.
    pub fn record_ok(&self, _id: &str, _interval_secs: u64) {}
    pub fn record_err(&self, _id: &str, _threshold: u32, _interval_secs: u64, _err: &str) {}
    pub fn record_skipped(&self, _id: &str, _reason: &str, _interval_secs: u64) {}
}
fn view_entry(entry: &Entry, now: Instant) -> ConnectionHealthView {
    let remaining = Duration::from_secs(entry.policy.interval_secs).saturating_sub(now.saturating_duration_since(entry.last_started));
    ConnectionHealthView { connection_id: entry.binding.connection_id.clone(), state: entry.state.clone(), last_checked_at: entry.checked.clone(), last_error: entry.error.clone(), probe_skipped_reason: entry.skipped.clone(), interval_secs: entry.policy.interval_secs, timeout_secs: entry.policy.timeout_secs, enabled: entry.policy.enabled && entry.supported, generation: Some(entry.binding.generation.clone()), resources_pending: entry.pending, next_due_in_ms: if entry.attempt.is_some() || !entry.policy.enabled || !entry.supported { None } else { Some(remaining.as_millis().min(u128::from(u64::MAX)) as u64) } }
}
impl ProbeTicket {
    fn update(&self, apply: impl FnOnce(&mut Entry)) { let mut map = self.registry.lock(); if let Some(entry) = map.get_mut(&self.binding.connection_id) { if entry.binding == self.binding && entry.attempt.as_deref() == Some(self.attempt.as_str()) { apply(entry); } } }
    fn failure(entry: &mut Entry, code: &str) { entry.failures = entry.failures.saturating_add(1); entry.state = if entry.failures >= entry.policy.failure_threshold { "offline" } else { "suspect" }.into(); entry.error = Some(code.into()); entry.checked = Some(chrono::Utc::now().to_rfc3339()); }
    fn timeout(&self) { self.timed_out.store(true, Ordering::Release); self.update(|entry| { Self::failure(entry, "KEEPALIVE_TIMEOUT"); entry.pending = true; entry.skipped = Some("probe_resources_pending".into()); }); }
    fn settle(&self, result: std::result::Result<(), ProbeFailure>, timed_out: bool) { self.update(|entry| {
        if !timed_out { match result { Ok(()) => { entry.state = "healthy".into(); entry.failures = 0; entry.error = None; entry.checked = Some(chrono::Utc::now().to_rfc3339()); }, Err(ProbeFailure::Busy) => entry.skipped = Some("pool_busy".into()), Err(ProbeFailure::Unsupported) => entry.skipped = Some("unsupported".into()), Err(ProbeFailure::Failed) => Self::failure(entry, "KEEPALIVE_PROBE_FAILED") } }
        entry.attempt = None; entry.pending = false;
    }); }
    fn view(&self) -> ConnectionHealthView { Registry { inner: self.registry.clone() }.view(&self.binding.connection_id, 60) }
}
#[derive(Clone, Copy)]
enum ProbeFailure { Busy, Unsupported, Failed }
/// Worker owns BOTH the lifecycle/transaction lease and the activity guard until physical work is settled.
/// The parent must acquire the lease with try-lock, never queue behind a query/manual transaction.
pub async fn probe_owned(db: Arc<crate::db::Db>, ticket: ProbeTicket, lease: crate::manual_transaction::OrdinaryLease, activity: tokio::sync::OwnedRwLockReadGuard<()>) -> ConnectionHealthView {
    let __sql_history_context = crate::sql_history::db_context(db.as_ref());
    crate::sql_history::scope(__sql_history_context, async {
    let timed_out = ticket.timed_out.clone();
    supervise_owned(ticket, (lease, activity), async move { probe_native(&db, &timed_out).await }).await

    }).await
}
async fn supervise_owned<L, F>(ticket: ProbeTicket, lease: L, work: F) -> ConnectionHealthView
where L: Send + 'static, F: Future<Output = std::result::Result<(), ProbeFailure>> + Send + 'static {
    let initial = ticket.view(); let (send, receive) = tokio::sync::oneshot::channel();
    crate::sql_history::spawn(async move {
        let _lease = lease; tokio::pin!(work);
        tokio::select! {
            result = &mut work => { ticket.settle(result, false); let _ = send.send(ticket.view()); }
            _ = tokio::time::sleep(ticket.timeout) => { ticket.timeout(); let _ = send.send(ticket.view()); let result = work.await; ticket.settle(result, true); }
        }
    });
    receive.await.unwrap_or(ConnectionHealthView { state: "unknown".into(), last_error: Some("KEEPALIVE_WORKER_UNKNOWN".into()), resources_pending: true, ..initial })
}
async fn probe_native(db: &crate::db::Db, timed_out: &AtomicBool) -> std::result::Result<(), ProbeFailure> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    use sqlx::Executor;
    // Taking and owning an available physical connection prevents queued pool acquisitions. It is never reused after a failed probe.
    macro_rules! query { ($pool:expr) => {{
        let mut connection = $pool.try_acquire().ok_or(ProbeFailure::Busy)?;
        let result = (&mut *connection).execute("SELECT 1").await;
        if result.is_err() || timed_out.load(Ordering::Acquire) { connection.close_on_drop(); }
        result.map(|_| ()).map_err(|_| ProbeFailure::Failed)
    }}; }
    match db { crate::db::Db::Mysql(pool) => query!(pool), crate::db::Db::Postgres(pool) => query!(pool), _ => Err(ProbeFailure::Unsupported) }

    }).await
}
/// Legacy unowned path is intentionally inert. SQLite/local files need no network keepalive.
pub async fn probe(_db: &crate::db::Db) -> Option<std::result::Result<(), String>> {
    let __sql_history_context = crate::sql_history::db_context(_db);
    crate::sql_history::scope(__sql_history_context, async { None 
    }).await
}
#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> KeepAlivePolicy { KeepAlivePolicy::try_parse("", true).unwrap() }
    fn register(registry: &Registry, now: Instant) -> Generation { registry.register_at("c1", &"a".repeat(64), policy(), true, now).unwrap() }
    fn begin(registry: &Registry, binding: &Generation, now: Instant) -> ProbeTicket { registry.try_begin_at(binding, "main", "health", now + Duration::from_secs(60)).unwrap().unwrap() }
    #[test] fn invalid_policy_is_diagnostic_not_clamped() { for params in ["keepalive_interval=1", "keepalive_timeout=0", "keepalive_failures=4294967296", "keepalive_timeout=x", "keepalive=typo", "keepalive_interval=60;keepalive_interval_secs=120"] { assert!(KeepAlivePolicy::try_parse(params, true).is_err()); let policy = KeepAlivePolicy::parse(params, true); assert!(!policy.enabled); assert!(policy.validation_error.is_some()); } }
    #[test] fn defaults_and_json_false_are_strict() { let value = policy(); assert_eq!((value.interval_secs, value.timeout_secs, value.failure_threshold), (60,5,2)); assert!(!KeepAlivePolicy::try_parse(r#"{"keep_alive":false,"keepalive_interval":120}"#, true).unwrap().enabled); }
    #[test] fn monotonic_policy_interval_is_observed() { let registry = Registry::default(); let now = Instant::now(); let binding = register(&registry, now); assert!(registry.try_begin_at(&binding, "main", "health", now + Duration::from_secs(59)).unwrap().is_none()); let ticket = begin(&registry, &binding, now); assert!(registry.try_begin_at(&binding, "main", "health", now + Duration::from_secs(120)).unwrap().is_none()); ticket.settle(Ok(()), false); assert_eq!(registry.view_generation(&binding).unwrap().state, "healthy"); }
    #[test] fn stale_generation_cannot_poison_reopened_connection() { let registry = Registry::default(); let now = Instant::now(); let old = register(&registry, now); let ticket = begin(&registry, &old, now); registry.clear_generation(&old); let reopened = register(&registry, now); ticket.timeout(); ticket.settle(Err(ProbeFailure::Failed), false); assert_ne!(old.generation, reopened.generation); assert_eq!(registry.view_generation(&reopened).unwrap().state, "unknown"); assert!(registry.try_begin_at(&old, "main", "health", now + Duration::from_secs(60)).is_err()); }
    #[test] fn timeout_does_not_enable_second_probe_before_real_drain() { let registry = Registry::default(); let now = Instant::now(); let binding = register(&registry, now); let ticket = begin(&registry, &binding, now); ticket.timeout(); assert!(registry.view_generation(&binding).unwrap().resources_pending); assert!(registry.try_begin_at(&binding, "main", "health", now + Duration::from_secs(3600)).unwrap().is_none()); ticket.settle(Ok(()), true); let view = registry.view_generation(&binding).unwrap(); assert!(!view.resources_pending); assert_eq!(view.last_error.as_deref(), Some("KEEPALIVE_TIMEOUT")); assert_ne!(view.state, "healthy"); }
    #[test] fn sqlite_and_unsupported_protocols_never_get_probe_tickets() { let registry = Registry::default(); let now = Instant::now(); let binding = registry.register_at("c1", &"a".repeat(64), policy(), false, now).unwrap(); assert!(registry.try_begin_at(&binding, "main", "health", now + Duration::from_secs(60)).unwrap().is_none()); assert_eq!(registry.view_generation(&binding).unwrap().probe_skipped_reason.as_deref(), Some("unsupported")); }
    #[test] fn failures_and_busy_have_different_meaning() { let registry = Registry::default(); let now = Instant::now(); let binding = register(&registry, now); begin(&registry, &binding, now).settle(Err(ProbeFailure::Busy), false); assert_eq!(registry.view_generation(&binding).unwrap().state, "unknown"); let ticket = registry.try_begin_at(&binding, "main", "health", now + Duration::from_secs(120)).unwrap().unwrap(); ticket.settle(Err(ProbeFailure::Failed), false); assert_eq!(registry.view_generation(&binding).unwrap().state, "suspect"); }
    #[test] fn id_only_legacy_callbacks_cannot_overwrite_managed_generation() { let registry = Registry::default(); let binding = register(&registry, Instant::now()); registry.record_ok("c1",60); registry.record_err("c1",1,60,"PASSWORD"); assert_eq!(registry.view_generation(&binding).unwrap().state, "unknown"); }
    #[tokio::test] async fn deadline_returns_but_lease_lives_until_worker_settles() { use std::sync::atomic::{AtomicBool,Ordering}; struct Lease(Arc<AtomicBool>); impl Drop for Lease { fn drop(&mut self) { self.0.store(true,Ordering::Release); } } let registry = Registry::default(); let now = Instant::now(); let binding = register(&registry, now); let mut ticket = begin(&registry, &binding, now); ticket.timeout = Duration::from_millis(1); let dropped = Arc::new(AtomicBool::new(false)); let (send,receive) = tokio::sync::oneshot::channel(); let view = supervise_owned(ticket, Lease(dropped.clone()), async move { let _ = receive.await; Ok(()) }).await; assert!(view.resources_pending); assert!(!dropped.load(Ordering::Acquire)); send.send(()).unwrap(); for _ in 0..100 { if dropped.load(Ordering::Acquire) { break; } tokio::task::yield_now().await; } assert!(dropped.load(Ordering::Acquire)); assert!(!registry.view_generation(&binding).unwrap().resources_pending); }
    #[tokio::test] async fn timed_out_worker_observes_quarantine_even_after_late_success() { let registry = Registry::default(); let now = Instant::now(); let binding = register(&registry, now); let mut ticket = begin(&registry, &binding, now); ticket.timeout = Duration::from_millis(1); let flag = ticket.timed_out.clone(); let (send, receive) = tokio::sync::oneshot::channel(); let (observed, result) = tokio::sync::oneshot::channel(); let view = supervise_owned(ticket, (), async move { let _ = receive.await; let _ = observed.send(flag.load(Ordering::Acquire)); Ok(()) }).await; assert!(view.resources_pending); send.send(()).unwrap(); assert!(result.await.unwrap()); }
    #[tokio::test] async fn sqlite_legacy_probe_does_not_send_sql() { let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap(); assert!(probe(&crate::db::Db::Sqlite(pool.clone())).await.is_none()); pool.close().await; }
}
