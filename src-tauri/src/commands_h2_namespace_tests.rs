use super::*;

#[tokio::test]
#[ignore = "requires a copied test executable in a unique navkit-command-tests-h2-* temp directory plus real Java/H2"]
async fn h2_memory_queries_metadata_and_schemas_share_one_real_session() {
    let root = storage::portable_root();
    assert!(root.starts_with(std::env::temp_dir()), "refuse user storage: {}", root.display());
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("navkit-command-tests-h2-"));
    let state = AppState { store: Store::load(), registry: Arc::new(DbRegistry::default()), manual_transactions: Default::default(), activity: Default::default() };
    let kind = DbKind::of("h2");
    let id = "h2-memory-command-probe";
    let config = ConnectionConfig {
        id: id.into(), name: id.into(), kind: kind.clone(), host: String::new(), port: 0,
        user: "sa".into(), password: String::new(), database: "jdbc:h2:mem:command_probe;DB_CLOSE_DELAY=-1".into(),
        file: String::new(), params: String::new(), group: String::new(), read_only: false, keep_alive: false,
    };
    state.store.save_connections(vec![config], Vec::new()).expect("isolated H2 fixture must actually persist its configuration");
    let base = ensure_db(&state, id).await.unwrap();
    let (public_key, public) = query_db(&state, id, Some("PUBLIC")).await.unwrap();
    for sql in [
        "CREATE TABLE CMD_ITEMS (payload VARCHAR(100))",
        "INSERT INTO CMD_ITEMS VALUES ('PUBLIC:中文;O''Reilly')",
        "CREATE SCHEMA CMD_OTHER",
    ] {
        db::execute(&state.registry, &public_key, sql, kind.clone()).await.unwrap();
    }
    let metadata = scoped_db(&state, id, "PUBLIC").await.unwrap();
    assert!(Arc::ptr_eq(&base, &metadata), "metadata must retain the base live DB");
    let objects = schema::list_tables(metadata.as_ref(), kind.clone(), "PUBLIC").await.unwrap();
    assert!(objects.iter().any(|(name, _)| name == "CMD_ITEMS"), "query-created memory table missing from metadata: {objects:?}");
    let (other_key, _) = query_db(&state, id, Some("CMD_OTHER")).await.unwrap();
    db::execute(&state.registry, &other_key, "CREATE TABLE CMD_ITEMS (payload VARCHAR(100))", kind.clone()).await.unwrap();
    db::execute(&state.registry, &other_key, "INSERT INTO CMD_ITEMS VALUES ('OTHER:真实模式')", kind.clone()).await.unwrap();
    let result_contains = |result: &MultiResult, expected: &str| result.sets.iter().any(|set| {
        set.rows.iter().any(|row| row.iter().any(|cell| cell.as_deref() == Some(expected)))
    });
    for _ in 0..3 {
        let (left, right) = tokio::join!(
            db::execute(&state.registry, &public_key, "SELECT payload FROM CMD_ITEMS", kind.clone()),
            db::execute(&state.registry, &other_key, "SELECT payload FROM CMD_ITEMS", kind.clone()),
        );
        assert!(result_contains(&left.unwrap(), "PUBLIC:中文;O'Reilly"));
        assert!(result_contains(&right.unwrap(), "OTHER:真实模式"));
    }
    let default_query = db::execute(&state.registry, id, "SELECT payload FROM CMD_ITEMS", kind.clone()).await.unwrap();
    assert!(result_contains(&default_query, "PUBLIC:中文;O'Reilly"), "unscoped query leaked the last schema");
    let (_, cached) = query_db(&state, id, Some("PUBLIC")).await.unwrap();
    assert!(Arc::ptr_eq(&public, &cached), "namespace facade should remain cached");
    assert!(query_db(&state, id, Some("CMD_MISSING_SCHEMA")).await.is_err());
    assert!(query_db(&state, id, Some("bad\0schema")).await.is_err());
    db::close_with_derived(&state.registry, id);
    assert!(state.registry.read().is_empty(), "closing the connection must release every namespace facade");
    println!("REAL H2 COMMAND SESSION OK: memory query/metadata share JDBC, concurrent PUBLIC/OTHER isolation, default schema reset, invalid schema errors and derived close");
}
