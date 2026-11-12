//! SQLite lineage capture inside C1. This module never grants or replays SQL.
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use crate::execution::Entry;
use crate::execution::Binding;
use crate::models::{MultiResult, ResultSet};
use crate::query_lineage::{CaptureAttempt, CaptureLimits, CapturedSelect};
use crate::query_result_store::{RegisteredResult, ResultBinding, ResultStore};

/// Current page generations are service state, never reconstructed from IPC fields.
#[derive(Default)]
pub(crate) struct QueryPages {
    pub store: ResultStore,
    current: Mutex<HashMap<(String, String), PageGeneration>>,
}
struct PageGeneration { execution: String, connection: String, writing: bool }
pub(crate) struct QueryWriteGuard { pages: Arc<QueryPages>, binding: ResultBinding }
impl Drop for QueryWriteGuard {
    fn drop(&mut self) {
        if let Some(page) = self.pages.current.lock().get_mut(&(self.binding.principal.clone(), self.binding.owner_id.clone())) {
            if page.execution == self.binding.source_execution_id { page.writing = false; }
        }
    }
}
impl QueryPages {
    /// Register and replace a page atomically with publication/writes. A rejected grant leaves it intact.
    pub(crate) fn register_execution<T>(&self, binding: &Binding, id: &str, register: impl FnOnce() -> crate::error::CmdResult<T>) -> crate::error::CmdResult<T> {
        let mut current = self.current.lock();
        let key = (binding.principal.clone(), binding.owner.clone());
        if current.get(&key).is_some_and(|page| page.writing) {
            return Err(crate::error::AppError::coded("QUERY_MUTATION_IN_PROGRESS", "此页面的行写入尚未结束，不能替换来源查询"));
        }
        let registered = register()?;
        // Bounded page identities; eviction only removes edit capability, never active writes.
        if current.len() >= 4096 {
            current.retain(|(principal, owner), page| {
                if page.writing { true } else { self.store.clear_owner(principal, owner); false }
            });
        }
        self.store.clear_owner(&binding.principal, &binding.owner);
        current.insert(key, PageGeneration { execution: id.into(), connection: binding.connection.clone(), writing: false });
        Ok(registered)
    }
    pub(crate) fn require_current(&self, binding: &ResultBinding) -> Result<()> {
        if !self.current.lock().get(&(binding.principal.clone(), binding.owner_id.clone()))
            .is_some_and(|page| page.execution == binding.source_execution_id && page.connection == binding.connection_id) {
            return Err(anyhow!("QUERY_CONTEXT_CHANGED：页面已有新执行或已释放，请重新执行查询"));
        }
        Ok(())
    }
    pub(crate) fn pin_write(self: &Arc<Self>, binding: &ResultBinding) -> Result<QueryWriteGuard> {
        let mut current = self.current.lock();
        let page = current.get_mut(&(binding.principal.clone(), binding.owner_id.clone()))
            .filter(|page| page.execution == binding.source_execution_id && page.connection == binding.connection_id)
            .ok_or_else(|| anyhow!("QUERY_CONTEXT_CHANGED：页面已替换或释放，没有发送 DML"))?;
        if page.writing { return Err(anyhow!("QUERY_MUTATION_IN_PROGRESS：同一页面已有行写入")); }
        page.writing = true;
        Ok(QueryWriteGuard { pages: self.clone(), binding: binding.clone() })
    }
    pub(crate) fn release_execution(&self, binding: &Binding, id: &str) -> Result<()> {
        let mut current = self.current.lock();
        let key = (binding.principal.clone(), binding.owner.clone());
        if current.get(&key).is_some_and(|page| page.execution == id && page.connection == binding.connection) {
            if current.get(&key).is_some_and(|page| page.writing) { return Err(anyhow!("QUERY_MUTATION_IN_PROGRESS：行写入仍在执行，请保留页面草稿直到回执到达")); }
            current.remove(&key); self.store.clear_owner(&binding.principal, &binding.owner);
        }
        Ok(())
    }
    pub(crate) fn clear_connection(&self, id: &str) {
        let mut current = self.current.lock(); current.retain(|_, page| page.connection != id);
        self.store.clear_connection(id);
    }
}

#[derive(Clone)]
pub(crate) struct QuerySlot {
    pub binding: ResultBinding,
    pub result_token: Option<String>,
    pub total_rows: u64,
    pub loaded_rows: usize,
    pub read_only_reason: String,
}
pub(crate) struct QueryCapture {
    pub pages: Arc<QueryPages>,
    binding: Binding,
    execution_id: String,
    released: AtomicBool,
    slot: Mutex<Option<QuerySlot>>,
}
impl QueryCapture {
    pub(crate) fn new(pages: Arc<QueryPages>, binding: Binding, execution_id: String) -> Arc<Self> {
        Arc::new(Self { pages, binding, execution_id, released: AtomicBool::new(false), slot: Mutex::new(None) })
    }
    pub(crate) fn slot(&self, index: usize) -> Result<QuerySlot> {
        if self.released.load(Ordering::Acquire) { return Err(anyhow!("QUERY_RESULT_RELEASED：页面来源证据已释放，请重新执行")); }
        self.slot.lock().as_ref().filter(|slot| slot.binding.result_index == index).cloned()
            .ok_or_else(|| anyhow!("QUERY_RESULT_READ_ONLY：本次执行未认证单表来源，结果保持只读"))
    }
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Some(slot) = self.slot.lock().take() {
            if let Some(token) = &slot.result_token { let _ = self.pages.store.invalidate(token, &slot.binding); }
        }
    }
    fn publish(&self, capture: &CapturedSelect) {
        if self.released.load(Ordering::Acquire) { return; }
        let binding = ResultBinding { principal: self.binding.principal.clone(), owner_id: self.binding.owner.clone(),
            connection_id: self.binding.connection.clone(), endpoint_fingerprint: self.binding.fingerprint.clone(),
            source_execution_id: self.execution_id.clone(), result_index: 0, generation: uuid::Uuid::new_v4().to_string() };
        // Serialize publication with begin_execution: a late old query cannot evict a newer page generation.
        let current_page = self.pages.current.lock();
        if !current_page.get(&(binding.principal.clone(), binding.owner_id.clone()))
            .is_some_and(|page| page.execution == binding.source_execution_id && page.connection == binding.connection_id && !page.writing) { return; }
        let registered = self.pages.store.register(binding.clone(), capture).unwrap_or_else(|_| RegisteredResult {
            editability: crate::query_result_store::readonly(&binding, "查询来源证据未通过服务端校验，结果保持只读"),
            rows: Vec::new(), loaded_rows: 0, total_rows: capture.total,
        });
        let slot = QuerySlot { binding, result_token: registered.editability.result_token,
            total_rows: registered.total_rows, loaded_rows: registered.loaded_rows,
            read_only_reason: registered.editability.reason.chars().take(1024).collect() };
        let mut current = self.slot.lock();
        if self.released.load(Ordering::Acquire) {
            if let Some(token) = &slot.result_token { let _ = self.pages.store.invalidate(token, &slot.binding); }
        } else { *current = Some(slot); }
    }
}

/// NotEligible is the only branch allowed to call the old C1 reader. Executed/Err never retries.
pub(crate) async fn sqlite_batch(conn: &mut sqlx::SqliteConnection, sql: &str, entry: &Arc<Entry>) -> Result<MultiResult> {
    let Some(context) = entry.query_capture() else { return crate::execution::sqlite_batch(conn, sql, entry).await; };
    let start = std::time::Instant::now();
    let cache = entry.spool();
    let mut target: Option<Arc<crate::query_spool::SpoolSet>> = None;
    let attempt = crate::query_lineage::capture_sqlite_select(conn, sql, CaptureLimits::default(), |names, types, values| {
        if entry.stopping() { return Err(anyhow!("执行已请求停止，未发布查询来源证据")); }
        if target.is_none() { target = cache.as_ref().map(|cache| cache.begin_set(0, names, types)); }
        if let Some(target) = &target { target.push(values); }
        Ok(())
    }).await?;
    let capture = match attempt {
        CaptureAttempt::NotEligible(_) => return crate::execution::sqlite_batch(conn, sql, entry).await,
        CaptureAttempt::Executed(capture) => capture,
    };
    // Empty SELECTs still have genuine columns; do not execute another query to discover them.
    if target.is_none() { target = cache.as_ref().map(|cache| cache.begin_set(0, &capture.names, &capture.types)); }
    if let Some(target) = &target { target.finish(); }
    let message = if target.as_ref().is_some_and(|set| set.truncated()) {
        "本次查询缓存不完整，完整导出已禁用；不会重新执行 SQL。".to_string()
    } else if capture.total > capture.display_rows.len() as u64 {
        format!("查询已完成，共 {} 行；当前显示前 {} 行，分页与导出使用本次缓存。", capture.total, capture.display_rows.len())
    } else { String::new() };
    context.publish(&capture);
    let columns = capture.names.iter().zip(&capture.types).map(|(name, kind)| crate::models::ResultColumn {
        name: name.clone(), data_type: kind.clone(), nullable: true, primary_key: false, auto_increment: false,
    }).collect();
    let mut result = MultiResult { sets: vec![ResultSet { columns, rows: capture.display_rows, affected: 0,
        elapsed_ms: start.elapsed().as_millis() as u64, ok: true, message: message.clone(), has_rows: true,
        primary_keys: Vec::new(), total: capture.total }], messages: if message.is_empty() { Vec::new() } else { vec![message] },
        elapsed_ms: start.elapsed().as_millis() as u64, spool_id: None };
    if let Some(cache) = cache { cache.finish_all(); result.spool_id = Some(cache.id.clone()); }
    Ok(result)
}
