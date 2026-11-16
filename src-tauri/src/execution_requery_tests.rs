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
