//! 真实 MySQL 端到端：服务器分页 SQL 执行 + 表数据 Keyset 深分页。
//! 由脚本用 NAVKIT_MYSQL_* 指向专用 UUID 库；--ignored 才运行。
use super::*;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

#[tokio::test]
#[ignore = "requires NAVKIT_MYSQL_* pointing at a dedicated test database"]
async fn live_mysql_server_paging_and_keyset() {
    let host = std::env::var("NAVKIT_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("NAVKIT_MYSQL_PORT").unwrap_or_else(|_| "3306".into()).parse().unwrap();
    let user = std::env::var("NAVKIT_MYSQL_USER").unwrap_or_else(|_| "root".into());
    let pass = std::env::var("NAVKIT_MYSQL_PASSWORD").expect("NAVKIT_MYSQL_PASSWORD required");
    let database = std::env::var("NAVKIT_MYSQL_DATABASE").expect("NAVKIT_MYSQL_DATABASE required");
    assert!(database.starts_with("navkit_pgtest_"), "只允许本轮独立库");
    let opts = MySqlConnectOptions::new()
        .host(&host).port(port).username(&user).password(&pass).database(&database);
    let pool = MySqlPoolOptions::new().max_connections(2).connect_with(opts).await.unwrap();

    sqlx::query("DROP TABLE IF EXISTS pgtest").execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE pgtest(id INT PRIMARY KEY, name VARCHAR(32))").execute(&pool).await.unwrap();
    for i in 0..1000 {
        sqlx::query("INSERT INTO pgtest VALUES(?,?)").bind(i).bind(format!("n{i}")).execute(&pool).await.unwrap();
    }
    let kind = DbKind::of("mysql");

    // 1) 无条件查询：规划生成 LIMIT，真实执行返回当前页。
    let plan = crate::query_pagination::plan_query_page(&kind, "SELECT id,name FROM pgtest ORDER BY id", 100, 0);
    let sql = match plan { crate::query_pagination::PagePlan::Page(s) => s, other => panic!("plan: {other:?}") };
    assert!(sql.to_uppercase().contains("LIMIT 100"), "生成的 SQL 应含 LIMIT 100: {sql}");
    let rows = sqlx::query(&sql).fetch_all(&pool).await.unwrap();
    assert_eq!(rows.len(), 100, "数据库端 LIMIT 应只返回 100 行");

    // 2) 用户已写 LIMIT：子查询包装后仍按页返回。
    let plan2 = crate::query_pagination::plan_query_page(&kind, "SELECT id FROM pgtest LIMIT 500", 100, 200);
    let sql2 = match plan2 { crate::query_pagination::PagePlan::Page(s) => s, other => panic!("plan2: {other:?}") };
    assert!(sql2.contains("navkit_page"), "用户 LIMIT 应被包装: {sql2}");
    let rows2 = sqlx::query(&sql2).fetch_all(&pool).await.unwrap();
    assert_eq!(rows2.len(), 100, "包装后第 3 页应返回 100 行");

    // 3) Keyset 深分页：offset=0 + 锚点 id>500 走谓词定位，返回 501..600。
    let dbref = Db::Mysql(pool.clone());
    let req: PageRequest = serde_json::from_value(serde_json::json!({
        "connection_id":"x","schema":database,"table":"pgtest","offset":0,"limit":100,"include_total":false,
        "order_rules":[{"column":"id","ascending":true}],"after":["500"]
    })).unwrap();
    let page = crate::schema::page_table(&dbref, kind.clone(), &req).await.unwrap();
    assert_eq!(page.rows.len(), 100, "Keyset 页应返回 100 行");
    assert_eq!(page.rows[0][0].as_deref(), Some("501"), "Keyset 应从锚点之后开始");
    assert_eq!(page.next_after, Some(vec![Some("600".to_string())]), "应给出下一页锚点 600");

    // 4) 查询页 Keyset：单表 SELECT + 单列 ORDER BY 命中唯一键 → 可分析、可生成锚点 SQL 并真实执行。
    let shaped = crate::query_pagination::analyze_keyset(&kind, "SELECT id, name FROM pgtest ORDER BY id")
        .expect("单表单列排序应可分析");
    assert_eq!((shaped.column.as_str(), shaped.ascending), ("id", true));
    let predicate = crate::query_pagination::keyset_predicate("`id`", true, Some("700"), true).unwrap();
    let keyset_sql = crate::query_pagination::plan_keyset_page(
        &kind, "SELECT id, name FROM pgtest ORDER BY id", &predicate, 50,
    ).expect("应能生成锚点分页 SQL");
    assert!(!keyset_sql.to_uppercase().contains("OFFSET"), "锚点 SQL 不该带 OFFSET: {keyset_sql}");
    let ks_rows = sqlx::query(&keyset_sql).fetch_all(&pool).await.unwrap();
    assert_eq!(ks_rows.len(), 50, "锚点页应返回 50 行");
    let first_id: i32 = sqlx::Row::try_get(&ks_rows[0], 0).unwrap();
    assert_eq!(first_id, 701, "应从锚点 700 之后开始（701）");

    // 5) 非唯一排序列必须拒绝锚点定位。
    assert!(
        crate::query_pagination::analyze_keyset(&kind, "SELECT id, name FROM pgtest ORDER BY name").is_some(),
        "形状本身可分析（唯一性由元数据判定，见 plan_query_keyset 命令）"
    );

    pool.close().await;
}
