//! Full command-service parity using the existing isolated SQLite fixture.
use super::*;
use crate::execution_stream::{StreamOptions, StreamSnapshot};

fn begin(f: &Fixture, request: ExecutionRequest) -> Arc<crate::execution::Entry> {
    let registered = crate::commands::register_streaming_execution(&f.state, &request, "main", StreamOptions::default()).unwrap();
    let entry = registered.2.clone();
    let state = f.state.clone();
    crate::execution_stream::launch(entry.clone(), async move {
        crate::commands::execute_registered_sql_service(&state, request, "main", registered).await
    });
    entry
}
async fn wait_final(entry: &Arc<crate::execution::Entry>) -> StreamSnapshot {
    let progress = entry.stream().unwrap();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let snapshot = progress.snapshot(entry);
            if snapshot.outcome.is_some() { return snapshot; }
            tokio::task::yield_now().await;
        }
    }).await.expect("background SQL did not settle")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sqlite_lineage_first_page_and_same_execution_full_export() {
    let f = Fixture::new(false).await;
    sqlx::query("CREATE TABLE streamed(id INTEGER PRIMARY KEY, text TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<50000) INSERT INTO streamed SELECT x, '中文' FROM n").execute(&f.observer).await.unwrap();
    let request = f.request("SELECT id,text FROM streamed ORDER BY id", 0);
    let id = request.execution_id.clone();
    let started = std::time::Instant::now();
    let entry = begin(&f, request);
    let first = entry.stream().unwrap().wait_first(&entry).await;
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first.status.state, ExecutionState::Running);
    let preview = first.preview.expect("SQLite lineage path must emit a first page");
    assert!(preview.provisional);
    assert_eq!(preview.rows.len(), 200);
    assert_eq!(preview.rows[0], vec![Some("1".into()), Some("中文".into())]);
    assert_eq!(preview.columns[0].name, "id");
    assert!(f.state.executions.has_active("c1"));
    let final_snapshot = wait_final(&entry).await;
    let full_ms = started.elapsed().as_secs_f64() * 1000.0;
    let outcome = final_snapshot.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    assert!(!outcome.resources_pending);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].total, 50000);
    assert!(result.sets[0].rows.is_empty(), "poll receipts contain metadata, not a permanent full row cache");
    let spool_id = result.spool_id.unwrap();
    let spool = crate::query_spool::authorize(&spool_id, "c1", "tab1", "main", "main").unwrap();
    let set = spool.set(0).unwrap();
    set.ensure_complete().unwrap();
    assert_eq!(set.fetch(49999, 200).unwrap()[0][0].as_deref(), Some("50000"));
    sqlx::query("DELETE FROM streamed").execute(&f.observer).await.unwrap();
    let path = f.root.join("streamed-all.csv");
    let opts = ExportOptions { connection_id: "c1".into(), schema: "main".into(), table: "streamed".into(), file: path.to_string_lossy().into(), format: "csv".into(), has_header: true, delimiter: ",".into(), encoding: "utf-8".into(), where_clause: None, limit: None };
    let exported = crate::commands::export_query_spooled_service(&f.state, opts, &spool_id, "tab1", "main", 0, "main").await.unwrap();
    assert_eq!(exported, 50000);
    crate::sql_history::flush();
    let text = std::fs::read_to_string(f.root.join("logs/sql_log.jsonl")).unwrap();
    let logs: Vec<serde_json::Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let user_query = logs.iter().filter(|log| log["operation_id"] == id && log["sql"] == "SELECT id,text FROM streamed ORDER BY id" && log["origin"] == "query").collect::<Vec<_>>();
    assert_eq!(user_query.len(), 1, "streaming must execute and log original SQL once");
    eprintln!("streaming_sqlite_lineage rows=50000 first_page_ms={first_ms:.2} full_finish_ms={full_ms:.2} exported={exported}");
    crate::query_spool::drop_spool(&spool_id);
    drop(set); drop(spool); drop(entry);
    f.cleanup().await;
}

#[tokio::test]
async fn quick_query_and_write_have_authoritative_final_metadata() {
    let f = Fixture::new(false).await;
    let entry = begin(&f, f.request("SELECT 42", 0));
    let outcome = wait_final(&entry).await.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].total, 1);
    let id = result.spool_id.unwrap();
    assert_eq!(crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap().set(0).unwrap().fetch(0, 200).unwrap(), vec![vec![Some("42".into())]]);
    crate::query_spool::drop_spool(&id);
    let write = begin(&f, f.request("INSERT INTO items VALUES(2,'stream write')", 0));
    let snapshot = wait_final(&write).await;
    assert!(snapshot.preview.is_none());
    let outcome = snapshot.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].affected, 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 2);
    if let Some(id) = result.spool_id { crate::query_spool::drop_spool(&id); }
    f.cleanup().await;
}

#[tokio::test]
async fn streaming_snapshot_rejects_foreign_binding_replay_and_config_change() {
    let f = Fixture::new(false).await;
    let request = f.request("SELECT 1", 0);
    let duplicate = request.clone();
    let entry = begin(&f, request);
    let _ = wait_final(&entry).await;
    assert!(crate::commands::register_streaming_execution(&f.state, &duplicate, "main", StreamOptions::default()).is_err());
    for (principal, owner, namespace) in [("other-window", "tab1", "main"), ("main", "other-owner", "main"), ("main", "tab1", "other-schema")] {
        assert!(crate::commands::stream_snapshot_service(&f.state, principal, &entry.id, "c1", owner, namespace).is_err());
    }
    assert!(crate::commands::stream_snapshot_service(&f.state, "main", "not-a-uuid", "c1", "tab1", "main").is_err());
    assert!(crate::commands::stream_snapshot_service(&f.state, "main", &entry.id, "c1", "tab1", "main").unwrap().outcome.is_some());
    let mut cfg = f.state.store.connections(); cfg[0].name = "changed endpoint version".into();
    // Host is included in the physical binding fingerprint even for this fixture.
    cfg[0].host = "changed".into();
    f.state.store.save_connections(cfg, vec![]).unwrap();
    assert!(crate::commands::stream_snapshot_service(&f.state, "main", &entry.id, "c1", "tab1", "main").is_err());
    f.cleanup().await;
}

#[tokio::test]
async fn late_sql_error_revokes_preview_and_export_instead_of_succeeding() {
    let f = Fixture::new(false).await;
    let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12005) SELECT x FROM n; SELECT * FROM no_such_stream_table";
    let entry = begin(&f, f.request(sql, 0));
    let snapshot = wait_final(&entry).await;
    assert_eq!(snapshot.outcome.unwrap().state, ExecutionState::Failed);
    assert!(snapshot.preview.is_none());
    assert!(!f.state.executions.has_active("c1"));
    assert_eq!(f.execute("SELECT 42", 0).await.state, ExecutionState::Succeeded);
    f.cleanup().await;
}

#[tokio::test]
async fn readonly_and_manual_transactions_keep_existing_safety() {
    let f = Fixture::new(true).await;
    let entry = begin(&f, f.request("SELECT text FROM items", 0));
    let outcome = wait_final(&entry).await.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    if let Some(id) = outcome.result.unwrap().spool_id { crate::query_spool::drop_spool(&id); }
    let rejected = begin(&f, f.request("DELETE FROM items", 0));
    assert_eq!(wait_final(&rejected).await.outcome.unwrap().state, ExecutionState::Failed);
    f.cleanup().await;

    let f = Fixture::new(false).await;
    let db = db::get_db(&f.state.registry, "c1").unwrap();
    { let mut lease = f.state.manual_transactions.lease("c1").await; lease.start(db.as_ref(), DbKind::of("sqlite"), "main".into(), "tab1".into()).await.unwrap(); lease.bind_principal("main"); }
    let mut req = f.request("INSERT INTO items VALUES(2,'transaction'); SELECT text FROM items ORDER BY id", 0);
    req.transaction_owner = Some("tab1".into()); f.authorize(&mut req);
    let entry = begin(&f, req);
    let outcome = wait_final(&entry).await.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.transaction_state, "active");
    let id = outcome.result.unwrap().spool_id.unwrap();
    { let mut lease = f.state.manual_transactions.lease("c1").await; lease.finish("tab1", false).await.unwrap(); }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    assert_eq!(crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap().set(1).unwrap().fetch(0, 200).unwrap().len(), 2);
    crate::query_spool::drop_spool(&id);
    f.cleanup().await;
}

#[tokio::test]
async fn cancelled_before_send_finishes_without_preview_or_writes() {
    let f = Fixture::new(false).await;
    let mut req = f.request("INSERT INTO items VALUES(2,'must not execute')", 0);
    req.cancel_before_send = true;
    let entry = begin(&f, req);
    let snapshot = wait_final(&entry).await;
    assert!(snapshot.preview.is_none());
    let outcome = snapshot.outcome.unwrap();
    assert_eq!(outcome.state, ExecutionState::Cancelled);
    assert_eq!(outcome.effects, "no_effect");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
