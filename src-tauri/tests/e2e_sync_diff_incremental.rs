//! 端到端集成测试：两个真实 SQLite 文件库之间走完 diff / 全量 / 增量同步的完整链路。
//! 不是 mock：SQL 生成、执行、数据比较全部走真实数据库。
use std::path::PathBuf;
use std::sync::Arc;

use sqlx::Row;

use navkit_lib::db::{self, Db};
use navkit_lib::models::DbKind;
use navkit_lib::sync::{
    self, DbSyncRequest, DbSyncTable, SyncDiffRequest, SyncOptions,
};

struct Fixture {
    dir: PathBuf,
    source: Arc<Db>,
    target: Arc<Db>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn open(path: &std::path::Path) -> sqlx::SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    sqlx::pool::PoolOptions::new().max_connections(2).connect_with(options).await.unwrap()
}

/// 建一对独立的源/目标 SQLite 文件库；ddl 在两边都执行。
async fn fixture(ddl: &[&str]) -> Fixture {
    let dir = std::env::temp_dir().join(format!("navkit-sync-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = open(&dir.join("source.db")).await;
    let target = open(&dir.join("target.db")).await;
    for sql in ddl {
        sqlx::query(sql).execute(&source).await.unwrap();
        sqlx::query(sql).execute(&target).await.unwrap();
    }
    Fixture { dir, source: Arc::new(Db::Sqlite(source)), target: Arc::new(Db::Sqlite(target)) }
}

/// 整库同步用：ddl 只建在源库，目标库保持全空（验证自动建表路径）。
async fn fixture_source_only(ddl: &[&str]) -> Fixture {
    let dir = std::env::temp_dir().join(format!("navkit-sync-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = open(&dir.join("source.db")).await;
    let target = open(&dir.join("target.db")).await;
    for sql in ddl {
        sqlx::query(sql).execute(&source).await.unwrap();
    }
    Fixture { dir, source: Arc::new(Db::Sqlite(source)), target: Arc::new(Db::Sqlite(target)) }
}

fn kind() -> DbKind {
    DbKind::of("sqlite")
}

async fn target_rows(fx: &Fixture, sql: &str) -> Vec<(i64, String, f64)> {
    let Db::Sqlite(pool) = fx.target.as_ref() else { unreachable!() };
    let rows = sqlx::query(sql).fetch_all(pool).await.unwrap();
    rows.into_iter()
        .map(|r| (r.try_get::<i64, _>(0).unwrap(), r.try_get::<String, _>(1).unwrap(), r.try_get::<f64, _>(2).unwrap()))
        .collect()
}

const DDL: &[&str] = &["CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, amount REAL NOT NULL)"];

fn options(sync_mode: &str, incremental_update: bool) -> SyncOptions {
    SyncOptions {
        task_id: None,
        source_connection: "src".into(),
        source_schema: "main".into(),
        source_table: "t".into(),
        target_connection: "dst".into(),
        target_schema: "main".into(),
        target_table: "t".into(),
        mode: "both".into(),
        write_mode: "append".into(),
        sync_mode: sync_mode.into(),
        incremental_update,
        create_target: false,
        columns: vec![],
        key_columns: Vec::new(), delete_missing: false, where_clause: None,
    }
}

/// 目标库预置：id 1-5，其中 id3 amount 被改、id4 name 被改；另有多出的 id99。
async fn seed_partial_target(fx: &Fixture) {
    let Db::Sqlite(pool) = fx.target.as_ref() else { unreachable!() };
    for id in 1..=5 {
        let (name, amount): (String, f64) = match id {
            3 => ("n3".into(), 3.5),      // amount 与源(3.0)不同
            4 => ("changed4".into(), 4.0), // name 与源(n4)不同
            _ => (format!("n{id}"), id as f64),
        };
        sqlx::query("INSERT INTO t (id, name, amount) VALUES (?, ?, ?)")
            .bind(id).bind(&name).bind(amount).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO t (id, name, amount) VALUES (99, 'extra', 99.0)")
        .execute(pool).await.unwrap();
}

async fn seed_source(fx: &Fixture) {
    let Db::Sqlite(pool) = fx.source.as_ref() else { unreachable!() };
    for id in 1..=10i64 {
        sqlx::query("INSERT INTO t (id, name, amount) VALUES (?, ?, ?)")
            .bind(id).bind(format!("n{id}")).bind(id as f64).execute(pool).await.unwrap();
    }
}

#[tokio::test]
async fn diff_reports_missing_extra_changed() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    seed_partial_target(&fx).await;
    let report = sync::diff(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &SyncDiffRequest {
        source_connection: "src".into(), source_schema: "main".into(), source_table: "t".into(),
        target_connection: "dst".into(), target_schema: "main".into(), target_table: "t".into(),
        where_clause: None, columns: vec![],
    }).await.unwrap();
    assert_eq!(report.source_rows, 10);
    assert_eq!(report.target_rows, 6);
    assert!(report.keyed);
    // 源 6..=10 目标缺失
    assert_eq!(report.missing_in_target, 5);
    // 目标多出 id99
    assert_eq!(report.extra_in_target, 1);
    // id3 amount、id4 name 内容不同
    assert_eq!(report.changed, 2);
    assert!(report.target_missing_columns.is_empty() && report.target_extra_columns.is_empty());
    assert!(report.message.is_none());
}

#[tokio::test]
async fn diff_without_primary_key_only_counts_rows() {
    let ddl = ["CREATE TABLE t (a TEXT, b TEXT)"];
    let fx = fixture(&ddl).await;
    sqlx::query("INSERT INTO t VALUES ('1','x')").execute(fx.source_sqlite()).await.unwrap();
    sqlx::query("INSERT INTO t VALUES ('1','x')").execute(fx.source_sqlite()).await.unwrap();
    let report = sync::diff(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &SyncDiffRequest {
        source_connection: "src".into(), source_schema: "main".into(), source_table: "t".into(),
        target_connection: "dst".into(), target_schema: "main".into(), target_table: "t".into(),
        where_clause: None, columns: vec![],
    }).await.unwrap();
    assert!(!report.keyed);
    assert_eq!(report.source_rows, 2);
    assert!(report.message.as_deref().unwrap_or("").contains("没有主键"));
}

impl Fixture {
    fn source_sqlite(&self) -> &sqlx::SqlitePool {
        let Db::Sqlite(pool) = self.source.as_ref() else { unreachable!() };
        pool
    }
}

#[tokio::test]
async fn diff_when_target_table_missing() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    // 目标库只有 DDL 没有数据？—— 直接建一个没有 t 表的目标库场景：另开 fixture 再 drop
    sqlx::query("DROP TABLE t").execute(fx.target_sqlite()).await.unwrap();
    let report = sync::diff(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &SyncDiffRequest {
        source_connection: "src".into(), source_schema: "main".into(), source_table: "t".into(),
        target_connection: "dst".into(), target_schema: "main".into(), target_table: "t".into(),
        where_clause: None, columns: vec![],
    }).await.unwrap();
    assert_eq!(report.source_rows, 10);
    assert_eq!(report.target_rows, 0);
    assert_eq!(report.missing_in_target, 10);
    assert_eq!(report.target_missing_columns.len(), 3);
    assert!(report.message.as_deref().unwrap_or("").contains("不存在"));
}

impl Fixture {
    fn target_sqlite(&self) -> &sqlx::SqlitePool {
        let Db::Sqlite(pool) = self.target.as_ref() else { unreachable!() };
        pool
    }
}

#[tokio::test]
async fn incremental_inserts_missing_only_and_updates_changed() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    seed_partial_target(&fx).await;
    let report = sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &options("incremental", true)).await.unwrap();
    // 5 条插入 + 2 条更新
    assert_eq!(report.rows, 7, "增量应恰好执行 5 INSERT + 2 UPDATE");
    let rows = target_rows(&fx, "SELECT id, name, amount FROM t ORDER BY id").await;
    // 目标多出的 id99 保留；缺失的 6..=10 插入；共 11 行
    assert_eq!(rows.len(), 11);
    assert!(rows.iter().any(|(id, _, _)| *id == 99));
    // id3 amount 被更新回源的 3.0；id4 name 被更新回 n4
    let id3 = rows.iter().find(|(id, _, _)| *id == 3).unwrap();
    assert_eq!(id3.2, 3.0, "增量更新应把 amount 修正为源值");
    let id4 = rows.iter().find(|(id, _, _)| *id == 4).unwrap();
    assert_eq!(id4.1, "n4", "增量更新应把 name 修正为源值");
    let id10 = rows.iter().find(|(id, _, _)| *id == 10).unwrap();
    assert_eq!((id10.1.as_str(), id10.2), ("n10", 10.0));
}

#[tokio::test]
async fn incremental_without_update_keeps_changed_rows() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    seed_partial_target(&fx).await;
    let report = sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &options("incremental", false)).await.unwrap();
    assert_eq!(report.rows, 5, "不更新时只执行 5 条 INSERT");
    let rows = target_rows(&fx, "SELECT id, name, amount FROM t ORDER BY id").await;
    assert_eq!(rows.len(), 11);
    let id3 = rows.iter().find(|(id, _, _)| *id == 3).unwrap();
    assert_eq!(id3.2, 3.5, "未开启增量更新时目标值应保持不变");
}

#[tokio::test]
async fn incremental_on_empty_target_inserts_everything_without_pk_check() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    let report = sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &options("incremental", true)).await.unwrap();
    assert_eq!(report.rows, 10);
    let rows = target_rows(&fx, "SELECT id, name, amount FROM t").await;
    assert_eq!(rows.len(), 10);
}

#[tokio::test]
async fn incremental_requires_primary_key_when_target_has_rows() {
    let ddl = ["CREATE TABLE t (a TEXT, b TEXT)"];
    let fx = fixture(&ddl).await;
    sqlx::query("INSERT INTO t VALUES ('s1','v')").execute(fx.source_sqlite()).await.unwrap();
    sqlx::query("INSERT INTO t VALUES ('t1','v')").execute(fx.target_sqlite()).await.unwrap();
    let error = sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &options("incremental", true)).await.unwrap_err();
    assert!(format!("{error:#}").contains("主键"), "实际错误：{error:#}");
}

#[tokio::test]
async fn incremental_rejects_replace_write_mode() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    seed_partial_target(&fx).await;
    let mut opts = options("incremental", false);
    opts.write_mode = "replace".into();
    let error = sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &opts).await.unwrap_err();
    assert!(format!("{error:#}").contains("增量"), "实际错误：{error:#}");
}

#[tokio::test]
async fn full_replace_still_works() {
    let fx = fixture(DDL).await;
    seed_source(&fx).await;
    seed_partial_target(&fx).await;
    let mut opts = options("full", false);
    opts.write_mode = "replace".into();
    sync::synchronize(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &opts).await.unwrap();
    let rows = target_rows(&fx, "SELECT id, name, amount FROM t ORDER BY id").await;
    // 清空后写入：恰好 10 行，id99 与旧值都被覆盖
    assert_eq!(rows.len(), 10);
    assert!(!rows.iter().any(|(id, _, _)| *id == 99));
    let id3 = rows.iter().find(|(id, _, _)| *id == 3).unwrap();
    assert_eq!(id3.2, 3.0);
}

/// 整库端到端：目标库全空，两张表 + 一个视图；视图物化为普通表。
#[tokio::test]
async fn whole_database_sync_with_view_materialization() {
    let ddl = [
        "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT)",
        "CREATE TABLE b (id INTEGER PRIMARY KEY, w TEXT)",
        "CREATE VIEW va AS SELECT id, v FROM a WHERE id > 1",
    ];
    let fx = fixture_source_only(&ddl).await;
    sqlx::query("INSERT INTO a VALUES (1,'x1'),(2,'x2'),(3,'x3')").execute(fx.source_sqlite()).await.unwrap();
    sqlx::query("INSERT INTO b VALUES (1,'y1')").execute(fx.source_sqlite()).await.unwrap();
    let req = DbSyncRequest {
        task_id: None,
        source_connection: "src".into(), source_schema: "main".into(),
        target_connection: "dst".into(), target_schema: "main".into(),
        mode: "both".into(), write_mode: "skip".into(),
        sync_mode: "incremental".into(), incremental_update: false,
        tables: vec![
            DbSyncTable { source: "a".into(), target: String::new(), view: false },
            DbSyncTable { source: "b".into(), target: String::new(), view: false },
            DbSyncTable { source: "va".into(), target: "va_mat".into(), view: true },
        ],
        where_clause: None,
    };
    let reports = sync::synchronize_database(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &req).await.unwrap();
    assert_eq!(reports.len(), 3);
    assert!(reports.iter().all(|r| r.error.is_none()), "全部表/视图应成功：{:?}", reports);
    assert!(reports[0].created && reports[1].created && reports[2].created);
    // 视图物化提示
    assert!(reports[2].warnings.iter().any(|w| w.contains("物化")), "视图应有物化提示：{:?}", reports[2].warnings);
    let Db::Sqlite(pool) = fx.target.as_ref() else { unreachable!() };
    let a: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM a").fetch_one(pool).await.unwrap();
    let b: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM b").fetch_one(pool).await.unwrap();
    let va: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM va_mat").fetch_one(pool).await.unwrap();
    assert_eq!((a, b, va), (3, 1, 2), "视图应只物化 2 行（id>1）");
    // 目标里 va_mat 是普通表（可以再插入），不是视图
    sqlx::query("INSERT INTO va_mat VALUES (9,'x9')").execute(pool).await.unwrap();
    // 重复整库同步 + skip：目标已有同名表全部跳过
    let again = sync::synchronize_database(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &req).await.unwrap();
    assert!(again.iter().all(|r| r.skipped), "skip 模式下第二遍应全部跳过");
}

/// diff 中的映射列：目标列名不同也能按映射比较。
#[tokio::test]
async fn diff_with_column_mapping() {
    let source_ddl = ["CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)"];
    let target_ddl = ["CREATE TABLE t2 (pk INTEGER PRIMARY KEY, label TEXT)"];
    let dir = std::env::temp_dir().join(format!("navkit-sync-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = open(&dir.join("s.db")).await;
    let target = open(&dir.join("t.db")).await;
    for sql in source_ddl { sqlx::query(sql).execute(&source).await.unwrap(); }
    for sql in target_ddl { sqlx::query(sql).execute(&target).await.unwrap(); }
    sqlx::query("INSERT INTO t VALUES (1,'a'),(2,'b')").execute(&source).await.unwrap();
    sqlx::query("INSERT INTO t2 VALUES (1,'a'),(3,'c')").execute(&target).await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let fx = Fixture { dir, source: Arc::new(Db::Sqlite(source)), target: Arc::new(Db::Sqlite(target)) };
    let columns = vec![
        navkit_lib::sync::SyncColumnMapping { source: "id".into(), target: "pk".into(), target_type: None },
        navkit_lib::sync::SyncColumnMapping { source: "name".into(), target: "label".into(), target_type: None },
    ];
    let report = sync::diff(fx.source.as_ref(), &kind(), fx.target.as_ref(), &kind(), &SyncDiffRequest {
        source_connection: "src".into(), source_schema: "main".into(), source_table: "t".into(),
        target_connection: "dst".into(), target_schema: "main".into(), target_table: "t2".into(),
        where_clause: None, columns,
    }).await.unwrap();
    assert!(report.keyed);
    assert_eq!(report.missing_in_target, 1, "源 id2 目标缺失");
    assert_eq!(report.extra_in_target, 1, "目标 pk3 源缺失");
    assert_eq!(report.changed, 0);
}

/// 防呆：源表与目标表是同一张表（同一文件库）时拒绝同步。
#[tokio::test]
async fn same_database_sync_rejected() {
    let fx = fixture(DDL).await;
    let opts = options("incremental", false);
    // target 指向同一个池的同一个表
    let result = sync::synchronize(fx.source.as_ref(), &kind(), fx.source.as_ref(), &kind(), &opts).await;
    assert!(result.is_err());
    let _ = db::qualified(&kind(), "main", "t");
}
