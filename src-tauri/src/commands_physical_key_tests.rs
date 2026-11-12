//! 端点校验必须在“登记实际使用的那条物理池记录”上进行。
//! MySQL 的库级读取返回 `id@database` 派生池；若沿用基础 `id` 键，
//! connection_runtime.matches 会因 Arc 指针不同而误判端点已变，
//! 表设计/清空表在读取基线时即报 TABLE_RUNTIME_CHANGED。
use super::physical_key;
use crate::db::runtime::RuntimeRegistry;
use std::sync::Arc;

// connect_lazy 需要 Tokio 上下文，但不建立网络连接，只用于构造可登记/比较的池对象。
fn lazy_mysql() -> Arc<crate::db::Db> {
    let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(1)
        .connect_lazy("mysql://user:pass@127.0.0.1:1/db").unwrap();
    Arc::new(crate::db::Db::Mysql(pool))
}

fn lazy_sqlite() -> Arc<crate::db::Db> {
    Arc::new(crate::db::Db::Sqlite(
        sqlx::sqlite::SqlitePoolOptions::new().connect_lazy("sqlite::memory:").unwrap(),
    ))
}

#[tokio::test]
async fn mysql_database_pool_is_matched_under_derived_key() {
    let db = lazy_mysql();
    let runtime = RuntimeRegistry::default();
    runtime.publish_derived("c1@sales", "fp", &db, None).unwrap();
    // 派生池登记在 id@database；用它自己推导出的键才能证明端点未变。
    assert!(runtime.matches(&physical_key("c1", "sales", db.as_ref()), "fp", &db));
    // 旧逻辑用基础 id 比对，指向的是另一条记录，必然失败。
    assert!(!runtime.matches("c1", "fp", &db), "基础 id 不能代表 MySQL 库级派生池");
}

#[tokio::test]
async fn mysql_connection_level_key_stays_base() {
    let db = lazy_mysql();
    assert_eq!(physical_key("c1", "", db.as_ref()), "c1");
}

#[tokio::test]
async fn sqlite_scoped_read_stays_on_base_pool() {
    let db = lazy_sqlite();
    let runtime = RuntimeRegistry::default();
    runtime.publish_derived("c1", "fp", &db, None).unwrap();
    assert_eq!(physical_key("c1", "main", db.as_ref()), "c1");
    assert!(runtime.matches(&physical_key("c1", "main", db.as_ref()), "fp", &db));
}
