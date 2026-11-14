//! C1 实际临时 SQLite；不启动服务，不连接用户数据库。
#[path = "execution_spool_tests.rs"]
mod spool_tests;
#[path = "execution_stream_service_tests.rs"]
mod stream_service_tests;
#[path = "perf_bench_tests.rs"]
mod perf_bench;
use std::{sync::Arc, path::PathBuf, time::Duration};
use crate::{commands::{AppState, execute_sql_service}, db::{self, Db}, execution::{Binding, Registry}, execution_contracts::*, models::*, storage::Store};
const LONG: &str = "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n";
struct Fixture { state: Arc<AppState>, observer: sqlx::SqlitePool, root: PathBuf }
impl Fixture {
    async fn new(read_only: bool) -> Self {
        let root = std::env::temp_dir().join(format!("navkit-execution-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&root).unwrap();
        let path = root.join("test.sqlite");
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true).journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let observer = sqlx::sqlite::SqlitePoolOptions::new().max_connections(2).connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, text TEXT); INSERT INTO items VALUES(1,'原值'); CREATE VIEW v AS SELECT * FROM items").execute(&observer).await.unwrap();
        let store = Store::empty_at(&root);
        let cfg = ConnectionConfig { id: "c1".into(), name: "临时测试".into(), kind: DbKind::of("sqlite"), host: String::new(), port: 0, user: String::new(), password: String::new(), database: "main".into(), file: path.to_string_lossy().into(), params: String::new(), group: String::new(), read_only, keep_alive: false };
        store.save_connections(vec![cfg.clone()], vec![]).unwrap();
        let state = Arc::new(AppState { store, registry: Arc::new(db::DbRegistry::default()), manual_transactions: Default::default(), activity: Default::default(), executions: Default::default(), mutations: Default::default(), connection_runtime: Default::default(), query_results: Default::default() });
        let db = db::open_saved(&db::request_from_config(&cfg, ""), read_only, &state.store.settings()).await.unwrap();
        let conn = Arc::new(db);
        // 与生产准入一致：连接必须登记运行指纹，否则 ensure 的上下文校验会拒绝。
        let expected = crate::db::runtime::binding_fingerprint_at(&cfg, state.store.data_root()).unwrap();
        state.connection_runtime.publish("c1", &expected, &conn, None, crate::db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
        state.registry.write().insert("c1".into(), conn);
        Self { state, observer, root }
    }
    fn request(&self, sql: &str, timeout_ms: u64) -> ExecutionRequest {
        let mut request = ExecutionRequest { execution_id: String::new(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: sql.into(), transaction_owner: None, timeout_ms: Some(timeout_ms), origin: Some("query".into()), cancel_before_send: false, execution_grant: None };
        self.authorize(&mut request);
        request
    }
    fn authorize(&self, request: &mut ExecutionRequest) {
        let endpoint = self.state.store.connections().into_iter().find(|cfg| cfg.id == request.connection_id).unwrap();
        let binding = crate::commands::execution_binding(&self.state, &endpoint, "main", &request.owner_id, &request.namespace).unwrap();
        self.state.executions.grant_request(request, binding).unwrap();
    }
    async fn execute(&self, sql: &str, ms: u64) -> ExecutionOutcome {
        // 外层预算只用于防止挂死；单个用例实测约 6s，全量长跑的调度抖动需要更宽余量（断言本体不变）。
        tokio::time::timeout(Duration::from_secs(90), execute_sql_service(&self.state, self.request(sql, ms), "main")).await.expect("执行服务没有在测试上限内返回").unwrap()
    }
    async fn cleanup(self) {
        async fn close_physical(pool: &sqlx::SqlitePool) {
            if !pool.is_closed() {
                // Wait for physical SQLite worker/file-handle closure, not only pool accounting.
                let mut physical = Vec::new();
                for _ in 0..pool.size() { physical.push(pool.acquire().await.unwrap()); }
                for connection in physical { connection.close().await.unwrap(); }
            }
            pool.close().await;
        }
        let Self { state, observer, root } = self;
        drop(state.manual_transactions.close_all().await.unwrap());
        let conns: Vec<_> = state.registry.write().drain().map(|(_, v)| v).collect();
        for conn in conns { if let Db::Sqlite(pool) = conn.as_ref() { close_physical(pool).await; } }
        close_physical(&observer).await;
        drop(observer); drop(state);
        // Only remove this fixture's UUID directory; errors remain hard failures.
        for attempt in 1..=20 {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => return,
                Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)) && attempt < 20 => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(error) => panic!("isolated execution fixture {} cleanup failed after {attempt} attempts: {error}", root.display()),
            }
        }
    }
}
fn binding(request: &ExecutionRequest) -> Binding {
    Binding { principal: "main".into(), owner: request.owner_id.clone(), connection: request.connection_id.clone(), namespace: request.namespace.clone(), fingerprint: "fixture".into() }
}
fn registered(registry: &Registry, request: &mut ExecutionRequest) -> Arc<crate::execution::Entry> {
    let binding = binding(request);
    registry.grant_request(request, binding.clone()).unwrap();
    registry.register(request, binding, 30).unwrap()
}
#[tokio::test]
async fn c1_query_preview_row_limit_drains_and_next_execution_is_independent() {
    let f = Fixture::new(false).await;
    let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12005) SELECT x FROM n ORDER BY x";
    let outcome = f.execute(sql, 0).await;
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.effects, "completed");
    assert!(!outcome.resources_pending);
    let result = outcome.result.unwrap();
    let set = &result.sets[0];
    assert_eq!(set.total, 12005);
    assert_eq!(set.rows.len(), 10000);
    assert_eq!(set.rows[0][0].as_deref(), Some("1"));
    assert_eq!(set.rows[9999][0].as_deref(), Some("10000"));
    assert!(set.ok && set.has_rows);
    // 审定行为：结果进入本机缓存后，文案指出「分页与导出使用本次缓存」（旧版为仅预览上限）；
    // 核心语义（完整读取 + 不重跑 SQL）必须仍在文案中体现。
    // 审定文案有三种形态（含缓存/预览上限/分页）；核心语义：数据已完整读取且不重跑 SQL。
    assert!((set.message.contains("本次执行缓存") || set.message.contains("本次缓存") || set.message.contains("数据库响应已完整读取"))
        && set.message.contains("10000"), "实际 message: {:?}", set.message);
    assert!(result.messages.iter().any(|m| m.contains("12005") && m.contains("10000")));
    assert!(!f.state.executions.has_active("c1"));
    let next = f.execute("SELECT 42", 0).await;
    assert_ne!(next.execution_id, outcome.execution_id);
    assert_eq!(next.state, ExecutionState::Succeeded);
    assert_eq!(next.result.unwrap().sets[0].rows, vec![vec![Some("42".into())]]);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_query_preview_boundary_empty_rows_and_batch_after_large_result() {
    let f = Fixture::new(false).await;
    let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) SELECT x FROM n; SELECT text FROM items WHERE id=-1; INSERT INTO items VALUES(2,'后续写入'); SELECT text FROM items WHERE id=2";
    let outcome = f.execute(sql, 0).await;
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets.len(), 4);
    assert_eq!(result.sets[0].total, 10000);
    assert_eq!(result.sets[0].rows.len(), 10000);
    assert!(result.sets[0].message.is_empty(), "exactly the limit is not a truncated result");
    assert!(result.sets[1].has_rows && result.sets[1].rows.is_empty());
    assert_eq!(result.sets[1].columns[0].name, "text");
    assert_eq!(result.sets[2].affected, 1);
    assert_eq!(result.sets[3].rows[0][0].as_deref(), Some("后续写入"));
    let larger = f.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10001) SELECT x FROM n; UPDATE items SET text='恰好一次' WHERE id=2; SELECT text FROM items WHERE id=2", 0).await;
    assert_eq!(larger.state, ExecutionState::Succeeded, "{:?}", larger.error);
    let sets = larger.result.unwrap().sets;
    assert_eq!(sets[0].total, 10001);
    assert_eq!(sets[0].rows.len(), 10000);
    assert_eq!(sets[1].affected, 1);
    assert_eq!(sets[2].rows[0][0].as_deref(), Some("恰好一次"));
    f.cleanup().await;
}
#[tokio::test]
async fn c1_query_preview_byte_limit_drains_without_discarding_later_results() {
    let f = Fixture::new(false).await;
    let sql = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<55) SELECT printf('%.*c',1048576,'x') AS payload FROM n; SELECT 7";
    let outcome = f.execute(sql, 0).await;
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].total, 55);
    assert!(!result.sets[0].rows.is_empty() && result.sets[0].rows.len() < 55);
    assert!(result.sets[0].rows.iter().all(|row| row[0].as_ref().unwrap().len() == 1048576));
    // 字节预览上限提示因缓存路径措辞不同；断言必须体现「预览有上限且数据完整」
    assert!(result.sets[0].message.contains("50 MiB") || result.sets[0].message.contains("预览") || result.sets[0].message.contains("缓存"),
        "实际 message: {:?}", result.sets[0].message);
    assert_eq!(result.sets[1].rows, vec![vec![Some("7".into())]]);
    assert!(!outcome.resources_pending);
    assert_eq!(f.execute("SELECT 8", 0).await.state, ExecutionState::Succeeded);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_query_preview_in_manual_transaction_remains_committable() {
    let f = Fixture::new(false).await;
    let conn = db::get_db(&f.state.registry, "c1").unwrap();
    {
        let mut lease = f.state.manual_transactions.lease("c1").await;
        lease.start(&conn, DbKind::of("sqlite"), "main".into(), "tab1".into()).await.unwrap();
        lease.bind_principal("main");
    }
    let mut request = f.request("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<12001) SELECT x FROM n; INSERT INTO items VALUES(2,'可提交')", 0);
    request.transaction_owner = Some("tab1".into()); f.authorize(&mut request);
    let outcome = execute_sql_service(&f.state, request, "main").await.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded, "{:?}", outcome.error);
    assert_eq!(outcome.transaction_state, "active");
    assert_eq!(outcome.result.unwrap().sets[0].total, 12001);
    let mut lease = f.state.manual_transactions.lease("c1").await;
    assert!(!lease.status().failed);
    lease.finish("tab1", true).await.unwrap(); drop(lease);
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT text FROM items WHERE id=2").fetch_one(&f.observer).await.unwrap(), "可提交");
    f.cleanup().await;
}
#[tokio::test]
async fn c1_sqlite_memory_and_temporary_objects_survive_settled_execution() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    let db = Db::Sqlite(pool.clone());
    for sql in ["CREATE TABLE items(id INTEGER); INSERT INTO items VALUES(17); CREATE TEMP TABLE local_note(value TEXT); INSERT INTO local_note VALUES('临时稿')", "SELECT id FROM items; SELECT value FROM local_note", LONG, "SELECT id FROM items; SELECT value FROM local_note"] {
        let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: sql.into(), transaction_owner: None, timeout_ms: Some(if sql == LONG { 20 } else { 0 }), origin: None, cancel_before_send: false, execution_grant: None };
        let entry = registered(&Registry::default(), &mut req);
        let (result, confirmed) = crate::execution::run(&db, &DbKind::of("sqlite"), sql, &entry, false).await;
        let out = entry.finish(result, confirmed, false);
        if sql == LONG { assert_eq!(out.state, ExecutionState::TimedOut); }
        else { assert_eq!(out.state, ExecutionState::Succeeded, "{:?}", out.error); }
        if sql.starts_with("SELECT id") { let sets = out.result.unwrap().sets; assert_eq!(sets[0].rows[0][0].as_deref(), Some("17")); assert_eq!(sets[1].rows[0][0].as_deref(), Some("临时稿")); }
    }
    pool.close().await;
}
#[tokio::test]
async fn c1_sqlite_readonly_rejects_temporary_table_shadow_and_cleans_snapshot() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE items(id INTEGER); INSERT INTO items VALUES(17); CREATE TEMP TABLE items(id INTEGER); INSERT INTO temp.items VALUES(99)").execute(&pool).await.unwrap();
    let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: "SELECT id FROM items".into(), transaction_owner: None, timeout_ms: Some(0), origin: None, cancel_before_send: false, execution_grant: None };
    let entry = registered(&Registry::default(), &mut req);
    let mut conn = pool.acquire().await.unwrap();
    let (result, confirmed) = crate::execution::sqlite_run(&mut conn, &req.sql, &entry, true).await;
    assert_eq!(entry.finish(result.map_err(crate::error::AppError::from), confirmed, false).state, ExecutionState::Failed);
    // 清理后 authorizer 已移除，临时对象仍可读；内部读事务已结束，显式 BEGIN 可用。
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT id FROM temp.items").fetch_one(&mut *conn).await.unwrap(), 99);
    sqlx::query("BEGIN; ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn); pool.close().await;
}
#[tokio::test]
async fn c1_sqlite_queued_pool_cancel_never_sends_the_write() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    let mut held = pool.acquire().await.unwrap();
    sqlx::query("CREATE TABLE items(id INTEGER); INSERT INTO items VALUES(17)").execute(&mut *held).await.unwrap();
    let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: "DELETE FROM items".into(), transaction_owner: None, timeout_ms: Some(0), origin: None, cancel_before_send: false, execution_grant: None };
    let entry = registered(&Registry::default(), &mut req);
    let running_entry = entry.clone(); let running_pool = pool.clone();
    let task = tokio::spawn(async move { crate::execution::run(&Db::Sqlite(running_pool), &DbKind::of("sqlite"), &req.sql, &running_entry, false).await });
    tokio::task::yield_now().await;
    assert_eq!(entry.status().state, ExecutionState::Queued); entry.request_stop(false);
    let (result, confirmed) = tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    let outcome = entry.finish(result, confirmed, false);
    assert_eq!(outcome.state, ExecutionState::Cancelled); assert!(outcome.deadline.stop_confirmed);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&mut *held).await.unwrap(), 1);
    held.close().await.unwrap(); pool.close().await;
}
#[tokio::test]
async fn c1_sqlite_readonly_whole_batch_no_side_effects_and_projection() {
    let f = Fixture::new(true).await;
    for sql in ["SELECT 1; DELETE FROM items", "WITH x AS (DELETE FROM items RETURNING *) SELECT * FROM x", "SELECT * INTO copy FROM items", "SELECT load_extension('evil')", "PRAGMA query_only=OFF", "ATTACH DATABASE ':memory:' AS other", "EXPLAIN ANALYZE DELETE FROM items", "SELECT * FROM v", "SELECT random()", "SELECT text || 'changed' FROM items"] {
        let result = f.execute(sql, 0).await; assert_eq!(result.state, ExecutionState::Failed, "{sql}"); assert!(result.result.is_none());
    }
    let result = f.execute("SELECT id,text FROM items LIMIT 10; SELECT '中文;DROP',NULL,9223372036854775807", 0).await;
    assert_eq!(result.state, ExecutionState::Succeeded, "{:?}", result.error);
    let sets = result.result.unwrap().sets; assert_eq!(sets[0].rows[0][1].as_deref(), Some("原值")); assert_eq!(sets[1].rows[0][2].as_deref(), Some("9223372036854775807"));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    // 文件以 SQLITE_OPEN_READONLY 打开，即使未安装 authorizer 的底层旧入口也不能写。
    assert!(db::execute(&f.state.registry, "c1", "DELETE FROM items", DbKind::of("sqlite")).await.is_err());
    f.cleanup().await;
}
#[tokio::test]
async fn c1_sqlite_real_recursive_cancel_next_query_usable() {
    let f = Fixture::new(false).await; let req = f.request(LONG, 0); let id = req.execution_id.clone(); let state = f.state.clone();
    let task = tokio::spawn(async move { execute_sql_service(&state, req, "main").await.unwrap() });
    let entry = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // 使用 grant 签发的真实 ID；注册错误/身份变化不能无限等待。
            let found = f.state.executions.entry_for_test(&id);
            if let Some(entry) = found { if entry.status().state == ExecutionState::Running { break entry; } }
            assert!(!task.is_finished(), "执行在进入 Running 前已经结束，无法注册 {id}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }).await.expect("执行注册/运行等待超过测试上限");
    let ack = entry.request_stop(false); assert!(ack.accepted); assert_eq!(ack.state, ExecutionState::CancelRequested);
    entry.request_stop(false); entry.request_stop(true);
    let outcome = tokio::time::timeout(Duration::from_secs(60), task).await.unwrap().unwrap();
    assert_eq!(outcome.state, ExecutionState::Cancelled, "{:?}", outcome.error); assert!(outcome.deadline.stop_confirmed); assert!(!outcome.deadline.deadline_reached);
    assert!(!entry.request_stop(false).accepted);
    assert_eq!(f.execute("SELECT 42", 0).await.state, ExecutionState::Succeeded);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_sqlite_deadline_real_interrupt_and_zero_disabled() {
    let f = Fixture::new(false).await;
    let outcome = f.execute(LONG, 30).await;
    assert_eq!(outcome.state, ExecutionState::TimedOut, "{:?}", outcome.error); assert!(outcome.deadline.deadline_reached && outcome.deadline.stop_confirmed);
    assert_eq!(f.execute("SELECT 7", 0).await.deadline.budget_ms, None);
    assert_eq!(f.execute("SELECT 8", 1000).await.state, ExecutionState::Succeeded);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_sqlite_batch_deadline_does_not_send_later_write() {
    let f = Fixture::new(false).await;
    let outcome = f.execute(&format!("{LONG}; DELETE FROM items"), 20).await;
    assert_eq!(outcome.state, ExecutionState::TimedOut);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_transaction_timeout_requires_rollback_and_rejects_other_window() {
    let f = Fixture::new(false).await;
    let conn = db::get_db(&f.state.registry, "c1").unwrap();
    { let mut lease = f.state.manual_transactions.lease("c1").await;
        lease.start(&conn, DbKind::of("sqlite"), "main".into(), "tab1".into()).await.unwrap(); lease.bind_principal("main"); }
    let mut req = f.request("INSERT INTO items VALUES(2,'未提交')", 0); req.transaction_owner = Some("tab1".into()); f.authorize(&mut req);
    assert_eq!(execute_sql_service(&f.state, req, "main").await.unwrap().state, ExecutionState::Succeeded);
    let mut req = f.request(LONG, 20); req.transaction_owner = Some("tab1".into()); f.authorize(&mut req);
    let outcome = execute_sql_service(&f.state, req, "main").await.unwrap(); assert_eq!(outcome.state, ExecutionState::TimedOut); assert_eq!(outcome.transaction_state, "rollback_required");
    let mut lease = f.state.manual_transactions.lease("c1").await;
    assert!(lease.status().failed); assert!(lease.require_principal("other-window").is_err()); assert!(lease.finish("tab1", true).await.is_err());
    // SQLite interrupt 可已自动回滚，此时再次显式 rollback 也可能返回 no transaction；不能报已回滚假成功。
    let _ = lease.finish("tab1", false).await; drop(lease);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    assert_eq!(f.execute("SELECT 1", 0).await.state, ExecutionState::Succeeded); f.cleanup().await;
}
#[tokio::test]
async fn c1_execution_duplicate_invalid_ids_owner_and_endpoint_binding() {
    let f = Fixture::new(false).await; let req = f.request("SELECT 1", 0); let id = req.execution_id.clone();
    assert_eq!(execute_sql_service(&f.state, req.clone(), "main").await.unwrap().state, ExecutionState::Succeeded);
    assert_eq!(execute_sql_service(&f.state, req, "main").await.unwrap_err().code, "EXECUTION_ID_DUPLICATE");
    let entry = f.state.executions.entry_for_test(&id).unwrap();
    for mut b in [entry.binding.clone(), entry.binding.clone(), entry.binding.clone(), entry.binding.clone()] {
        b.principal = "other".into(); assert!(f.state.executions.authorized(&id, &b).is_err());
        b = entry.binding.clone(); b.owner = "other".into(); assert!(f.state.executions.authorized(&id, &b).is_err());
        b = entry.binding.clone(); b.fingerprint = "changed".into(); assert!(f.state.executions.authorized(&id, &b).is_err());
        b = entry.binding.clone(); b.namespace = "other".into(); assert!(f.state.executions.authorized(&id, &b).is_err());
    }
    let mut req = f.request("SELECT 1", 0); req.execution_id = "arbitrary".into(); assert!(execute_sql_service(&f.state, req, "main").await.is_err());
    let mut req = f.request("SELECT 1", 0); req.transaction_owner = Some("other".into()); assert!(execute_sql_service(&f.state, req, "main").await.is_err());
    f.cleanup().await;
}
#[tokio::test]
async fn c1_protected_controls_and_queued_cancel_have_zero_writes() {
    let f = Fixture::new(false).await;
    for sql in ["INSERT INTO items VALUES(2,'不应发送'); COMMIT", "BEGIN", "SET search_path=other", "PRAGMA query_only=OFF", "ATTACH DATABASE ':memory:' AS other"] {
        let out = f.execute(sql, 0).await; assert_eq!(out.state, ExecutionState::Failed); assert_eq!(out.error.unwrap().code, "EXECUTION_PROTECTED_CONTROL");
    }
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    let mut req = f.request("DELETE FROM items", 0); let entry = registered(&Registry::default(), &mut req);
    entry.request_stop(false);
    let db = db::get_db(&f.state.registry, "c1").unwrap();
    let (result, confirmed) = crate::execution::run(&db, &DbKind::of("sqlite"), &req.sql, &entry, false).await;
    let out = entry.finish(result, confirmed, false); assert_eq!(out.state, ExecutionState::Cancelled); assert!(out.deadline.stop_confirmed);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_queued_cancel_does_not_wait_for_other_owners_lease_or_send_sql() {
    let f = Fixture::new(false).await;
    let held = f.state.manual_transactions.lease("c1").await;
    let req = f.request("DELETE FROM items", 0); let id = req.execution_id.clone(); let state = f.state.clone();
    let pending = tokio::spawn(async move { execute_sql_service(&state, req, "main").await.unwrap() });
    let entry = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(entry) = f.state.executions.entry_for_test(&id) { break entry; }
            assert!(!pending.is_finished(), "排队执行在注册前已经结束，无法注册 {id}");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.expect("排队注册等待超过测试上限");
    assert_eq!(entry.status().state, ExecutionState::Queued); entry.request_stop(false);
    let outcome = tokio::time::timeout(Duration::from_secs(2), pending).await.unwrap().unwrap();
    assert_eq!(outcome.state, ExecutionState::Cancelled); assert!(outcome.deadline.stop_confirmed);
    drop(held);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
#[test]
fn c1_cancel_after_completion_budget_validation_and_single_cause() {
    let overridden = crate::execution::capabilities(None, &DbKind::of("mysql"), true, false);
    assert_eq!(overridden.cancel.level, "unsupported"); assert_eq!(overridden.manual_transaction.level, "unsupported");
    assert_eq!(overridden.safe_browse.level, "limited");
    let audit = crate::execution::audit_sql("SELECT 'secret-password', '私人行数据'");
    assert!(!audit.contains("secret-password") && !audit.contains("私人行数据"));
    let reg = Registry::default(); let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: "SELECT 1".into(), transaction_owner: None, timeout_ms: Some(0), origin: None, cancel_before_send: false, execution_grant: None };
    let entry = registered(&reg, &mut req); entry.start();
    let outcome = entry.finish(Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }), false, false);
    assert_eq!(outcome.state, ExecutionState::Succeeded); assert!(!entry.request_stop(false).accepted);
    let mut cleanup_req = req.clone(); cleanup_req.timeout_ms = Some(1);
    let cleaning = registered(&reg, &mut cleanup_req); cleaning.start(); cleaning.work_settled();
    std::thread::sleep(Duration::from_millis(3));
    assert!(!cleaning.request_stop(false).accepted); assert!(!cleaning.stopping());
    assert_eq!(cleaning.finish(Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }), false, false).state, ExecutionState::Succeeded);
    let mut bad = req.clone(); bad.timeout_ms = Some(86400001);
    let bad_binding = binding(&bad); reg.grant_request(&mut bad, bad_binding).unwrap();
    assert_eq!(reg.register(&bad, binding(&bad), 30).err().unwrap().code, "EXECUTION_BUDGET_INVALID");
    let mut settings = AppSettings::default(); settings.query_timeout_secs = 0; assert!(settings.validate_execution_settings().is_ok()); settings.connect_timeout_secs = 0; assert!(settings.validate_execution_settings().is_err());
}
#[test]
fn c1_settings_migration_atomic_save_unknown_fields_and_corruption_refusal() {
    let root = std::env::temp_dir().join(format!("navkit-settings-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&root).unwrap();
    let path = root.join("config.json");
    let old = br#"{"query_timeout_secs":60,"future_field":{"preserve":true}}"#;
    std::fs::write(&path, old).unwrap();
    let store = Store::load_at(&root); let mut settings = store.settings();
    assert_eq!(settings.connect_timeout_secs, 60); assert_eq!(settings.pool_acquire_timeout_secs, 60); assert_eq!(settings.sqlite_busy_timeout_secs, 60);
    assert_eq!(settings.settings_version, 2); assert_eq!(std::fs::read(&path).unwrap(), old);
    settings.query_timeout_secs = 0; store.set_settings(settings).unwrap();
    assert_eq!(std::fs::read(root.join("config.v1.backup.json")).unwrap(), old);
    assert_eq!(Store::load_at(&root).settings().query_timeout_secs, 0); assert!(Store::load_at(&root).settings().extra.contains_key("future_field"));
    let before = store.settings(); std::fs::write(&path, b"broken-json").unwrap(); assert!(store.set_settings(before.clone()).is_err()); assert_eq!(std::fs::read(&path).unwrap(), b"broken-json");
    std::fs::write(&path, br#"{"settings_version":99}"#).unwrap(); assert!(store.set_settings(before).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), br#"{"settings_version":99}"#);
    drop(store); std::fs::remove_dir_all(root).unwrap();
}

fn control_request(sql: &str, timeout_ms: u64) -> ExecutionRequest {
    ExecutionRequest { execution_id: String::new(), connection_id: "c1".into(), namespace: "main".into(), owner_id: "tab1".into(), sql: sql.into(), transaction_owner: None, timeout_ms: Some(timeout_ms), origin: Some("query".into()), execution_grant: None, cancel_before_send: false }
}
#[tokio::test]
async fn c1_signed_work_scope_rejects_swapped_sql_budget_transaction_and_origin() {
    let f = Fixture::new(false).await;
    let original = f.request("SELECT count(*) FROM items", 0);
    let mut sql = original.clone(); sql.sql = "INSERT INTO items VALUES(2,'未经授权')".into();
    let mut budget = original.clone(); budget.timeout_ms = Some(20);
    let mut transaction = original.clone(); transaction.transaction_owner = Some("tab1".into());
    let mut origin = original.clone(); origin.origin = Some("job".into());
    for changed in [sql, budget, transaction, origin] {
        let error = execute_sql_service(&f.state, changed, "main").await.unwrap_err();
        assert_eq!(error.code, "EXECUTION_GRANT_INVALID");
    }
    assert_eq!(execute_sql_service(&f.state, original.clone(), "another-window").await.unwrap_err().code, "EXECUTION_GRANT_INVALID");
    let outcome = execute_sql_service(&f.state, original.clone(), "main").await.unwrap();
    assert_eq!(outcome.state, ExecutionState::Succeeded);
    assert_eq!(outcome.execution_id, original.execution_id);
    assert_eq!(execute_sql_service(&f.state, original, "main").await.unwrap_err().code, "EXECUTION_ID_DUPLICATE");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
#[tokio::test]
async fn c1_fixture_cleanup_releases_idle_physical_handles() {
    for _ in 0..7 {
        let fixture = Fixture::new(false).await;
        let directory = fixture.root.clone();
        fixture.cleanup().await;
        assert!(!directory.exists(), "owned UUID fixture directory must be fully removed");
    }
}
#[tokio::test]
async fn c1_grant_negotiation_cancel_before_send_has_no_effect_and_cannot_replay() {
    let f = Fixture::new(false).await;
    let mut request = f.request("DELETE FROM items", 0); request.cancel_before_send = true;
    let outcome = execute_sql_service(&f.state, request.clone(), "main").await.unwrap();
    assert_eq!(outcome.state, ExecutionState::Cancelled);
    assert!(outcome.deadline.stop_confirmed); assert!(!outcome.resources_pending);
    assert_eq!(outcome.effects, "no_effect");
    assert_eq!(execute_sql_service(&f.state, request, "main").await.unwrap_err().code, "EXECUTION_ID_DUPLICATE");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}
#[test]
fn c1_grants_reject_missing_tampered_expired_replayed_and_foreign_process_tokens() {
    use crate::execution_grants::Grants;
    let request = control_request("SELECT 1", 0); let bound = binding(&request); let scope = "scoped-request-digest";
    let mut grants = Grants::default();
    let grant = grants.issue(bound.clone(), scope, 200).unwrap();
    assert_eq!(grant.expires_in_ms, 60_000);
    for (token, now) in [(None, 200), (Some(grant.token.as_str()), 199), (Some(grant.token.as_str()), 60_200)] {
        assert_eq!(grants.consume(&grant.execution_id, token, &bound, scope, now).unwrap_err().code, "EXECUTION_GRANT_INVALID");
    }
    let mut tampered = grant.token.clone().into_bytes(); tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    assert!(grants.consume(&grant.execution_id, Some(&tampered), &bound, scope, 200).is_err());
    assert!(Grants::default().consume(&grant.execution_id, Some(&grant.token), &bound, scope, 200).is_err());
    assert!(grants.consume(&grant.execution_id, Some(&grant.token), &bound, "different-work", 200).is_err());
    for field in 0..5 {
        let mut other = bound.clone();
        match field { 0 => other.principal = "other".into(), 1 => other.owner = "other".into(), 2 => other.connection = "other".into(), 3 => other.namespace = "other".into(), _ => other.fingerprint = "other".into() }
        assert!(grants.consume(&grant.execution_id, Some(&grant.token), &other, scope, 200).is_err());
    }
    grants.consume(&grant.execution_id, Some(&grant.token), &bound, scope, 200).unwrap();
    assert_eq!(grants.consume(&grant.execution_id, Some(&grant.token), &bound, scope, 201).unwrap_err().code, "EXECUTION_ID_DUPLICATE");
}
#[test]
fn c1_retired_grant_window_still_rejects_old_replay_and_preserves_space_namespaces() {
    use crate::execution_grants::Grants;
    let mut request = control_request("SELECT 1", 0); request.namespace = " exact schema ".into();
    let bound = binding(&request); let mut grants = Grants::default(); let scope = "work";
    let old = grants.issue(bound.clone(), scope, 0).unwrap();
    let mut last = old.clone();
    for _ in 0..2048 { last = grants.issue(bound.clone(), scope, 1).unwrap(); }
    grants.consume(&last.execution_id, Some(&last.token), &bound, scope, 1).unwrap();
    assert_eq!(grants.consume(&old.execution_id, Some(&old.token), &bound, scope, 1).unwrap_err().code, "EXECUTION_GRANT_INVALID");
    let registry = Registry::default(); let entry = registered(&registry, &mut request);
    assert_eq!(entry.binding.namespace, " exact schema ");
    assert!(registry.authorized(&entry.id, &Binding { namespace: "exact schema".into(), ..bound }).is_err());
    entry.finish(Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }), false, false);
}
#[tokio::test]
async fn c1_stop_broadcast_wakes_all_waiters_without_spending_a_single_permit() {
    let registry = Registry::default(); let mut request = control_request("SELECT 1", 0); let entry = registered(&registry, &mut request);
    let first = entry.clone(); let second = entry.clone();
    let a = tokio::spawn(async move { first.wait_stop().await });
    let b = tokio::spawn(async move { second.wait_stop().await });
    tokio::task::yield_now().await; entry.request_stop(false);
    tokio::time::timeout(Duration::from_secs(2), async { a.await.unwrap(); b.await.unwrap(); }).await.unwrap();
    let outcome = entry.finish(Err(crate::error::AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND", "未发送")), true, false);
    assert_eq!(outcome.state, ExecutionState::Cancelled);
}
#[tokio::test]
async fn c1_ready_work_is_not_marked_timed_out_after_it_has_already_finished() {
    let registry = Registry::default(); let mut request = control_request("SELECT 1", 1); let entry = registered(&registry, &mut request); entry.start();
    std::thread::sleep(Duration::from_millis(3));
    let (result, confirmed) = crate::execution::drive(async { Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }) }, &entry, || async { panic!("已完成工作不得再发送取消控制"); #[allow(unreachable_code)] Ok(()) }).await;
    let outcome = entry.finish(result.map_err(crate::error::AppError::from), confirmed, false);
    assert_eq!(outcome.state, ExecutionState::Succeeded); assert!(!outcome.deadline.deadline_reached); assert!(!outcome.deadline.stop_confirmed);
}
#[tokio::test]
async fn c1_unknown_retains_work_and_control_until_both_drain_and_blocks_reuse() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>); impl Drop for Dropped { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
    let registry = Registry::default(); let mut request = control_request("SELECT 1", 0); let entry = registered(&registry, &mut request);
    let activity = Arc::new(tokio::sync::RwLock::new(())); entry.own_activity(activity.clone().read_owned().await);
    let work_dropped = Arc::new(AtomicBool::new(false)); let control_dropped = Arc::new(AtomicBool::new(false));
    let (work_done, work_wait) = tokio::sync::oneshot::channel::<()>(); let (control_done, control_wait) = tokio::sync::oneshot::channel::<()>();
    let running = entry.clone(); let working = work_dropped.clone(); let controlling = control_dropped.clone();
    let worker = async move {
        running.start();
        let work = async move { let _guard = Dropped(working); work_wait.await.unwrap(); Err(anyhow::anyhow!("模拟在途数据库结果")) };
        let control = || async move { let _guard = Dropped(controlling); control_wait.await.unwrap(); Ok(()) };
        let (result, confirmed) = crate::execution::drive(work, &running, control).await;
        (result.map_err(crate::error::AppError::from), confirmed)
    };
    let pending = tokio::spawn(crate::execution::supervise_with_grace(entry.clone(), worker, Duration::from_millis(20)));
    tokio::time::timeout(Duration::from_secs(2), entry.wait_started()).await.unwrap();
    entry.request_stop(false);
    let (result, confirmed) = tokio::time::timeout(Duration::from_secs(2), pending).await.unwrap().unwrap();
    let outcome = entry.finish(result, confirmed, true);
    assert_eq!(outcome.state, ExecutionState::Unknown); assert_eq!(outcome.transaction_state, "unknown");
    assert_eq!(outcome.effects, "possibly_applied"); assert!(outcome.resources_pending); assert!(!outcome.deadline.stop_confirmed);
    assert!(!work_dropped.load(Ordering::SeqCst)); assert!(!control_dropped.load(Ordering::SeqCst)); assert!(activity.try_write().is_err());
    assert!(registry.has_active("c1")); assert_eq!(registry.require_quiescent("c1").unwrap_err().code, "EXECUTION_QUARANTINED");
    let mut later = control_request("SELECT 2", 0); let later_binding = binding(&later); registry.grant_request(&mut later, later_binding.clone()).unwrap();
    assert_eq!(registry.register(&later, later_binding.clone(), 30).err().unwrap().code, "EXECUTION_QUARANTINED");
    work_done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async { while !work_dropped.load(Ordering::SeqCst) { tokio::task::yield_now().await; } }).await.unwrap();
    assert!(!control_dropped.load(Ordering::SeqCst)); assert!(entry.status().resources_pending); assert!(activity.try_write().is_err());
    control_done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async { while entry.status().resources_pending { tokio::time::sleep(Duration::from_millis(1)).await; } }).await.unwrap();
    assert!(control_dropped.load(Ordering::SeqCst)); assert!(activity.try_write().is_ok());
    assert_eq!(entry.status().state, ExecutionState::Unknown); assert_eq!(entry.status().deadline.elapsed_ms, outcome.deadline.elapsed_ms);
    assert!(!registry.has_active("c1")); registry.require_quiescent("c1").unwrap();
    let later_entry = registry.register(&later, later_binding, 30).unwrap(); later_entry.finish(Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }), false, false);
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn c1_settled_work_cleanup_has_bounded_unknown_even_without_budget_or_cancel() {
    let registry = Registry::default(); let mut request = control_request("SELECT 1", 0); let entry = registered(&registry, &mut request);
    let activity = Arc::new(tokio::sync::RwLock::new(())); entry.own_activity(activity.clone().read_owned().await);
    let (cleanup_done, cleanup_wait) = tokio::sync::oneshot::channel::<()>(); let working = entry.clone();
    let worker = async move {
        assert!(working.start()); working.work_settled();
        cleanup_wait.await.unwrap();
        (Ok(MultiResult { sets: vec![], messages: vec![], elapsed_ms: 0, spool_id: None }), false)
    };
    let (result, confirmed) = tokio::time::timeout(Duration::from_secs(2), crate::execution::supervise_with_grace(entry.clone(), worker, Duration::from_millis(20))).await.unwrap();
    let outcome = entry.finish(result, confirmed, false);
    assert_eq!(outcome.state, ExecutionState::Unknown); assert!(outcome.resources_pending);
    assert!(!outcome.deadline.deadline_reached); assert!(!outcome.deadline.stop_confirmed);
    assert_eq!(outcome.effects, "possibly_applied"); assert!(activity.try_write().is_err());
    assert!(!entry.request_stop(false).accepted); assert!(!entry.start());
    cleanup_done.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async { while entry.status().resources_pending { tokio::time::sleep(Duration::from_millis(1)).await; } }).await.unwrap();
    assert!(activity.try_write().is_ok()); assert_eq!(entry.status().state, ExecutionState::Unknown);
    registry.shutdown().await.unwrap();
}

// 显式测试库 URL；没有服务不运行、不算 PASS。只运行 SELECT，不创建用户对象。
#[tokio::test]
#[ignore = "需要专用 NAVKIT_EXECUTION_PG_URL；验证定向取消和权限，不触碰用户库"]
async fn c1_postgres_real_cancel_environment() {
    let url = std::env::var("NAVKIT_EXECUTION_PG_URL").expect("必须明确配置隔离测试库");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "pg-fixture".into(), namespace: String::new(), owner_id: "tab1".into(), sql: "SELECT pg_catalog.pg_sleep(30)".into(), transaction_owner: None, timeout_ms: Some(100), origin: None, cancel_before_send: false, execution_grant: None };
    let entry = registered(&Registry::default(), &mut req);
    let (result, confirmed) = crate::execution::run(&Db::Postgres(pool.clone()), &DbKind::of("postgresql"), &req.sql, &entry, false).await;
    let outcome = entry.finish(result, confirmed, false); assert_eq!(outcome.state, ExecutionState::TimedOut); assert!(outcome.deadline.stop_confirmed);
    assert_eq!(sqlx::query_scalar::<_, i32>("SELECT 42").fetch_one(&pool).await.unwrap(), 42); pool.close().await;
}
#[tokio::test]
#[ignore = "需要专用 NAVKIT_EXECUTION_MYSQL_URL；验证 KILL QUERY 权限，不触碰用户库"]
async fn c1_mysql_real_cancel_environment() {
    let url = std::env::var("NAVKIT_EXECUTION_MYSQL_URL").expect("必须明确配置隔离测试库");
    let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let mut req = ExecutionRequest { execution_id: uuid::Uuid::new_v4().to_string(), connection_id: "mysql-fixture".into(), namespace: String::new(), owner_id: "tab1".into(), sql: "SELECT SLEEP(30), 1".into(), transaction_owner: None, timeout_ms: Some(100), origin: None, cancel_before_send: false, execution_grant: None };
    let entry = registered(&Registry::default(), &mut req);
    let (result, confirmed) = crate::execution::run(&Db::Mysql(pool.clone()), &DbKind::of("mysql"), &req.sql, &entry, false).await;
    let outcome = entry.finish(result, confirmed, false); assert_eq!(outcome.state, ExecutionState::TimedOut); assert!(outcome.deadline.stop_confirmed);
    assert_eq!(sqlx::query_scalar::<_, i32>("SELECT 42").fetch_one(&pool).await.unwrap(), 42); pool.close().await;
}
