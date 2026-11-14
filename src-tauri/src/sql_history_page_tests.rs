use super::*;

#[path = "sql_history_page_generic_tests.rs"]
mod generic_tests;

fn request(table: &str, offset: u64, limit: u64, include_total: bool) -> PageRequest {
    serde_json::from_value(serde_json::json!({
        "connection_id":"history-test", "schema":"main", "table":table,
        "offset":offset, "limit":limit, "include_total":include_total,
        "order_by":"id", "ascending":true
    })).unwrap()
}
async fn page(f: &Fixture, req: &PageRequest) -> crate::models::PageResult {
    crate::schema::page_table(f.db.as_ref(), DbKind::of("sqlite"), req).await.unwrap()
}
fn no_count(f: &Fixture) -> Vec<SqlLogEntry> {
    let logs = f.logs();
    assert!(!logs.is_empty());
    assert!(logs.iter().all(|e| !e.sql.to_uppercase().contains("COUNT(")), "{:?}", logs.iter().map(|e| &e.sql).collect::<Vec<_>>());
    assert!(logs.iter().all(|e| e.ok));
    logs
}

#[tokio::test]
async fn page_no_count_edges_preserve_metadata_and_probe_real_sql() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("INSERT INTO items VALUES(1,'alpha'),(2,NULL),(3,'gamma'),(4,'delta'),(5,'epsilon')").execute(f.pool()).await.unwrap();
    query("CREATE TABLE empty_items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    for (table, offset, limit, n, total, exact, more) in [
        ("items", 0, 2, 2, 3, false, Some(true)),
        ("items", 2, 2, 2, 5, false, Some(true)),
        ("items", 4, 2, 1, 5, true, Some(false)),
        ("items", 3, 2, 2, 5, true, Some(false)),
        ("items", 99, 2, 0, 0, false, Some(false)),
        ("items", 0, 0, 0, 0, false, None),
        ("items", 99, 0, 0, 0, false, None),
        ("empty_items", 0, 2, 0, 0, true, Some(false)),
    ] {
        crate::storage::clear_sql_log_at(&f.path);
        let req = request(table, offset, limit, false);
        let result = page(&f, &req).await;
        assert_eq!((result.rows.len(), result.total, result.total_exact, result.has_more), (n, total, exact, more), "{table}/{offset}/{limit}");
        assert_eq!(result.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["id", "name"]);
        assert_eq!(result.columns[0].data_type, "INTEGER");
        assert!(result.columns[0].primary_key);
        assert_eq!(result.primary_keys, vec!["id"]);
        if offset == 0 && limit == 2 && table == "items" { assert_eq!(result.rows[1][1], None); }
        let logs = no_count(&f);
        let rows = logs.iter().find(|e| e.sql.starts_with("SELECT * FROM")).unwrap();
        if limit == 0 {
            assert!(rows.sql.contains("1=0"));
            assert_eq!(rows.rows, 0);
        } else {
            assert!(rows.sql.contains(&format!("LIMIT {} OFFSET {}", limit + 1, offset)));
            assert!(rows.rows <= (limit + 1) as i64);
        }
    }
    let mut req = request("items", 0, 2, false);
    req.filters = serde_json::from_value(serde_json::json!([{"conds":[{"column":"id", "operator":">", "value":"2"}]}])).unwrap();
    req.ascending = false;
    crate::storage::clear_sql_log_at(&f.path);
    let result = page(&f, &req).await;
    assert_eq!(result.rows.iter().map(|r| r[0].as_deref().unwrap()).collect::<Vec<_>>(), vec!["5", "4"]);
    assert_eq!((result.total, result.total_exact, result.has_more), (3, false, Some(true)));
    let logs = no_count(&f);
    assert!(logs.iter().any(|e| e.sql.starts_with("SELECT * FROM") && e.sql.contains("WHERE") && e.sql.contains("DESC") && e.sql.contains("LIMIT 3 OFFSET 0")));
    req.filters[0].conds[0].value = "100".into();
    crate::storage::clear_sql_log_at(&f.path);
    let result = page(&f, &req).await;
    assert!(result.rows.is_empty());
    assert_eq!((result.total, result.total_exact, result.has_more), (0, true, Some(false)));
    assert_eq!(result.columns.len(), 2);
    no_count(&f);
}

#[tokio::test]
async fn page_count_compatibility_and_explicit_count_only() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("INSERT INTO items VALUES(1,'alpha'),(2,'beta'),(3,'gamma')").execute(f.pool()).await.unwrap();
    for (offset, limit, expected_rows, more) in [(0, 2, 2, true), (2, 2, 1, false), (99, 2, 0, false)] {
        crate::storage::clear_sql_log_at(&f.path);
        let result = page(&f, &request("items", offset, limit, true)).await;
        assert_eq!((result.rows.len(), result.total, result.total_exact, result.has_more), (expected_rows, 3, true, Some(more)));
        assert_eq!(f.logs().iter().filter(|e| e.sql.starts_with("SELECT COUNT(*) FROM")).count(), 1);
        assert!(f.logs().iter().any(|e| e.sql.contains(&format!("LIMIT {limit} OFFSET {offset}"))));
    }
    crate::storage::clear_sql_log_at(&f.path);
    let result = page(&f, &request("items", 0, 0, true)).await;
    assert_eq!((result.rows.len(), result.total, result.total_exact, result.has_more), (0, 3, true, None));
    assert_eq!(result.columns.len(), 2);
    let logs = f.logs();
    assert_eq!(logs.iter().filter(|e| e.sql.starts_with("SELECT COUNT(*) FROM")).count(), 1);
    assert!(logs.iter().any(|e| e.sql.starts_with("SELECT * FROM") && e.sql.contains("1=0") && e.rows == 0));
}

#[tokio::test]
async fn page_attached_and_temp_namespaces_preserve_identity() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("INSERT INTO items VALUES(1,'main')").execute(f.pool()).await.unwrap();
    query("CREATE TEMP TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("INSERT INTO temp.items VALUES(2,'temp')").execute(f.pool()).await.unwrap();
    query("ATTACH DATABASE ':memory:' AS other").execute(f.pool()).await.unwrap();
    query("CREATE TABLE other.items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("INSERT INTO other.items VALUES(3,'attached')").execute(f.pool()).await.unwrap();
    for (namespace, value) in [("main", "main"), ("temp", "temp"), ("other", "attached")] {
        let mut req = request("items", 0, 20, false);
        req.schema = namespace.into();
        crate::storage::clear_sql_log_at(&f.path);
        let result = page(&f, &req).await;
        assert_eq!(result.rows[0][1].as_deref(), Some(value));
        assert_eq!(result.primary_keys, vec!["id"]);
        assert_eq!((result.total, result.total_exact), (1, true));
        no_count(&f);
    }
}

#[tokio::test]
async fn page_million_sqlite_measures_count_and_no_count() {
    let f = Fixture::new().await;
    query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT)").execute(f.pool()).await.unwrap();
    query("WITH RECURSIVE seq(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM seq WHERE id < 1000000) INSERT INTO items SELECT id, 'payload-' || id FROM seq").execute(f.pool()).await.unwrap();
    let actual: i64 = query_scalar("SELECT COUNT(*) FROM items").fetch_one(f.pool()).await.unwrap();
    assert_eq!(actual, 1_000_000);
    for filtered in [false, true] {
        let mut req = request("items", 0, 200, false);
        if filtered {
            req.filters = serde_json::from_value(serde_json::json!([{"conds":[{"column":"name","operator":"!=","value":"absent"}]}])).unwrap();
        }
        // Warm both paths before alternating seven measured rounds; includes the
        // real product metadata and history hooks, not just naked SELECT timing.
        for count in [true, false] { req.include_total = count; page(&f, &req).await; }
        let mut counted_ms = Vec::new();
        let mut no_count_ms = Vec::new();
        for _ in 0..7 {
            for count in [true, false] {
                req.include_total = count;
                crate::storage::clear_sql_log_at(&f.path);
                let start = std::time::Instant::now();
                let result = page(&f, &req).await;
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(result.rows.len(), 200);
                if count {
                    assert_eq!((result.total, result.total_exact, result.has_more), (1_000_000, true, Some(true)));
                    assert_eq!(f.logs().iter().filter(|e| e.sql.starts_with("SELECT COUNT(*) FROM")).count(), 1);
                    counted_ms.push(elapsed);
                } else {
                    assert_eq!((result.total, result.total_exact, result.has_more), (201, false, Some(true)));
                    let logs = no_count(&f);
                    let rows = logs.iter().find(|e| e.sql.starts_with("SELECT * FROM")).unwrap();
                    assert!(rows.sql.contains("LIMIT 201 OFFSET 0"));
                    assert_eq!(rows.rows, 201);
                    no_count_ms.push(elapsed);
                }
            }
        }
        counted_ms.sort_by(f64::total_cmp);
        no_count_ms.sort_by(f64::total_cmp);
        println!("million SQLite rows={actual} page_size=200 filtered={filtered} warmup=2 rounds=7 COUNT_ms={counted_ms:?} NO_COUNT_ms={no_count_ms:?} median_speedup={:.2}x", counted_ms[3] / no_count_ms[3]);
    }
}
