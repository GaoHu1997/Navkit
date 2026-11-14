use super::*;
use std::collections::VecDeque;

struct Directory(std::path::PathBuf);
impl Directory { fn new() -> Self { let p = std::env::temp_dir().join(format!("navkit-restore-test-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&p).unwrap(); Self(p) }
    fn input(&self, sql: &str) -> String { let p = self.0.join("input.sql"); std::fs::write(&p, sql).unwrap(); p.to_str().unwrap().to_string() }
}
impl Drop for Directory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
struct Mock { sent: Vec<String>, failures: VecDeque<(String, bool)>, mysql: bool }
impl Mock { fn new(failure: Option<(&str, bool)>) -> Self { Self { sent: Vec::new(), failures: failure.map(|(s,c)| (s.into(),c)).into_iter().collect(), mysql: false } } }
impl Session for Mock {
    fn execute<'a>(&'a mut self, sql: &'a str) -> Receipt<'a> {
        Box::pin(async move { self.sent.push(sql.to_string());
            if self.failures.front().is_some_and(|(s,_)| sql.contains(s)) { let (_, certain) = self.failures.pop_front().unwrap(); Err(Failure { message: "injected".into(), certain }) } else { Ok(1) }
        })
    }
    fn rollback_proven(&self) -> bool { !self.mysql }
}
fn prepared(dir: &Directory, sql: &str) -> PreparedScript { PreparedScript::prepare_restore(&dir.input(sql), ScriptDialect::Sqlite, 4096, None, || Ok(())).unwrap() }

#[tokio::test]
async fn restore_1001_failure_keeps_committed_prefix_and_uses_actual_failure_index() {
    let dir = Directory::new();
    let sql = format!("{}INSERT INTO t VALUES(FAIL);INSERT INTO t VALUES(2);", "INSERT INTO t VALUES(1);".repeat(1000));
    let mut script = prepared(&dir, &sql); let mut session = Mock::new(Some(("FAIL", true)));
    let report = execute(&mut session, &mut script, true, None).await;
    assert_eq!(report.acked, 1000); assert_eq!(report.committed, 1000); assert_eq!(report.succeeded, 1000);
    assert_eq!(report.failed, 1); assert_eq!(report.first_failed_index, Some(1001)); assert_eq!(report.skipped, 1);
    assert_eq!(session.sent.iter().filter(|s| s.as_str() == "COMMIT").count(), 1);
    assert_eq!(session.sent.last().unwrap(), "ROLLBACK"); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_batch_rollback_acknowledged_is_not_succeeded_or_committed() {
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);INSERT INTO t VALUES(FAIL);INSERT INTO t VALUES(2);");
    let mut session = Mock::new(Some(("FAIL", true))); let report = execute(&mut session, &mut script, true, None).await;
    assert_eq!((report.acked, report.committed, report.succeeded, report.rolled_back, report.failed, report.skipped), (1,0,0,1,1,1));
    assert_eq!(report.steps[0].effect, "rolled_back"); assert!(!report.steps[0].ok); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_long_report_is_capped_and_late_failure_is_retained() {
    let dir = Directory::new(); let mut script = prepared(&dir, &format!("{}INSERT INTO t VALUES(FAIL);INSERT INTO t VALUES(2);", "INSERT INTO t VALUES(1);".repeat(5000)));
    let mut session = Mock::new(Some(("FAIL", true))); let report = execute(&mut session, &mut script, true, None).await;
    assert_eq!(report.steps.len(), REPORT_LIMIT); assert_eq!(report.omitted_steps, 5002 - REPORT_LIMIT);
    assert_eq!(report.first_failed_index, Some(5001)); assert_eq!(report.steps.last().unwrap().index, 5001);
    assert_eq!((report.acked,report.committed,report.skipped), (5000,5000,1)); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_commit_unknown_stops_without_replay() {
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);CREATE TABLE x(v);");
    let mut session = Mock::new(Some(("COMMIT", false))); let report = execute(&mut session, &mut script, true, None).await;
    assert!(report.effects_unknown); assert!(report.error.unwrap().contains("TRANSFER_EFFECTS_UNKNOWN"));
    assert_eq!((report.acked, report.committed, report.unknown, report.skipped), (1,0,1,1));
    assert!(!session.sent.iter().any(|s| s.contains("CREATE TABLE"))); assert_eq!(session.sent.iter().filter(|s| s.contains("INSERT")).count(), 1); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_mysql_rollback_is_not_proof_for_nontransactional_engines() {
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);INSERT INTO t VALUES(FAIL);");
    let mut session = Mock::new(Some(("FAIL", true))); session.mysql = true;
    let report = execute(&mut session, &mut script, false, None).await;
    assert!(report.effects_unknown); assert_eq!((report.acked,report.committed,report.rolled_back,report.unknown), (1,0,0,2)); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_non_dml_commit_and_continue_after_confirmed_rollback() {
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);CREATE TABLE x(v);INSERT INTO t VALUES(FAIL);INSERT INTO t VALUES(2);");
    let mut session = Mock::new(Some(("FAIL", true))); let report = execute(&mut session, &mut script, false, None).await;
    assert_eq!((report.committed,report.failed,report.skipped), (3,1,0));
    let ddl = session.sent.iter().position(|s| s.contains("CREATE TABLE")).unwrap(); assert_eq!(session.sent[ddl - 1], "COMMIT"); script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_late_bad_input_is_zero_sql_on_real_sqlite() {
    let dir = Directory::new(); let database = dir.0.join("target.db");
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&database).create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new().connect_with(opts).await.unwrap(); let db = Db::Sqlite(pool.clone());
    for tail in ["SELECT 'unterminated", "COMMIT;", "CREATE TRIGGER tr AFTER INSERT ON t BEGIN SELECT 1;"] {
        assert!(run(&db, &dir.input(&format!("CREATE TABLE t(v);{tail}")), true).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name='t'").fetch_one(&pool).await.unwrap(); assert_eq!(count, 0);
    }
    pool.close().await;
}
#[test]
fn restore_delimiter_guards_and_policy_are_independent_from_ordinary_import() {
    let dir = Directory::new(); let path = dir.input("SET NAMES utf8mb4;SET FOREIGN_KEY_CHECKS = 0;\nDELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT ';'; END$$\nDELIMITER ;\nSET FOREIGN_KEY_CHECKS = 1;");
    let mut script = PreparedScript::prepare_restore(&path, ScriptDialect::Mysql, 4096, None, || Ok(())).unwrap(); assert_eq!(script.count(), 4);
    assert!(PreparedScript::prepare(&path, "utf8", &crate::models::DbKind::of("mysql"), 4096, None, || Ok(())).is_err());
    while let Some(sql) = script.next_statement().unwrap() { assert!(!sql.contains("DELIMITER")); } script.cleanup().unwrap();
}
#[tokio::test]
async fn restore_sqlite_roundtrip_trigger_guard_and_finally_close() {
    let dir = Directory::new(); let database = dir.0.join("roundtrip.db");
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&database).create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap(); let db = Db::Sqlite(pool.clone());
    let report = run(&db, &dir.input("PRAGMA foreign_keys = OFF;CREATE TABLE t(v INTEGER UNIQUE);CREATE TABLE audit(v TEXT);CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(CASE WHEN NEW.v=1 THEN 'a; END' ELSE 'b' END);END;INSERT INTO t VALUES(1);INSERT INTO t VALUES(2);"), true).await.unwrap();
    assert_eq!((report.total,report.acked,report.committed,report.succeeded,report.failed), (6,6,6,6,0));
    let values: Vec<String> = sqlx::query_scalar("SELECT v FROM audit ORDER BY rowid").fetch_all(&pool).await.unwrap(); assert_eq!(values, vec!["a; END", "b"]);
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys").fetch_one(&pool).await.unwrap(); assert_eq!(fk, 1, "restore guard must not leak into pool physical session");
    let report = run(&db, &dir.input("INSERT INTO t VALUES(3);INSERT INTO t VALUES(3);"), true).await.unwrap();
    assert_eq!((report.acked,report.committed,report.rolled_back,report.failed), (1,0,1,1));
    sqlx::query("INSERT INTO t VALUES(4)").execute(&pool).await.unwrap(); // rollback + close released all write locks
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM t").fetch_one(&pool).await.unwrap(); assert_eq!(count, 3);
    pool.close().await;
}
#[tokio::test]
async fn restore_rollback_unknown_is_not_retried_and_pending_is_unknown() {
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);INSERT INTO t VALUES(FAIL);INSERT INTO t VALUES(2);");
    let mut session = Mock::new(Some(("FAIL", true))); session.failures.push_back(("ROLLBACK".into(), false));
    let report = execute(&mut session, &mut script, false, None).await;
    assert!(report.effects_unknown); assert_eq!((report.committed,report.rolled_back,report.unknown,report.skipped), (0,0,1,1));
    assert_eq!(session.sent.iter().filter(|s| s.as_str()=="ROLLBACK").count(), 1); script.cleanup().unwrap();
}
#[test]
fn restore_delimiter_must_start_its_own_line_and_bad_tail_is_preflight_error() {
    let dir = Directory::new();
    for sql in ["SELECT 1;DELIMITER $$\nSELECT 2$$", "DELIMITER --\nSELECT 1", "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END"] {
        assert!(PreparedScript::prepare_restore(&dir.input(sql), ScriptDialect::Mysql, 4096, None, || Ok(())).is_err(), "{sql}");
    }
}
#[tokio::test]
async fn restore_cancel_after_current_receipt_rolls_back_pending_and_does_not_send_next() {
    struct CancelSession { inner: Mock, id: String }
    impl Session for CancelSession {
        fn execute<'a>(&'a mut self, sql: &'a str) -> Receipt<'a> { Box::pin(async move {
            let result = self.inner.execute(sql).await;
            if sql.starts_with("INSERT") { super::super::cancel_task(&self.id).unwrap(); }
            result
        }) }
    }
    let dir = Directory::new(); let mut script = prepared(&dir, "INSERT INTO t VALUES(1);INSERT INTO t VALUES(2);");
    let id = format!("restore-cancel-{}", uuid::Uuid::new_v4()); let task = TaskControl::begin(&id).unwrap();
    let mut session = CancelSession { inner: Mock::new(None), id };
    let report = execute(&mut session, &mut script, true, Some(&task)).await;
    assert_eq!((report.acked,report.committed,report.rolled_back,report.skipped), (1,0,1,1));
    assert!(report.error.unwrap().contains("取消")); assert_eq!(session.inner.sent.last().unwrap(), "ROLLBACK");
    assert!(!session.inner.sent.iter().any(|s| s.contains("VALUES(2)"))); script.cleanup().unwrap();
    task.finish::<()>(&Err(anyhow!("cancelled")));
}
#[tokio::test]
async fn restore_soft_byte_budget_allows_large_single_statement_then_commits_before_next() {
    let dir = Directory::new(); let sql = format!("INSERT INTO t VALUES('{}');INSERT INTO t VALUES('next');", "x".repeat(SCRIPT_BATCH_BYTES + 1));
    let mut script = PreparedScript::prepare_restore(&dir.input(&sql), ScriptDialect::Sqlite, STATEMENT_LIMIT, None, || Ok(())).unwrap();
    let mut session = Mock::new(None); let report = execute(&mut session, &mut script, true, None).await;
    assert_eq!((report.total,report.committed,report.failed), (2,2,0));
    assert_eq!(session.sent.iter().filter(|s| s.as_str()=="COMMIT").count(), 2); script.cleanup().unwrap();
}
#[test]
fn restore_policy_compatibility_boundaries_fail_closed() {
    let dir = Directory::new();
    for sql in ["SET `autocommit`=0;", "SET statement_timeout=0;", "DO $$ BEGIN COMMIT; END $$;", "CALL p();", "CREATE FUNCTION f() RETURNS int LANGUAGE SQL BEGIN ATOMIC SELECT 1; END;", "CREATE FUNCTION f() RETURNS int AS $中文$ SELECT 1; $中文$ LANGUAGE SQL;"] {
        assert!(PreparedScript::prepare_restore(&dir.input(sql), ScriptDialect::Postgres, 4096, None, || Ok(())).is_err(), "{sql}");
    }
    let mut script = PreparedScript::prepare_restore(&dir.input("CREATE TABLE [t;name] (v int);INSERT INTO [t;name] VALUES(1);"), ScriptDialect::Generic, 4096, None, || Ok(())).unwrap(); assert_eq!(script.count(),2); script.cleanup().unwrap();
}
#[test]
fn restore_snapshot_is_immutable_and_spill_and_cancel_are_preflight_errors() {
    let dir = Directory::new(); let path = dir.input("INSERT INTO t VALUES(1);");
    let mut script = PreparedScript::prepare_restore(&path, ScriptDialect::Sqlite, 4096, None, || Ok(())).unwrap();
    std::fs::write(&path, "DELETE FROM t;").unwrap(); assert!(script.next_statement().unwrap().unwrap().contains("INSERT")); script.cleanup().unwrap();
    assert!(PreparedScript::prepare_restore(&path, ScriptDialect::Sqlite, 4096, Some("1"), || Ok(())).is_err());
    assert!(PreparedScript::prepare_restore(&path, ScriptDialect::Sqlite, 4096, None, || Err(anyhow!("cancel"))).is_err());
}

#[tokio::test]
async fn restore_public_task_reports_terminal_failure_not_false_completed() {
    let dir=Directory::new();
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.0.join("target.sqlite")).create_if_missing(true)).await.unwrap();
    sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY); INSERT INTO t VALUES(1)").execute(&pool).await.unwrap();
    let db=Db::Sqlite(pool.clone());
    let id=format!("restore-task-failed-{}",uuid::Uuid::new_v4());
    let report=super::super::restore_sql_file_task(&db,&dir.input("INSERT INTO t VALUES(2); INSERT INTO t VALUES(1);"),&id).await.unwrap();
    assert_eq!((report.failed,report.committed,report.rolled_back),(1,0,1));
    assert_eq!(super::super::task_progress(&id).unwrap().status,"failed");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM t").fetch_one(&pool).await.unwrap(),1);
    let success_id=format!("restore-task-success-{}",uuid::Uuid::new_v4());
    let report=super::super::restore_sql_file_task(&db,&dir.input("INSERT INTO t VALUES(3);"),&success_id).await.unwrap();
    assert_eq!(report.committed,1);assert_eq!(super::super::task_progress(&success_id).unwrap().status,"completed");
    assert!(super::super::restore_sql_file_task(&db,&dir.input("INSERT INTO t VALUES(4);"),&success_id).await.is_err(),"used task id cannot silently replay writes");
    pool.close().await;
}

#[tokio::test]
async fn restore_public_task_stops_tail_and_retains_only_confirmed_prefix() {
    let dir=Directory::new();
    let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.0.join("target.sqlite")).create_if_missing(true)).await.unwrap();
    sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY)").execute(&pool).await.unwrap();
    let path=dir.input(&(0..10000).map(|index|format!("INSERT INTO t VALUES({index});")).collect::<String>());
    let id=format!("restore-task-stop-{}",uuid::Uuid::new_v4());
    let running_id=id.clone();let db=Db::Sqlite(pool.clone());
    let work=tokio::spawn(async move{super::super::restore_sql_file_task(&db,&path,&running_id).await});
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            if let Ok(progress)=super::super::task_progress(&id) {
                if progress.processed>0 || progress.status!="running" {break;}
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }).await.unwrap();
    super::super::cancel_task(&id).unwrap();
    let report=tokio::time::timeout(std::time::Duration::from_secs(10),work).await.unwrap().unwrap().unwrap();
    let count=sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM t").fetch_one(&pool).await.unwrap();
    assert_eq!(count,report.committed as i64);
    let status=super::super::task_progress(&id).unwrap();
    if report.error.is_some() {assert!(report.error.as_deref().unwrap().contains("取消"));assert_eq!(status.status,"cancelled");assert!(count<10000);}
    else {assert_eq!(status.status,"completed");assert_eq!(count,10000,"a late stop cannot invalidate confirmed COMMIT");}
    assert!(!report.effects_unknown);pool.close().await;
}

#[tokio::test]
async fn restore_private_memory_target_fails_before_sql_and_preserves_original_pool() {
    let dir = Directory::new();
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(":memory:").in_memory(true).shared_cache(false);
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
    sqlx::raw_sql("CREATE TABLE original(v INTEGER); INSERT INTO original VALUES(7)").execute(&pool).await.unwrap();
    let db = Db::Sqlite(pool.clone());
    // Before the fix this succeeded on a different, disposable memory database.
    let error = run(&db, &dir.input("CREATE TABLE restored(v INTEGER); INSERT INTO restored VALUES(9);"), true).await.unwrap_err();
    assert!(error.to_string().contains("RESTORE_MEMORY_TARGET_UNSUPPORTED"));
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT v FROM original").fetch_one(&pool).await.unwrap(), 7);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_schema WHERE name='restored'").fetch_one(&pool).await.unwrap(), 0);
    sqlx::query("INSERT INTO original VALUES(8)").execute(&pool).await.unwrap();
    pool.close().await;
}
