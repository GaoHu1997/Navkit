//! Opt-in observation of the existing C1 worker, not a second SQL executor.
//! Preview is provisional; only Entry::finish may publish the final receipt.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use crate::{error::{AppError, CmdResult}, execution::Entry, execution_contracts::{ExecutionOutcome, ExecutionStatus}, models::{ResultColumn, ResultSet, MultiResult}};

pub const STREAM_PREVIEW_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOptions {
    #[serde(default = "default_page_rows")]
    pub page_rows: usize,
}
fn default_page_rows() -> usize { 200 }
impl Default for StreamOptions { fn default() -> Self { Self { page_rows: default_page_rows() } } }
impl StreamOptions {
    pub(crate) fn validate(&self) -> CmdResult<()> {
        if !(1..=1000).contains(&self.page_rows) {
            return Err(AppError::coded("STREAM_PAGE_INVALID", "流式首屏行数必须为 1..1000；没有发送 SQL"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamPreview {
    pub result_index: usize,
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Option<String>>>,
    /// This is a lower bound, NOT the final row count of the query.
    pub observed_rows: u64,
    pub elapsed_ms: u64,
    pub spool_id: Option<String>,
    pub provisional: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct StreamSnapshot {
    pub status: ExecutionStatus,
    pub preview: Option<StreamPreview>,
    pub outcome: Option<ExecutionOutcome>,
}
#[derive(Default)]
struct Inner {
    building: Option<StreamPreview>,
    preview: Option<StreamPreview>,
    bytes: usize,
    outcome: Option<ExecutionOutcome>,
}
pub(crate) struct StreamProgress {
    pub options: StreamOptions,
    inner: Mutex<Inner>,
    changed: Notify,
    published: AtomicBool,
}
impl StreamProgress {
    pub fn new(options: StreamOptions) -> Arc<Self> {
        Arc::new(Self { options, inner: Mutex::new(Inner::default()), changed: Notify::new(), published: AtomicBool::new(false) })
    }
    /// Observe only the first result. Copies at most a single bounded page;
    /// never affects the SQL stream, cancellation, or spool completeness.
    pub fn observe(&self, entry: &Entry, index: usize, names: &[String], types: &[String], row: &[Option<String>]) {
        if index != 0 || self.published.load(Ordering::Acquire) || entry.stopping() { return; }
        let elapsed_ms = entry.status().deadline.elapsed_ms;
        let spool_id = entry.spool().map(|s| s.id.clone());
        let mut inner = self.inner.lock();
        if inner.preview.is_some() || inner.outcome.is_some() { return; }
        if inner.building.is_none() {
            // Worst-case JSON escaping + Rust capacities. No unbounded temporary JSON.
            let metadata_bytes = names.iter().chain(types).fold(256usize, |n, s| n.saturating_add(s.len().saturating_mul(6)).saturating_add(128));
            if metadata_bytes > STREAM_PREVIEW_BYTES { return; }
            inner.bytes = metadata_bytes;
            inner.building = Some(StreamPreview {
                result_index: index,
                columns: names.iter().zip(types).map(|(name, data_type)| ResultColumn {
                    name: name.clone(), data_type: data_type.clone(), nullable: true, primary_key: false, auto_increment: false,
                }).collect(),
                rows: Vec::new(), observed_rows: 0, elapsed_ms: 0,
                spool_id, provisional: true,
            });
        }
        let bytes = row.iter().fold(std::mem::size_of::<Vec<Option<String>>>(), |n, cell| {
            n.saturating_add(std::mem::size_of::<Option<String>>() + 8)
                .saturating_add(cell.as_ref().map_or(4, |s| s.capacity().saturating_add(s.len().saturating_mul(6))))
        });
        let fits = inner.bytes.saturating_add(bytes) <= STREAM_PREVIEW_BYTES;
        if fits { inner.bytes += bytes; }
        let building = inner.building.as_mut().expect("initialized");
        building.observed_rows += 1;
        if fits { building.rows.push(row.to_vec()); }
        if !fits || building.rows.len() >= self.options.page_rows {
            building.elapsed_ms = elapsed_ms;
            inner.preview = inner.building.take();
            self.published.store(true, Ordering::Release);
            self.changed.notify_waiters();
        }
    }
    /// Called under Entry's terminal-state lock. Failure revokes the provisional
    /// preview; a late completion can never turn Unknown into Succeeded.
    pub fn complete(&self, outcome: &ExecutionOutcome) {
        let mut inner = self.inner.lock();
        inner.building = None;
        inner.preview = None;
        // Construct metadata only: do NOT clone all rows and then discard them.
        let receipt = ExecutionOutcome {
            execution_id: outcome.execution_id.clone(), state: outcome.state,
            error: outcome.error.clone(), deadline: outcome.deadline.clone(),
            effects: outcome.effects.clone(), transaction_state: outcome.transaction_state.clone(),
            resources_pending: outcome.resources_pending,
            result: outcome.result.as_ref().map(|r| MultiResult {
                sets: r.sets.iter().map(|s| ResultSet {
                    columns: s.columns.clone(), rows: Vec::new(), affected: s.affected,
                    elapsed_ms: s.elapsed_ms, ok: s.ok, message: s.message.clone(),
                    has_rows: s.has_rows, primary_keys: s.primary_keys.clone(), total: s.total,
                }).collect(), messages: r.messages.clone(), elapsed_ms: r.elapsed_ms, spool_id: r.spool_id.clone(),
            }),
        };
        inner.outcome = Some(receipt);
        self.published.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    pub fn snapshot(&self, entry: &Entry) -> StreamSnapshot {
        // Clone bounded progress, release its lock, then read the actual Entry.
        // In particular, do not overwrite a newly quarantined receipt with a
        // pre-quarantine resources_pending flag read before publication.
        let (mut preview, mut outcome) = {
            let inner = self.inner.lock();
            (inner.preview.clone(), inner.outcome.clone())
        };
        let mut status = entry.status();
        if outcome.is_none() && status.state.terminal() {
            // finish publishes the receipt under Entry's terminal lock. If it
            // completed between the two reads, fetch that receipt now.
            outcome = self.inner.lock().outcome.clone();
        }
        if let Some(receipt) = &mut outcome {
            receipt.resources_pending = status.resources_pending;
            status.state = receipt.state;
            status.deadline = receipt.deadline.clone();
        }
        if outcome.is_some() || status.state.terminal()
            || matches!(status.state, crate::execution_contracts::ExecutionState::CancelRequested) {
            preview = None;
        }
        StreamSnapshot { status, preview, outcome }
    }
    pub async fn wait_first(&self, entry: &Entry) -> StreamSnapshot {
        // A slow first row must not hold an IPC open indefinitely. The same
        // worker continues; callers poll the authenticated snapshot endpoint.
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
        tokio::pin!(deadline);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let snapshot = self.snapshot(entry);
            if snapshot.preview.is_some() || snapshot.outcome.is_some() || snapshot.status.state.terminal()
                || matches!(snapshot.status.state, crate::execution_contracts::ExecutionState::CancelRequested) { return snapshot; }
            tokio::select! {
                _ = notified => {},
                _ = entry.wait_stop() => return self.snapshot(entry),
                _ = &mut deadline => return self.snapshot(entry),
            }
        }
    }
}

/// This owner survives a dropped IPC future. Keep the JoinHandle until it really
/// settles; panics are reported as Unknown with quarantined resources, not success.
pub(crate) fn launch<F>(entry: Arc<Entry>, work: F)
where F: std::future::Future<Output = CmdResult<ExecutionOutcome>> + Send + 'static {
    let worker = crate::sql_history::spawn(work);
    crate::sql_history::spawn(async move {
        let error = match worker.await {
            Ok(Ok(_)) => return,
            Ok(Err(error)) => error,
            Err(error) => AppError::coded("EXECUTION_EFFECTS_UNKNOWN", format!("后台查询任务异常结束，资源保持隔离：{error}")),
        };
        let progress = entry.stream();
        if progress.as_ref().is_some_and(|p| p.inner.lock().outcome.is_some()) { return; }
        entry.quarantine();
        if !entry.status().state.terminal() {
            // An unexpected failure cannot prove that a manual transaction was
            // absent; report unknown transaction state conservatively.
            entry.finish(Err(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", error.to_string())), false, true);
        } else if let Some(progress) = progress {
            // A panic during terminal publication must not leave waiters without
            // a receipt; never call finish twice or fabricate a late success.
            let status = entry.status();
            progress.complete(&ExecutionOutcome {
                execution_id: entry.id.clone(), state: crate::execution_contracts::ExecutionState::Unknown,
                result: None, error: Some(AppError::coded("EXECUTION_EFFECTS_UNKNOWN", error.to_string()).operation(&entry.id)),
                deadline: status.deadline, effects: "possibly_applied".into(),
                transaction_state: "unknown".into(), resources_pending: true,
            });
        }
    });
}

#[cfg(test)]
#[path = "execution_stream_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "execution_stream_mysql_tests.rs"]
mod mysql_tests;
