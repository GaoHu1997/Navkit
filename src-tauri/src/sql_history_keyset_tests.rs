//! 表数据 Keyset 连续翻页：有唯一主键锚点时不用 OFFSET；无锚点或复合排序回退 OFFSET。
use super::*;

#[tokio::test]
async fn keyset_continuation_uses_anchor_without_offset() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)")
        .execute(f.pool())
        .await
        .unwrap();
    query("INSERT INTO items VALUES(1,'a'),(2,'b'),(3,'c'),(4,'d'),(5,'e')")
        .execute(f.pool())
        .await
        .unwrap();
    let req: PageRequest = serde_json::from_value(serde_json::json!({
        "connection_id":"history-test","schema":"main","table":"items",
        "offset":0,"limit":2,"include_total":false,
        "order_rules":[{"column":"id","ascending":true}],
        "after":["2"]
    }))
    .unwrap();
    crate::storage::clear_sql_log_at(&f.path);
    let result = page(&f, &req).await;
    assert_eq!(
        result.rows.iter().map(|r| r[0].as_deref().unwrap()).collect::<Vec<_>>(),
        vec!["3", "4"]
    );
    assert_eq!(result.next_after, Some(vec![Some("4".to_string())]));
    let logs = f.logs();
    let rows_sql = logs.iter().find(|e| e.sql.starts_with("SELECT * FROM")).unwrap();
    assert!(rows_sql.sql.contains("\"id\" > 2"), "Keyset SQL 应含 id > 2 谓词: {}", rows_sql.sql);
    // 谓词已定位起点，OFFSET 恒为 0（零跳过成本），不是深 OFFSET。
    assert!(rows_sql.sql.to_uppercase().contains("OFFSET 0"), "Keyset 时 OFFSET 应为 0: {}", rows_sql.sql);
    assert!(!rows_sql.sql.to_uppercase().contains("OFFSET 2"), "不应出现深 OFFSET: {}", rows_sql.sql);
}

#[tokio::test]
async fn without_anchor_falls_back_to_offset() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)")
        .execute(f.pool())
        .await
        .unwrap();
    query("INSERT INTO items VALUES(1,'a'),(2,'b'),(3,'c'),(4,'d')")
        .execute(f.pool())
        .await
        .unwrap();
    let req: PageRequest = serde_json::from_value(serde_json::json!({
        "connection_id":"history-test","schema":"main","table":"items",
        "offset":2,"limit":2,"include_total":false,
        "order_rules":[{"column":"id","ascending":true}]
    }))
    .unwrap();
    crate::storage::clear_sql_log_at(&f.path);
    let result = page(&f, &req).await;
    assert_eq!(
        result.rows.iter().map(|r| r[0].as_deref().unwrap()).collect::<Vec<_>>(),
        vec!["3", "4"]
    );
    assert!(result.next_after.is_none());
    let logs = f.logs();
    let rows_sql = logs.iter().find(|e| e.sql.starts_with("SELECT * FROM")).unwrap();
    assert!(rows_sql.sql.contains("LIMIT 3 OFFSET 2"), "无锚点应回退 OFFSET: {}", rows_sql.sql);
}

#[tokio::test]
async fn composite_order_does_not_use_keyset() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)")
        .execute(f.pool())
        .await
        .unwrap();
    query("INSERT INTO items VALUES(1,'a'),(2,'b'),(3,'c')")
        .execute(f.pool())
        .await
        .unwrap();
    let req: PageRequest = serde_json::from_value(serde_json::json!({
        "connection_id":"history-test","schema":"main","table":"items",
        "offset":0,"limit":2,"include_total":false,
        "order_rules":[{"column":"id","ascending":true},{"column":"name","ascending":true}],
        "after":["2","b"]
    }))
    .unwrap();
    crate::storage::clear_sql_log_at(&f.path);
    let _ = page(&f, &req).await;
    let logs = f.logs();
    let rows_sql = logs.iter().find(|e| e.sql.starts_with("SELECT * FROM")).unwrap();
    assert!(
        rows_sql.sql.to_uppercase().contains("OFFSET"),
        "复合排序应回退 OFFSET: {}",
        rows_sql.sql
    );
}
