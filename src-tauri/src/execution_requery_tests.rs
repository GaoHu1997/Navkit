//! 导出「重新查询全量」的隔离 SQLite 集成测试。
//!
//! 覆盖：可安全重跑的 SELECT 重新执行并导出全部行；写语句拒绝且不生成文件。
use super::*;

fn opts(root: &std::path::Path, file: &str) -> ExportOptions {
    ExportOptions {
        connection_id: "c1".into(),
        schema: "main".into(),
        table: "items".into(),
        file: root.join(file).to_string_lossy().into(),
        format: "csv".into(),
        has_header: true,
        delimiter: ",".into(),
        encoding: "utf8".into(),
        where_clause: None,
        limit: None,
    }
}

#[tokio::test]
async fn requery_export_safe_select_exports_all_rows() {
    let f = Fixture::new(false).await;
    sqlx::query("INSERT INTO items VALUES(2,'a'),(3,'b')")
        .execute(&f.observer)
        .await
        .unwrap();
    let file = f.root.join("requery.csv");
    let exported = crate::commands::export_query_requery_service(
        &f.state,
        opts(&f.root, "requery.csv"),
        "SELECT id,text FROM items ORDER BY id",
        "tab1",
        "main",
        "main",
    )
    .await
    .expect("可安全重跑的 SELECT 应成功导出");
    assert_eq!(exported, 3, "应导出全部 3 行（初始 1 行 + 新增 2 行）");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("原值") && text.contains('a') && text.contains('b'), "导出内容缺失：{text}");
    f.cleanup().await;
}

#[tokio::test]
async fn requery_export_rejects_write_statement_without_file() {
    let f = Fixture::new(false).await;
    let error = crate::commands::export_query_requery_service(
        &f.state,
        opts(&f.root, "bad.csv"),
        "DELETE FROM items",
        "tab1",
        "main",
        "main",
    )
    .await;
    assert!(error.is_err(), "写语句必须被拒绝重新查询导出");
    assert!(!f.root.join("bad.csv").exists(), "拒绝时不得生成导出文件");
    // 确认没有副作用：原行仍在。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(&f.observer)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.cleanup().await;
}

#[tokio::test]
async fn requery_export_rejects_multiple_statements() {
    let f = Fixture::new(false).await;
    let error = crate::commands::export_query_requery_service(
        &f.state,
        opts(&f.root, "multi.csv"),
        "SELECT 1; SELECT 2",
        "tab1",
        "main",
        "main",
    )
    .await;
    assert!(error.is_err(), "多语句必须被拒绝");
    assert!(!f.root.join("multi.csv").exists());
    f.cleanup().await;
}

#[tokio::test]
async fn requery_export_read_only_with_exports_all_rows() {
    let f = Fixture::new(false).await;
    let count = crate::commands::export_query_requery_service(
        &f.state, opts(&f.root, "with.csv"),
        "WITH x AS (WITH y AS (SELECT id,text FROM items) SELECT id,text FROM y) SELECT id,text FROM x",
        "tab1", "main", "main",
    ).await.unwrap();
    assert_eq!(count, 1);
    assert!(std::fs::read_to_string(f.root.join("with.csv")).unwrap().contains("原值"));
    f.cleanup().await;
}

#[tokio::test]
async fn requery_export_write_cte_is_rejected_without_sql_or_file() {
    let f = Fixture::new(false).await;
    let error = crate::commands::export_query_requery_service(
        &f.state, opts(&f.root, "write-cte.csv"),
        "WITH gone AS (DELETE FROM items RETURNING id) SELECT id FROM gone",
        "tab1", "main", "main",
    ).await.unwrap_err();
    assert_eq!(error.code, "EXPORT_REQUERY_UNSAFE");
    assert!(!f.root.join("write-cte.csv").exists());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    f.cleanup().await;
}

#[tokio::test]
async fn requery_export_rejects_active_manual_transaction_without_ending_it() {
    let f = Fixture::new(false).await;
    let conn = db::get_db(&f.state.registry, "c1").unwrap();
    {
        let mut lease = f.state.manual_transactions.lease("c1").await;
        lease.start(&conn, DbKind::of("sqlite"), "main".into(), "tab1".into()).await.unwrap();
        lease.bind_principal("main");
    }
    let error = crate::commands::export_query_requery_service(
        &f.state, opts(&f.root, "transaction.csv"), "SELECT * FROM items", "tab1", "main", "main",
    ).await.unwrap_err();
    assert_eq!(error.code, "EXPORT_REQUERY_TRANSACTION_ACTIVE");
    assert!(!f.root.join("transaction.csv").exists());
    {
        let mut lease = f.state.manual_transactions.lease("c1").await;
        assert!(lease.status().active);
        assert!(!lease.status().failed);
        lease.finish("tab1", false).await.unwrap();
    }
    drop(conn);
    f.cleanup().await;
}

#[tokio::test]
async fn query_keyset_text_primary_key_uses_real_type_not_numeric_looking_anchor() {
    let f = Fixture::new(false).await;
    sqlx::query("CREATE TABLE text_keys(id TEXT PRIMARY KEY NOT NULL); INSERT INTO text_keys VALUES('001'),('01'),('1'),('2')")
        .execute(&f.observer).await.unwrap();
    let capability = crate::commands::plan_query_keyset_service(
        &f.state, "c1", "SELECT id FROM text_keys ORDER BY id", Some("main"),
    ).await.unwrap();
    assert!(capability.supported);
    assert_eq!(capability.numeric, Some(false));
    let page = crate::commands::plan_query_keyset_page_service(
        &f.state, "c1", "SELECT id FROM text_keys ORDER BY id", "001", 10, Some("main"),
    ).await.unwrap();
    assert_eq!(page.mode, "page");
    let sql = page.sql.unwrap();
    assert!(sql.contains("'001'"), "TEXT anchor must stay quoted: {sql}");
    let ids: Vec<String> = sqlx::query_scalar(&sql).fetch_all(&f.observer).await.unwrap();
    assert_eq!(ids, vec!["01", "1", "2"]);
    f.cleanup().await;
}

#[tokio::test]
async fn query_keyset_page_revalidates_primary_key_after_capability_check() {
    let f = Fixture::new(false).await;
    let original = crate::commands::plan_query_keyset_service(
        &f.state, "c1", "SELECT id,text FROM items ORDER BY id", Some("main"),
    ).await.unwrap();
    assert!(original.supported);
    sqlx::query("DROP VIEW v; DROP TABLE items; CREATE TABLE items(id INTEGER, text TEXT)")
        .execute(&f.observer).await.unwrap();
    let page = crate::commands::plan_query_keyset_page_service(
        &f.state, "c1", "SELECT id,text FROM items ORDER BY id", "1", 10, Some("main"),
    ).await.unwrap();
    assert_eq!(page.mode, "unsupported");
    assert!(page.sql.is_none(), "stale unique-key evidence must not generate keyset SQL");
    f.cleanup().await;
}
