//! Real SQLite execution-to-spool-to-export regression, without user databases.
use super::*;

#[tokio::test]
async fn c1_query_spool_pages_and_full_export_use_same_execution_offline() {
    let f = Fixture::new(false).await;
    sqlx::query("CREATE TABLE exported(id INTEGER PRIMARY KEY, text TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12005) INSERT INTO exported SELECT x, '中文' FROM n").execute(&f.observer).await.unwrap();
    let outcome = f.execute("SELECT id,text FROM exported ORDER BY id", 0).await;
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].total, 12005);
    // 审定文案：「分页与导出使用本次缓存」（等价语义：导出读缓存全部行，不重跑 SQL）
    assert!(result.sets[0].message.contains("分页与导出使用本次缓存") || result.sets[0].message.contains("导出默认包含全部行"),
        "实际 message: {:?}", result.sets[0].message);
    assert!(!result.sets[0].message.contains("导出仅包含当前预览"));
    let id = result.spool_id.expect("原生执行必须返回完整结果缓存");
    let cache = crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap();
    let set = cache.set(0).unwrap();
    set.ensure_complete().unwrap();
    let last = set.fetch(12000, 200).unwrap();
    assert_eq!(last.len(), 5);
    assert_eq!(last[0][0].as_deref(), Some("12001"));
    assert_eq!(last[4], vec![Some("12005".into()), Some("中文".into())]);
    // Mutate the database and disconnect: subsequent reads/exports must use this snapshot.
    sqlx::query("DELETE FROM exported").execute(&f.observer).await.unwrap();
    let conn = f.state.registry.write().remove("c1").unwrap();
    if let Db::Sqlite(pool) = conn.as_ref() { pool.close().await; }
    for format in ["csv", "json", "sql"] {
        let file = f.root.join(format!("all.{format}"));
        let opts = ExportOptions { connection_id: "c1".into(), schema: "main".into(), table: "exported".into(), file: file.to_string_lossy().into(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: "utf-8".into(), where_clause: None, limit: None };
        let count = crate::commands::export_query_spooled_service(&f.state, opts, &id, "tab1", "main", 0, "main").await.unwrap();
        assert_eq!(count, 12005, "{format}");
        let text = std::fs::read_to_string(file).unwrap();
        if format == "json" {
            let rows: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
            assert_eq!(rows.len(), 12005);
            assert_eq!(rows.last().unwrap()["id"], "12005");
        } else if format == "sql" { assert_eq!(text.matches("INSERT INTO").count(), 12005); }
        else { assert_eq!(text.lines().count(), 12006); }
        assert!(text.contains("12005") && text.contains("中文"));
    }
    assert_eq!(set.fetch(12000, 200).unwrap(), last);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM exported").fetch_one(&f.observer).await.unwrap(), 0);
    crate::query_spool::drop_spool(&id);
    f.cleanup().await;
}

#[tokio::test]
async fn c1_query_spool_failure_keeps_previous_snapshot_and_success_is_fresh() {
    let f = Fixture::new(false).await;
    let first = f.execute("SELECT text FROM items", 0).await.result.unwrap();
    let id = first.spool_id.unwrap();
    let old = crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap();
    let failed = f.execute("SELECT * FROM does_not_exist", 0).await;
    assert_eq!(failed.state, ExecutionState::Failed);
    assert!(failed.result.is_none());
    assert!(crate::query_spool::exists(&id));
    assert_eq!(old.set(0).unwrap().fetch(0, 200).unwrap(), vec![vec![Some("原值".into())]]);
    assert!(crate::query_spool::authorize(&id, "c1", "tab2", "main", "main").is_err());
    assert!(crate::query_spool::authorize(&id, "c1", "tab1", "main", "other-window").is_err());
    let next = f.execute("SELECT 42", 0).await.result.unwrap().spool_id.unwrap();
    assert_ne!(id, next);
    assert!(crate::query_spool::exists(&id), "成功替换由持有旧快照的查询页明确释放");
    crate::query_spool::drop_spool(&id); crate::query_spool::drop_spool(&next);
    f.cleanup().await;
}

#[tokio::test]
async fn c1_query_spool_manual_transaction_snapshot_survives_rollback() {
    let f = Fixture::new(false).await;
    let conn = db::get_db(&f.state.registry, "c1").unwrap();
    {
        let mut lease = f.state.manual_transactions.lease("c1").await;
        lease.start(&conn, DbKind::of("sqlite"), "main".into(), "tab1".into()).await.unwrap();
        lease.bind_principal("main");
    }
    let mut request = f.request("INSERT INTO items VALUES(2,'事务中'); SELECT text FROM items ORDER BY id", 0);
    request.transaction_owner = Some("tab1".into()); f.authorize(&mut request);
    let outcome = execute_sql_service(&f.state, request, "main").await.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    let id = outcome.result.unwrap().spool_id.unwrap();
    let cache = crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap();
    let set = cache.set(1).unwrap(); set.ensure_complete().unwrap();
    let mut lease = f.state.manual_transactions.lease("c1").await;
    lease.finish("tab1", false).await.unwrap(); drop(lease);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    assert_eq!(set.fetch(0, 200).unwrap(), vec![vec![Some("原值".into())], vec![Some("事务中".into())]]);
    crate::query_spool::drop_spool(&id);
    f.cleanup().await;
}

#[tokio::test]
async fn c1_query_spool_stop_completion_race_removes_untrusted_cache() {
    let registry = Registry::default();
    let mut request = control_request("SELECT 1", 0);
    let entry = registered(&registry, &mut request); entry.start();
    let cache = crate::query_spool::create("c1", "tab1", "main", "main");
    entry.attach_spool(cache.clone());
    cache.begin_set(0, &["value".into()], &["TEXT".into()]).push(&[Some("1".into())]);
    cache.finish_all(); entry.request_stop(false);
    let result = MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: Some(cache.id.clone()) };
    let outcome = entry.finish(Ok(result), false, false);
    assert_eq!(outcome.state, ExecutionState::Unknown);
    assert!(outcome.result.is_none());
    assert!(!crate::query_spool::exists(&cache.id));
    assert!(entry.spool().is_none());
}
