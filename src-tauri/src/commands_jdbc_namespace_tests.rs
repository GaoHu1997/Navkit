use super::*;
use crate::driver_manager::{DriverConnectionParams, DriverDefinition, DriverProtocol, DriverSource, PARAMS_PREFIX};

#[tokio::test]
#[ignore = "requires a copied executable in navkit-jdbc-command-tests-* plus actual Java/H2"]
async fn generic_jdbc_session_namespaces_binary_paging_and_close_are_real() {
    let root = storage::portable_root();
    assert!(root.starts_with(std::env::temp_dir()));
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"));
    let jar = std::env::var("NAVKIT_H2_JAR").expect("official H2 JAR");
    let state = AppState { store: Store::load(), registry: Arc::new(DbRegistry::default()), manual_transactions: Default::default(), executions: Default::default(), mutations: Default::default(), connection_runtime: Default::default(), activity: Default::default(), query_results: Default::default() };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let id = format!("jdbc-command-{suffix}");
    let driver_id = format!("driver-{suffix}");
    let database = root.join(format!("generic-jdbc-{suffix}"));
    let url = format!("jdbc:h2:file:{}", database.to_string_lossy().replace('\\', "/"));
    crate::driver_manager::save_custom_driver_for_test(DriverDefinition {
        id: driver_id.clone(), name: driver_id.clone(), protocol: DriverProtocol::Jdbc,
        source: DriverSource::File, target: jar, template: url, driver_class: "org.h2.Driver".into(), test_sql: "SELECT 1".into(),
    }).unwrap();
    let kind = DbKind::of("odbc");
    state.store.save_connections(vec![ConnectionConfig {
        id: id.clone(), name: id.clone(), kind: kind.clone(), host: String::new(), port: 0,
        user: "sa".into(), password: String::new(), database: "THIS_IS_CONNECTION_IDENTITY_NOT_A_SCHEMA".into(),
        file: String::new(), params: format!("{PARAMS_PREFIX}{}", serde_json::to_string(&DriverConnectionParams {
            version: 1, driver_id, template: String::new(),
        }).unwrap()), group: String::new(), read_only: false, keep_alive: false,
    }], Vec::new()).unwrap();
    let base = ensure_db(&state, &id).await.unwrap();
    let db::Db::Http(http) = base.as_ref() else { panic!("explicit JDBC must actually select the Java adapter") };
    assert!(http.shares_session_namespace());
    assert!(http.database.is_empty(), "saved database identity must not become a namespace");
    let (public_key, public_view) = query_db(&state, &id, Some("PUBLIC")).await.unwrap();
    for sql in [
        "CREATE TABLE ITEMS(id INT PRIMARY KEY,payload VARCHAR(100),nil VARCHAR(20),bin VARBINARY(20))",
        "INSERT INTO ITEMS VALUES(1,'PUBLIC:中文;O''Reilly\\尾',NULL,X'00FF0180')",
        "INSERT INTO ITEMS VALUES(2,'第二行',NULL,X'')",
        "CREATE SCHEMA OTHER_JDBC",
    ] { db::execute(&state.registry, &public_key, sql, kind.clone()).await.unwrap(); }
    let metadata = scoped_db(&state, &id, "PUBLIC").await.unwrap();
    assert!(Arc::ptr_eq(&base, &metadata));
    assert!(schema::list_tables(metadata.as_ref(), kind.clone(), "PUBLIC").await.unwrap().iter().any(|(name,_)| name == "ITEMS"));
    let (other_key, other_view) = query_db(&state, &id, Some("OTHER_JDBC")).await.unwrap();
    for sql in ["CREATE TABLE ITEMS(payload VARCHAR(100))", "INSERT INTO ITEMS VALUES('OTHER:另一真实模式')"] {
        db::execute(&state.registry, &other_key, sql, kind.clone()).await.unwrap();
    }
    fn contains(result: &MultiResult, value: &str) -> bool {
        result.sets.iter().any(|set| set.rows.iter().any(|row| row.iter().any(|cell| cell.as_deref() == Some(value))))
    }
    for _ in 0..3 {
        let (left,right) = tokio::join!(
            db::execute(&state.registry, &public_key, "SELECT payload FROM ITEMS WHERE id=1", kind.clone()),
            db::execute(&state.registry, &other_key, "SELECT payload FROM ITEMS", kind.clone()),
        );
        assert!(contains(&left.unwrap(), "PUBLIC:中文;O'Reilly\\尾"));
        assert!(contains(&right.unwrap(), "OTHER:另一真实模式"));
    }
    let full = db::execute(&state.registry, &id, "SELECT payload,nil,bin FROM ITEMS WHERE id=1", kind.clone()).await.unwrap();
    assert!(contains(&full, "PUBLIC:中文;O'Reilly\\尾"));
    assert!(contains(&full, "base64:AP8BgA=="));
    assert!(full.sets.iter().any(|s| s.rows.iter().any(|r| r.get(1) == Some(&None))));
    let preview = http.rows("PUBLIC", "ITEMS", 1, 1).await.unwrap();
    assert_eq!(preview.rows.len(), 1);
    assert_eq!(preview.rows[0][1].as_deref(), Some("第二行"));
    assert_eq!(preview.rows[0][3].as_deref(), Some("base64:"));
    let zero = http.rows("PUBLIC", "ITEMS", 0, 0).await.unwrap();
    assert_eq!(zero.columns.len(), 4); assert!(zero.rows.is_empty());
    assert!(http.rows("PUBLIC", "ITEMS", 1000000, 1).await.is_err());
    assert!(http.rows("PUBLIC", "ITEMS", 0, 10001).await.is_err());
    assert!(http.rows("PUBLIC", "ITEMS", u64::MAX, 1).await.is_err());
    assert!(query_db(&state, &id, Some("MISSING_JDBC")).await.is_err());
    assert!(query_db(&state, &id, Some("bad\0namespace")).await.is_err());
    assert!(db::execute(&state.registry, &id, "SELECT X FROM SYSTEM_RANGE(1,12001)", kind.clone()).await.unwrap_err().to_string().contains("10000"));
    let columns = http.table_columns("PUBLIC", "ITEMS").await.unwrap().unwrap();
    assert!(columns.iter().any(|column| column.name=="ID" && column.primary_key));
    db::close_with_derived(&state.registry, &id);
    assert!(state.registry.read().is_empty());
    drop(other_view); drop(public_view); drop(metadata); drop(base);
    // The ordinary close must flush committed file data; reopening is a new JVM.
    let reopened = ensure_db(&state, &id).await.unwrap();
    let result = db::execute(&state.registry, &id, "SELECT payload,bin FROM PUBLIC.ITEMS WHERE id=1", kind.clone()).await.unwrap();
    assert!(contains(&result, "PUBLIC:中文;O'Reilly\\尾") && contains(&result, "base64:AP8BgA=="));
    db::close_with_derived(&state.registry, &id); drop(reopened);
    println!("REAL GENERIC JDBC PASS: default identity retained; shared concurrent schemas; metadata; Unicode/NULL/binary; exact paging/zero/overflow; ordinary close and committed file reopen");
}
