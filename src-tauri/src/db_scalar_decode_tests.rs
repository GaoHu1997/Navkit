//! 标量解码回归：`COUNT(*)` 等数值列必须能被 `Db::sql_scalar` 读取。
//!
//! 背景（第四轮实测缺陷 A1）：MySQL 把 `COUNT(*)` 报为 `BIGINT`，而
//! `sql_scalar` 原先只用 `Option<String>` 解码，sqlx 的类型检查直接拒绝，
//! 导致查询页"总行数"永远统计中、跳页/末页按钮永久禁用。
//!
//! 这里用 SQLite 内存库覆盖真实解码路径（SQLite 的 `COUNT(*)` 返回 INTEGER，
//! 同样不属于 VARCHAR 兼容集合），并直接调用与桌面端相同的
//! `plan_query_count` + `sql_scalar` 组合。

use crate::db::Db;
use crate::query_pagination::plan_query_count;

async fn memory_db() -> Db {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("内存库连接");
    sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)")
        .execute(&pool)
        .await
        .expect("建表");
    for id in 1..=7 {
        sqlx::query("INSERT INTO items(id, name) VALUES(?, ?)")
            .bind(id)
            .bind(format!("row{id}"))
            .execute(&pool)
            .await
            .expect("插入");
    }
    Db::Sqlite(pool)
}

#[tokio::test]
async fn sql_scalar_reads_integer_count_column() {
    let db = memory_db().await;
    // 直接读 COUNT(*)：SQLite 返回 INTEGER，不是 VARCHAR。
    let value = db
        .sql_scalar("SELECT COUNT(*) FROM items")
        .await
        .expect("COUNT(*) 必须可解码");
    assert_eq!(value.as_deref(), Some("7"), "数值标量应转成十进制文本");

    if let Db::Sqlite(pool) = &db {
        pool.close().await;
    }
}

#[tokio::test]
async fn sql_scalar_still_reads_text_columns() {
    let db = memory_db().await;
    let value = db
        .sql_scalar("SELECT name FROM items WHERE id = 3")
        .await
        .expect("文本列解码不能回归");
    assert_eq!(value.as_deref(), Some("row3"));

    let empty = db
        .sql_scalar("SELECT name FROM items WHERE id = 999")
        .await
        .expect("空结果不是错误");
    assert_eq!(empty, None, "空结果应返回 None");

    if let Db::Sqlite(pool) = &db {
        pool.close().await;
    }
}

/// 端到端：查询页总数统计的完整链路（规划 COUNT SQL → 执行 → 解码）。
#[tokio::test]
async fn count_query_page_plan_and_scalar_roundtrip() {
    let db = memory_db().await;
    let kind = crate::db::DbKind::of("sqlite");
    let count_sql = plan_query_count(&kind, "SELECT * FROM items")
        .into_sql()
        .expect("普通单表查询应可规划精确总数");
    let total = db
        .sql_scalar(&count_sql)
        .await
        .expect("COUNT 规划结果必须可解码")
        .and_then(|value| value.trim().parse::<u64>().ok());
    assert_eq!(total, Some(7), "总数应为 7 且可解析为 u64");

    if let Db::Sqlite(pool) = &db {
        pool.close().await;
    }
}

/// 浮点与负数也要能解码（避免回退链只考虑正整数）。
#[tokio::test]
async fn sql_scalar_handles_float_and_negative_values() {
    let db = memory_db().await;
    let avg = db
        .sql_scalar("SELECT AVG(id) FROM items")
        .await
        .expect("AVG 应可解码");
    assert_eq!(avg.as_deref(), Some("4"), "AVG(1..7) = 4");

    let negative = db
        .sql_scalar("SELECT 0 - 5")
        .await
        .expect("负整数应可解码");
    assert_eq!(negative.as_deref(), Some("-5"));

    if let Db::Sqlite(pool) = &db {
        pool.close().await;
    }
}

/// 不可解码的查询仍要返回真实错误，不能静默变成 None。
#[tokio::test]
async fn sql_scalar_surfaces_real_error_for_missing_column() {
    let db = memory_db().await;
    let error = db
        .sql_scalar("SELECT no_such_column FROM items")
        .await
        .expect_err("不存在的列必须报错，而不是返回 None");
    let message = error.to_string();
    assert!(
        message.contains("no_such_column") || message.contains("no such column"),
        "错误信息应保留真实原因，实际：{message}"
    );

    if let Db::Sqlite(pool) = &db {
        pool.close().await;
    }
}
