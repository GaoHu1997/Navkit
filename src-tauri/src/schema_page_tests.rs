use super::*;
use crate::models::PageResult;

fn request(offset: u64, limit: u64) -> PageRequest {
    serde_json::from_value(serde_json::json!({
        "connection_id":"test", "schema":"main", "table":"items",
        "offset":offset, "limit":limit, "include_total":false
    })).unwrap()
}

#[test]
fn page_wire_contract_preserves_legacy_defaults() {
    let req: PageRequest = serde_json::from_value(serde_json::json!({
        "connection_id":"test", "schema":"main", "table":"items", "offset":0, "limit":20
    })).unwrap();
    assert!(req.include_total);
    assert!(!request(0, 20).include_total);
    let result: PageResult = serde_json::from_value(serde_json::json!({
        "columns":[], "rows":[], "total":0, "primary_keys":[]
    })).unwrap();
    assert!(result.total_exact);
    assert_eq!(result.has_more, None);
    assert!(serde_json::to_value(result).unwrap().get("has_more").is_none());
}

#[test]
fn page_probe_bounds_and_uncertainty_are_explicit() {
    for (offset, limit, n, expected) in [
        (0, 2, 3, (3, false, Some(true))),
        (2, 2, 3, (5, false, Some(true))),
        (4, 2, 1, (5, true, Some(false))),
        (2, 2, 2, (4, true, Some(false))),
        (0, 2, 0, (0, true, Some(false))),
        (99, 2, 0, (0, false, Some(false))),
        (0, 0, 3, (0, false, None)),
        (99, 0, 0, (0, false, None)),
    ] {
        let req = request(offset, limit);
        let mut rows = vec![(); n];
        assert_eq!(finish(&req, &mut rows, None), expected);
        assert!(rows.len() as u64 <= limit);
    }
    assert_eq!(fetch_limit(&request(0, u64::MAX)), 100_001);
    assert_eq!(fetch_limit(&request(0, 0)), 0);
    let mut req = request(2, 2);
    req.include_total = true;
    assert_eq!(fetch_limit(&req), 2);
    assert_eq!(finish(&req, &mut vec![(); 2], Some(10)), (10, true, Some(true)));
    assert_eq!(finish::<()>(&req, &mut vec![], Some(1)), (1, true, Some(false)));
    req.limit = 0;
    assert_eq!(finish(&req, &mut vec![(); 1], Some(10)), (10, true, None));
}

#[test]
fn page_sql_dialects_keep_probe_filters_order_and_zero_metadata() {
    let req = request(7, 20);
    for id in ["sqlite", "mysql", "postgres", "clickhouse", "sqlserver", "mssql", "oracle", "dameng", "db2", "firebird", "informix", "access", "iris", "cache", "hive", "inceptor", "argodb", "spark", "impala", "kyuubi"] {
        let kind = DbKind::of(id);
        let sql = rows_sql(&kind, &req, "items", " WHERE id > 0", " ORDER BY id DESC");
        assert!(sql.contains(" WHERE id > 0"), "{id}: {sql}");
        assert!(sql.contains(" ORDER BY id DESC"), "{id}: {sql}");
        assert!(!sql.to_uppercase().contains("COUNT"));
        let budget = if matches!(id, "access" | "iris" | "cache" | "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi") { "28" } else { "21" };
        assert!(sql.contains(budget), "{id}: {sql}");
        let zero = rows_sql(&kind, &request(99, 0), "items", " WHERE id > 0", "");
        assert_eq!(zero, "SELECT * FROM items WHERE id > 0 AND 1=0");
    }
    assert!(rows_sql(&DbKind::of("sqlserver"), &req, "items", "", "").contains(" ORDER BY (SELECT NULL) OFFSET 7 ROWS FETCH NEXT 21 ROWS ONLY"));
    assert!(rows_sql(&DbKind::of("firebird"), &req, "items", "", "").starts_with("SELECT FIRST 21 SKIP 7"));
    assert!(rows_sql(&DbKind::of("informix"), &req, "items", "", "").starts_with("SELECT SKIP 7 FIRST 21"));
}
