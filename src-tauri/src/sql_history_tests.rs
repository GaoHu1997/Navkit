use super::*;
use crate::models::{DbKind, FilterCondition, FilterLine, FilterLogic, PageRequest, SortRule};
use futures::TryStreamExt;

#[path = "sql_history_page_tests.rs"]
mod page_tests;

#[test]
fn queued_writer_keeps_sink_order_and_flush_barriers() {
    let root = std::env::temp_dir().join(format!("navkit-history-batches-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let first = root.join("first.jsonl"); let second = root.join("second.jsonl");
    let entry = |sql: String| {
        let mut guard = Guard::in_context(&sql, Vec::new(), Context::default());
        guard.entry.take().unwrap()
    };
    let (send, receive) = std::sync::mpsc::channel();
    // Prequeue > one batch, an oversized entry, a changed sink, and a flush.
    for i in 0..260 { send.send(WriteEvent::Entry(first.clone(), entry(format!("SELECT {i}")))).unwrap(); }
    send.send(WriteEvent::Entry(first.clone(), entry("x".repeat(300 * 1024)))).unwrap();
    send.send(WriteEvent::Entry(second.clone(), entry("SECOND".into()))).unwrap();
    let (flushed, barrier) = std::sync::mpsc::channel();
    send.send(WriteEvent::Flush(flushed)).unwrap();
    let writer = std::thread::spawn(move || run_writer(receive));
    barrier.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(crate::storage::read_sql_log_at(&first, 1000).len(), 261);
    assert_eq!(crate::storage::read_sql_log_at(&second, 1000)[0].sql, "SECOND");
    crate::storage::clear_sql_log_at(&first);
    send.send(WriteEvent::Entry(first.clone(), entry("AFTER CLEAR".into()))).unwrap();
    let (flushed, barrier) = std::sync::mpsc::channel(); send.send(WriteEvent::Flush(flushed)).unwrap();
    barrier.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(crate::storage::read_sql_log_at(&first, 1000).len(), 1);
    drop(send); writer.join().unwrap(); std::fs::remove_dir_all(root).unwrap();
}
struct Fixture {
    root: PathBuf,
    db: Arc<Db>,
    path: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("navkit-real-sql-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let db = Arc::new(Db::Sqlite(pool));
        let cfg = ConnectionConfig {
            id: "history-test".into(),
            name: "test only".into(),
            kind: DbKind::of("sqlite"),
            host: String::new(),
            port: 0,
            user: String::new(),
            password: "never persisted".into(),
            database: "wrong display label".into(),
            file: String::new(),
            params: String::new(),
            group: String::new(),
            read_only: false,
            keep_alive: false,
        };
        register(&db, &cfg, "main", &root);
        let path = root.join("logs/sql_log.jsonl");
        Self { root, db, path }
    }
    fn pool(&self) -> &sqlx::SqlitePool {
        let Db::Sqlite(p) = self.db.as_ref() else {
            panic!()
        };
        p
    }
    fn logs(&self) -> Vec<SqlLogEntry> {
        crate::storage::read_sql_log_at(&self.path, 1000)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[tokio::test]
async fn actual_text_binds_failure_and_pool_clone_are_recorded_once() {
    let f = Fixture::new().await;
    let pool = f.pool().clone();
    let sql = " 
/* original quote ' 中文 */
SELECT ? AS payload, ? AS bytes, ? AS nullable  ";
    let result = query(sql)
        .bind("'; DROP TABLE x; --中文")
        .bind(vec![0u8, 255, 128])
        .bind(None::<String>)
        .fetch_one(&pool)
        .await;
    assert!(result.is_ok());
    let logs = f.logs();
    assert_eq!(logs.len(), 1);
    let e = &logs[0];
    assert_eq!(e.sql.as_bytes(), sql.as_bytes());
    assert_eq!(e.connection_id, "history-test");
    assert_eq!(
        e.database,
        f.pool().connect_options().get_filename().to_string_lossy()
    );
    assert_eq!(e.namespace, "main");
    assert!(e.ok);
    assert_eq!(e.rows, 1);
    assert_eq!(e.parameters.len(), 3);
    assert!(e.parameters[0].value.contains("DROP TABLE"));
    assert_eq!(e.parameters[1].value, "[0, 255, 128]");
    assert_eq!(e.parameters[2].value, "None");
    assert!(!serde_json::to_string(e)
        .unwrap()
        .contains("never persisted"));
    let bad = "INSERT INTO no_such_table VALUES(?)";
    assert!(query(bad).bind(7i64).execute(&pool).await.is_err());
    let logs = f.logs();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].sql, bad);
    assert!(!logs[0].ok);
    assert!(!logs[0].message.is_empty());
    assert_eq!(logs[0].parameters[0].value, "7");
}
#[tokio::test]
async fn streams_complete_drop_and_never_polled_have_honest_states() {
    let f = Fixture::new().await;
    let mut stream = query("SELECT 1 UNION ALL SELECT 2").fetch(f.pool());
    assert!(stream.try_next().await.unwrap().is_some());
    drop(stream);
    let logs = f.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].execution_state,
        Some(crate::execution_contracts::ExecutionState::Unknown)
    );
    let stream = query("SELECT 99").fetch(f.pool());
    drop(stream);
    assert_eq!(f.logs().len(), 1);
    let _: Vec<_> = query("SELECT 3 UNION ALL SELECT 4")
        .fetch(f.pool())
        .try_collect()
        .await
        .unwrap();
    assert!(f.logs()[0].ok);
    assert_eq!(f.logs()[0].rows, 2);
}
#[tokio::test]
async fn page_filter_order_count_and_metadata_are_real_sql() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT)")
        .execute(f.pool())
        .await
        .unwrap();
    query("INSERT INTO items VALUES(1,'alpha'),(2,'beta'),(3,'gamma')")
        .execute(f.pool())
        .await
        .unwrap();
    crate::storage::clear_sql_log_at(&f.path);
    let req = PageRequest {
        connection_id: "history-test".into(),
        schema: "main".into(),
        table: "items".into(),
        offset: 0,
        limit: 1,
        include_total: true,
        order_by: None,
        ascending: false,
        filters: vec![FilterLine {
            conds: vec![FilterCondition {
                column: "id".into(),
                operator: ">".into(),
                value: "1".into(),
                logic: None,
                case_sensitive: false,
            }],
            logic: FilterLogic::And,
        }],
        filter_logic: FilterLogic::And,
        order_rules: Some(vec![SortRule {
            column: "id".into(),
            ascending: false,
        }]),
        after: None,
    };
    let page = crate::schema::page_table(f.db.as_ref(), DbKind::of("sqlite"), &req)
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 1);
    let logs = f.logs();
    assert!(logs
        .iter()
        .any(|e| e.sql.starts_with("SELECT COUNT(*) FROM") && e.sql.contains("WHERE")));
    let e = logs
        .iter()
        .find(|e| e.sql.starts_with("SELECT * FROM") && e.sql.contains("LIMIT 1 OFFSET 0"))
        .unwrap();
    assert!(e.sql.contains("WHERE") && e.sql.contains("DESC"));
    assert!(!e.sql.contains("条筛选"));
    assert!(logs.iter().any(|e| e.sql.starts_with("PRAGMA")));
    assert!(logs.iter().all(|e| e.connection_id == "history-test"));
}
#[tokio::test]
async fn pinned_transaction_binds_keep_context_and_rollback_semantics() {
    let f = Fixture::new().await;
    query("CREATE TABLE t(value TEXT)")
        .execute(f.pool())
        .await
        .unwrap();
    with_db(f.db.as_ref(), async {
        let mut tx = f.pool().begin().await.unwrap();
        query("INSERT INTO t VALUES(?)")
            .bind("real value")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
    })
    .await;
    assert_eq!(
        query_scalar::<_, i64>("SELECT COUNT(*) FROM t")
            .fetch_one(f.pool())
            .await
            .unwrap(),
        0
    );
    let logs = f.logs();
    let e = logs
        .iter()
        .find(|e| e.sql == "INSERT INTO t VALUES(?)")
        .unwrap();
    assert_eq!(e.connection_id, "history-test");
    assert_eq!(e.parameters[0].value, "\"real value\"");
}
#[tokio::test]
async fn explicit_sensitive_binds_and_unwritable_sink_do_not_change_execution() {
    let f = Fixture::new().await;
    query("SELECT ?")
        .bind_sensitive("secret-value")
        .fetch_one(f.pool())
        .await
        .unwrap();
    assert!(f.logs()[0].parameters[0].redacted);
    assert!(!serde_json::to_string(&f.logs()[0])
        .unwrap()
        .contains("secret-value"));
    let block = f.root.join("not-a-directory");
    std::fs::write(&block, "block").unwrap();
    let c = Context {
        path: Some(block.join("cannot-write.jsonl")),
        ..Default::default()
    };
    scope(c, async {
        let mut conn = f.pool().acquire().await.unwrap();
        assert_eq!(
            query_scalar::<_, i64>("SELECT 42")
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            42
        );
    })
    .await;
}
#[tokio::test]
async fn sql_import_and_export_capture_real_statements_not_operation_markers() {
    let f = Fixture::new().await;
    let input = f.root.join("input.sql");
    std::fs::write(&input,"CREATE TABLE imported(id INTEGER PRIMARY KEY,payload TEXT); INSERT INTO imported VALUES(1,'中文'),(2,'beta');").unwrap();
    let options:crate::models::ImportOptions=serde_json::from_value(serde_json::json!({"connection_id":"history-test","schema":"main","table":"imported","file":input,"format":"sql","encoding":"utf8"})).unwrap();
    assert_eq!(
        crate::transfer::import_sql(f.db.as_ref(), DbKind::of("sqlite"), &options)
            .await
            .unwrap(),
        2
    );
    let logs = f.logs();
    assert!(logs
        .iter()
        .any(|e| e.sql.trim_start().starts_with("CREATE TABLE imported") && e.ok));
    assert!(logs
        .iter()
        .any(|e| e.sql.trim_start().starts_with("INSERT INTO imported") && e.ok));
    let target = f.root.join("export.csv");
    let export:crate::transfer::ExportTaskOptions=serde_json::from_value(serde_json::json!({"task_id":uuid::Uuid::new_v4().to_string(),"connection_id":"history-test","schema":"main","table":"imported","file":target,"format":"csv","has_header":true,"delimiter":",","encoding":"utf8"})).unwrap();
    assert_eq!(
        crate::transfer::run_export_task(f.db.as_ref(), &DbKind::of("sqlite"), &export)
            .await
            .unwrap(),
        2
    );
    let logs = f.logs();
    assert!(logs
        .iter()
        .any(|e| e.sql.contains("SELECT") && e.sql.contains("imported") && e.ok));
    assert!(logs.iter().all(|e| e.connection_id == "history-test"));
}

#[test]
fn old_history_records_remain_deserializable() {
    let e:SqlLogEntry=serde_json::from_str(r#"{"ts":"2099-01-01 00:00:00","connection_id":"old","connection_name":"old","kind":"sqlite","database":"main","sql":"SELECT 1","ok":true,"elapsed_ms":1,"rows":1,"message":"","origin":"query"}"#).unwrap();
    assert!(e.parameters.is_empty());
    assert!(e.namespace.is_empty());
}
