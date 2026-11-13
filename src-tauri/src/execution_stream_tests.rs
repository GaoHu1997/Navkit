use super::*;
use crate::{execution::{Binding, Registry}, execution_contracts::{ExecutionRequest, ExecutionState}, models::{MultiResult, ResultSet}};
use std::{sync::atomic::{AtomicBool, Ordering}, time::Duration};

fn registered(page_rows: usize) -> (Registry, Arc<Entry>, Arc<StreamProgress>) {
    let registry = Registry::default();
    let binding = Binding { principal: "main".into(), owner: "tab".into(), connection: "c".into(), namespace: "db".into(), fingerprint: "test".into() };
    let mut req = ExecutionRequest { execution_id: String::new(), connection_id: "c".into(), namespace: "db".into(), owner_id: "tab".into(), sql: "SELECT test".into(), transaction_owner: None, timeout_ms: Some(0), origin: Some("query".into()), execution_grant: None, cancel_before_send: false };
    registry.grant_request(&mut req, binding.clone()).unwrap();
    let entry = registry.register(&req, binding, 0).unwrap();
    let progress = StreamProgress::new(StreamOptions { page_rows });
    entry.attach_stream(progress.clone());
    (registry, entry, progress)
}
fn result(spool_id: Option<String>, total: u64) -> MultiResult {
    MultiResult { sets: vec![ResultSet { columns: vec![], rows: vec![vec![Some("retained only in spool".into())]], affected: 0, elapsed_ms: 1, ok: true, message: String::new(), has_rows: true, primary_keys: vec![], total }], messages: vec![], elapsed_ms: 1, spool_id }
}
async fn settled(entry: &Entry, progress: &StreamProgress) -> StreamSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notified = progress.changed.notified();
            tokio::pin!(notified); notified.as_mut().enable();
            let snapshot = progress.snapshot(entry);
            if snapshot.outcome.is_some() { return snapshot; }
            notified.await;
        }
    }).await.expect("stream never settled")
}
struct OwnedResource(Arc<AtomicBool>);
impl Drop for OwnedResource { fn drop(&mut self) { self.0.store(true, Ordering::Release); } }

#[test]
fn page_options_reject_unbounded_or_zero() {
    assert!(StreamOptions { page_rows: 0 }.validate().is_err());
    assert!(StreamOptions { page_rows: 1001 }.validate().is_err());
    assert!(StreamOptions::default().validate().is_ok());
    assert!(serde_json::from_str::<StreamOptions>("{\"page_rows\":200,\"sql\":\"DROP\"}").is_err());
}
#[tokio::test]
async fn first_page_returns_before_tail_and_keeps_real_owner_and_export_gate() {
    let (registry, entry, progress) = registered(3);
    let activity = crate::activity::ActivityGate::default();
    entry.own_activity(activity.enter().unwrap());
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = OwnedResource(dropped.clone());
    let spool = crate::query_spool::create("c", "tab", "db", "main");
    entry.attach_spool(spool.clone());
    let set = spool.begin_set(0, &["n".into()], &["TEXT".into()]);
    let (release, tail) = tokio::sync::oneshot::channel();
    let worker_entry = entry.clone();
    let worker_set = set.clone();
    let worker_spool = spool.clone();
    launch(entry.clone(), async move {
        let _owner = resource;
        assert!(worker_entry.start());
        for n in 0..3 {
            let row = vec![Some(n.to_string())];
            worker_set.push(&row);
            worker_entry.observe_stream_row(0, &["n".into()], &["TEXT".into()], &row);
        }
        tail.await.unwrap();
        for n in 3..20 { worker_set.push(&[Some(n.to_string())]); }
        worker_spool.finish_all();
        worker_entry.work_settled();
        // Explicit ownership release before terminal receipt (physical cleanup).
        drop(_owner);
        Ok(worker_entry.finish(Ok(result(Some(worker_spool.id.clone()), 20)), false, false))
    });
    let first = progress.wait_first(&entry).await;
    assert_eq!(first.status.state, ExecutionState::Running);
    assert!(first.outcome.is_none());
    let preview = first.preview.unwrap();
    assert!(preview.provisional);
    assert_eq!(preview.rows.len(), 3);
    assert_eq!(preview.observed_rows, 3);
    assert!(registry.has_active("c"));
    assert!(activity.exclusive().is_err());
    assert!(!dropped.load(Ordering::Acquire));
    assert!(set.fetch(0, 3).is_err());
    assert!(set.ensure_complete().is_err());
    release.send(()).unwrap();
    let final_snapshot = settled(&entry, &progress).await;
    let outcome = final_snapshot.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded);
    assert!(!outcome.resources_pending);
    assert!(outcome.result.unwrap().sets[0].rows.is_empty(), "receipt must not retain full rows");
    assert!(final_snapshot.preview.is_none());
    assert!(!registry.has_active("c"));
    assert!(dropped.load(Ordering::Acquire));
    assert!(activity.exclusive().is_ok());
    set.ensure_complete().unwrap();
    assert_eq!(set.fetch(19, 200).unwrap(), vec![vec![Some("19".into())]]);
    crate::query_spool::drop_spool(&spool.id);
}
#[tokio::test]
async fn first_waiter_can_be_dropped_without_aborting_database_work() {
    let (_, entry, progress) = registered(1);
    let (release, tail) = tokio::sync::oneshot::channel();
    let worker = entry.clone();
    launch(entry.clone(), async move {
        worker.start();
        tail.await.unwrap();
        worker.work_settled();
        Ok(worker.finish(Ok(result(None, 1)), false, false))
    });
    let p = progress.clone(); let e = entry.clone();
    let waiter = tokio::spawn(async move { p.wait_first(&e).await });
    waiter.abort(); let _ = waiter.await;
    release.send(()).unwrap();
    assert_eq!(settled(&entry, &progress).await.outcome.unwrap().state, ExecutionState::Succeeded);
}
#[tokio::test]
async fn cancel_after_preview_revokes_rows_and_never_validates_spool() {
    let (_, entry, progress) = registered(1);
    entry.start();
    let spool = crate::query_spool::create("c", "tab", "db", "main");
    entry.attach_spool(spool.clone());
    let set = spool.begin_set(0, &["x".into()], &["TEXT".into()]);
    set.push(&[Some("first".into())]);
    entry.observe_stream_row(0, &["x".into()], &["TEXT".into()], &[Some("first".into())]);
    assert!(progress.snapshot(&entry).preview.is_some());
    entry.request_stop(false);
    assert!(progress.snapshot(&entry).preview.is_none());
    spool.finish_all();
    // Finished bytes alone must not make exports possible.
    assert!(set.ensure_complete().is_err());
    entry.work_settled();
    entry.finish(Err(AppError::coded("QUERY_INTERRUPTED", "controlled stop")), true, false);
    let snapshot = settled(&entry, &progress).await;
    assert_eq!(snapshot.outcome.unwrap().state, ExecutionState::Cancelled);
    assert!(snapshot.preview.is_none());
    assert!(!crate::query_spool::exists(&spool.id));
    assert!(set.ensure_complete().is_err(), "even an Arc pinned before revocation must be unusable");
}
#[tokio::test]
async fn background_panic_is_unknown_and_keeps_quarantine() {
    let (_, entry, progress) = registered(1);
    let working = entry.clone();
    launch(entry.clone(), async move {
        working.start();
        panic!("test worker panic");
        #[allow(unreachable_code)] Ok(working.finish(Ok(result(None, 0)), false, false))
    });
    let snapshot = settled(&entry, &progress).await;
    assert_eq!(snapshot.status.state, ExecutionState::Unknown);
    assert!(snapshot.status.resources_pending);
    assert!(snapshot.outcome.unwrap().error.is_some());
    entry.drained(); // Test-only proof that no synthetic resources remain.
    assert!(!progress.snapshot(&entry).outcome.unwrap().resources_pending);
}
#[tokio::test]
async fn late_drain_cannot_overwrite_unknown_receipt() {
    let (_, entry, progress) = registered(1);
    entry.start();
    let (release, tail) = tokio::sync::oneshot::channel();
    let worker = entry.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let resource = OwnedResource(dropped.clone());
    let task = crate::sql_history::spawn(async move {
        let (r, confirmed) = crate::execution::supervise_with_grace(worker.clone(), async move {
            let _owner = resource;
            worker.work_settled();
            tail.await.unwrap();
            (Ok(result(None, 0)), false)
        }, Duration::from_millis(5)).await;
        (r, confirmed)
    });
    let (r, confirmed) = task.await.unwrap();
    entry.finish(r, confirmed, false);
    assert_eq!(settled(&entry, &progress).await.outcome.unwrap().state, ExecutionState::Unknown);
    assert!(!dropped.load(Ordering::Acquire));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while entry.status().resources_pending { tokio::task::yield_now().await; }
    }).await.unwrap();
    let snapshot = progress.snapshot(&entry);
    assert_eq!(snapshot.outcome.unwrap().state, ExecutionState::Unknown);
    assert!(!snapshot.status.resources_pending);
    assert!(dropped.load(Ordering::Acquire));
}
#[test]
fn preview_limits_are_prefix_only_and_later_results_cannot_replace_it() {
    let (_, entry, progress) = registered(1000);
    entry.start();
    let names = ["x".into()]; let types = ["TEXT".into()];
    entry.observe_stream_row(1, &names, &types, &[Some("wrong set".into())]);
    assert!(progress.snapshot(&entry).preview.is_none());
    entry.observe_stream_row(0, &names, &types, &[Some("prefix".into())]);
    entry.observe_stream_row(0, &names, &types, &[Some("\u{0000}".repeat(STREAM_PREVIEW_BYTES / 6))]);
    let snapshot = progress.snapshot(&entry);
    let preview = snapshot.preview.unwrap();
    assert_eq!(preview.rows, vec![vec![Some("prefix".into())]]);
    assert!(serde_json::to_vec(&preview).unwrap().len() < STREAM_PREVIEW_BYTES);
    entry.observe_stream_row(0, &names, &types, &[Some("must not skip to a later small row".into())]);
    assert_eq!(progress.snapshot(&entry).preview.unwrap().rows.len(), 1);
    entry.finish(Ok(result(None, 3)), false, false);
}
#[test]
fn quick_completion_needs_no_prior_preview_poll() {
    let (_, entry, progress) = registered(200);
    entry.start(); entry.work_settled();
    entry.finish(Ok(result(None, 1)), false, false);
    let snapshot = progress.snapshot(&entry);
    assert_eq!(snapshot.status.state, ExecutionState::Succeeded);
    assert!(snapshot.outcome.is_some()); assert!(snapshot.preview.is_none());
}

#[tokio::test]
async fn slow_first_row_returns_pending_ticket_without_stopping_worker() {
    let (_, entry, progress) = registered(200);
    entry.start();
    let started = std::time::Instant::now();
    let snapshot = tokio::time::timeout(Duration::from_secs(4), progress.wait_first(&entry)).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(snapshot.status.state, ExecutionState::Running);
    assert!(snapshot.preview.is_none() && snapshot.outcome.is_none());
    assert!(!entry.stopping(), "the IPC wait cap is not a query timeout");
    entry.work_settled();
    entry.finish(Ok(result(None, 0)), false, false);
    assert_eq!(settled(&entry, &progress).await.outcome.unwrap().state, ExecutionState::Succeeded);
}

#[tokio::test]
async fn first_row_waiter_observes_cancel_request_before_database_settles() {
    let (_, entry, progress) = registered(200);
    entry.start();
    let worker = entry.clone();
    let request = tokio::spawn(async move { tokio::task::yield_now().await; worker.request_stop(false); });
    let snapshot = tokio::time::timeout(Duration::from_millis(500), progress.wait_first(&entry)).await.unwrap();
    request.await.unwrap();
    assert_eq!(snapshot.status.state, ExecutionState::CancelRequested);
    assert!(snapshot.preview.is_none() && snapshot.outcome.is_none());
    entry.work_settled();
    entry.finish(Err(AppError::coded("INTERRUPTED", "confirmed by synthetic database")), true, false);
    assert_eq!(settled(&entry, &progress).await.outcome.unwrap().state, ExecutionState::Cancelled);
}
