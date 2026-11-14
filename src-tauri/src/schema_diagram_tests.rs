use super::*;
async fn pool() -> SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap()
}
async fn exec(p: &SqlitePool, sql: &str) {
    crate::sql_history::query(sql).execute(p).await.unwrap();
}
fn table<'a>(metadata: &'a DiagramMetadata, name: &str) -> &'a TableSchema {
    metadata.tables.iter().find(|t| t.table == name).unwrap()
}

#[tokio::test]
async fn sqlite_complex_indexes_do_not_hide_columns_or_foreign_keys_and_never_count() {
    let p = pool().await;
    exec(&p, "CREATE TABLE parent(a INT,b INT,PRIMARY KEY(b,a))").await;
    exec(&p,"CREATE TABLE child(x INT,y INT,FOREIGN KEY(x,y) REFERENCES parent ON UPDATE CASCADE ON DELETE SET NULL)").await;
    exec(&p, "CREATE INDEX expr ON child(x+1)").await;
    exec(&p, "CREATE UNIQUE INDEX partial ON child(x) WHERE x>0").await;
    exec(&p, "CREATE UNIQUE INDEX simple ON child(y,x)").await;
    let path = std::env::temp_dir().join(format!("navkit-diagram-{}.jsonl", uuid::Uuid::new_v4()));
    let context = crate::sql_history::Context {
        path: Some(path.clone()),
        ..Default::default()
    };
    let metadata = crate::sql_history::scope(
        context,
        load(&Db::Sqlite(p.clone()), DbKind::of("sqlite"), ""),
    )
    .await
    .unwrap();
    assert_eq!(metadata.tables.len(), 2);
    let child = table(&metadata, "child");
    assert_eq!(child.database, "main");
    assert_eq!(child.columns.len(), 2);
    assert_eq!(child.foreign_keys.len(), 1);
    let fk = &child.foreign_keys[0];
    assert_eq!(fk.columns, vec!["x", "y"]);
    assert_eq!(fk.ref_columns, vec!["b", "a"]);
    assert_eq!(fk.ref_schema.as_deref(), Some("main"));
    assert_eq!(fk.on_update.as_deref(), Some("CASCADE"));
    assert_eq!(fk.on_delete.as_deref(), Some("SET NULL"));
    assert_eq!(child.indexes.len(), 1);
    assert_eq!(child.indexes[0].columns, vec!["y", "x"]);
    assert!(metadata.failures.is_empty());
    assert!(!metadata.limitations.is_empty());
    assert!(metadata.tables.iter().all(|t| t.row_count.is_none()));
    let logs = crate::storage::read_sql_log_at(&path, 1000);
    assert!(!logs.is_empty());
    assert!(logs
        .iter()
        .all(|e| !e.sql.to_ascii_uppercase().contains("COUNT(")));
    assert!(logs.iter().all(|e| e.ok));
    let json = serde_json::to_value(&metadata).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 3);
    assert_eq!(json["tables"][0]["columns"][0]["type"], "INT");
    std::fs::remove_file(path).unwrap();
    p.close().await;
}
#[tokio::test]
async fn sqlite_namespace_quoted_and_dotted_names_remain_exact() {
    let p = pool().await;
    exec(&p, "ATTACH DATABASE ':memory:' AS aux").await;
    exec(&p, "CREATE TABLE main.parent(wrong INT PRIMARY KEY)").await;
    exec(&p, "CREATE TABLE aux.parent(b INT PRIMARY KEY)").await;
    exec(
        &p,
        "CREATE TABLE aux.\"Child.\"\"quoted\"(x INT REFERENCES parent)",
    )
    .await;
    let metadata = sqlite(&p, "aux").await.unwrap();
    assert!(metadata.tables.iter().all(|t| t.database == "aux"));
    let child = table(&metadata, "Child.\"quoted");
    assert_eq!(child.foreign_keys[0].ref_columns, vec!["b"]);
    assert_eq!(child.foreign_keys[0].ref_schema.as_deref(), Some("aux"));
    assert!(sqlite(&p, "missing")
        .await
        .unwrap_err()
        .to_string()
        .contains("DIAGRAM_NAMESPACE_UNKNOWN"));
    p.close().await;
}
#[tokio::test]
async fn sqlite_bad_foreign_key_preserves_table_and_other_keys() {
    let p = pool().await;
    exec(&p, "CREATE TABLE parent(id INT PRIMARY KEY)").await;
    exec(
        &p,
        "CREATE TABLE child(x INT REFERENCES missing,y INT REFERENCES parent(id))",
    )
    .await;
    let metadata = sqlite(&p, "main").await.unwrap();
    let child = table(&metadata, "child");
    assert_eq!(child.columns.len(), 2);
    assert_eq!(child.foreign_keys.len(), 1);
    assert_eq!(child.foreign_keys[0].columns, vec!["y"]);
    assert!(metadata
        .failures
        .iter()
        .any(|f| f.table == "child" && f.error.contains("DIAGRAM_REFERENCE_UNPROVEN")));
    p.close().await;
}
#[tokio::test]
async fn sqlite_generated_columns_are_visible_and_empty_namespace_is_success() {
    let p = pool().await;
    assert!(sqlite(&p, "main").await.unwrap().tables.is_empty());
    exec(
        &p,
        "CREATE TABLE t(a INT,b INT GENERATED ALWAYS AS(a+1) STORED)",
    )
    .await;
    let metadata = sqlite(&p, "main").await.unwrap();
    assert_eq!(table(&metadata, "t").columns.len(), 2);
    p.close().await;
}
fn part(table: &str, ordinal: i64, local: &str, target: &str) -> ForeignPart {
    ForeignPart {
        table: table.into(),
        name: "same_constraint".into(),
        ordinal,
        column: local.into(),
        ref_schema: "Other.Schema".into(),
        ref_table: "Parent.Table".into(),
        ref_column: target.into(),
        on_update: Some("CASCADE".into()),
        on_delete: None,
    }
}
#[test]
fn batch_keys_use_table_and_constraint_tuple_and_pair_composite_ordinals() {
    let mut out = DiagramMetadata::default();
    out.tables = vec![
        shell("Selected.Schema", "Child.Table"),
        shell("Selected.Schema", "child.table"),
    ];
    foreign_groups(
        vec![
            part("Child.Table", 2, "b", "y"),
            part("child.table", 1, "z", "id"),
            part("Child.Table", 1, "a", "x"),
        ],
        &mut out,
    );
    assert!(out.failures.is_empty());
    let first = &table(&out, "Child.Table").foreign_keys[0];
    assert_eq!(first.columns, vec!["a", "b"]);
    assert_eq!(first.ref_columns, vec!["x", "y"]);
    assert_eq!(first.ref_schema.as_deref(), Some("Other.Schema"));
    assert_eq!(first.ref_table, "Parent.Table");
    assert_eq!(
        table(&out, "child.table").foreign_keys[0].columns,
        vec!["z"]
    );
}
#[test]
fn malformed_batch_key_does_not_remove_table_or_its_columns() {
    let mut out = DiagramMetadata::default();
    let mut t = shell("ns", "t");
    t.columns
        .push(column("x".into(), "INT".into(), true, None, false));
    out.tables.push(t);
    foreign_groups(vec![part("t", 2, "x", "id")], &mut out);
    assert_eq!(out.tables.len(), 1);
    assert_eq!(out.tables[0].columns.len(), 1);
    assert!(out.tables[0].foreign_keys.is_empty());
    assert_eq!(out.failures.len(), 1);
    out.batch_failure("indexes", "denied");
    assert_eq!(out.tables.len(), 1);
    assert_eq!(out.failures.len(), 2);
}
#[test]
fn unreliable_index_is_wholly_ignored_not_a_truncated_unique_key() {
    let mut out = DiagramMetadata::default();
    out.tables.push(shell("ns", "t"));
    index_groups(
        vec![
            IndexPart {
                table: "t".into(),
                name: "complex".into(),
                ordinal: 1,
                column: Some("x".into()),
                unique: true,
                primary: false,
                method: None,
                simple: true,
            },
            IndexPart {
                table: "t".into(),
                name: "complex".into(),
                ordinal: 2,
                column: None,
                unique: true,
                primary: false,
                method: None,
                simple: false,
            },
        ],
        &mut out,
    );
    assert!(out.tables[0].indexes.is_empty());
    assert!(out.failures.is_empty());
    assert_eq!(out.limitations.len(), 1);
}
#[test]
fn partial_column_failure_does_not_claim_a_truncated_primary_key() {
    let mut out = DiagramMetadata::default();
    let mut t = shell("ns", "t");
    t.columns
        .push(column("a".into(), "INT".into(), false, None, true));
    t.indexes.push(IndexInfo {
        name: "PRIMARY".into(),
        columns: vec!["a".into(), "b".into()],
        unique: true,
        primary: true,
        method: None,
    });
    out.tables.push(t);
    out.failure("t", "columns", "could not decode composite PK column b");
    out.check_columns();
    assert_eq!(out.tables[0].columns.len(), 1);
    assert!(!out.tables[0].columns[0].primary_key);
    assert_eq!(out.tables[0].indexes[0].columns, vec!["a", "b"]);
    assert_eq!(out.failures.len(), 1);
}
#[test]
fn failed_trailing_key_decode_never_emits_a_truncated_index_or_foreign_key() {
    let mut out = DiagramMetadata::default();
    out.tables.push(shell("ns", "t"));
    out.failure(
        "t",
        "indexes",
        "trailing composite index row cannot be decoded",
    );
    out.failure(
        "t",
        "foreign_keys",
        "trailing composite foreign key row cannot be decoded",
    );
    index_groups(
        vec![IndexPart {
            table: "t".into(),
            name: "primary".into(),
            ordinal: 1,
            column: Some("x".into()),
            unique: true,
            primary: true,
            method: None,
            simple: true,
        }],
        &mut out,
    );
    foreign_groups(vec![part("t", 1, "x", "id")], &mut out);
    assert!(out.tables[0].indexes.is_empty());
    assert!(out.tables[0].foreign_keys.is_empty());
    assert_eq!(out.tables.len(), 1);
}
#[tokio::test]
async fn sqlite_resolves_catalog_case_for_implicit_parent_and_keeps_non_system_prefix() {
    let p = pool().await;
    exec(&p, "CREATE TABLE Parent(id INT PRIMARY KEY)").await;
    exec(&p, "CREATE TABLE child(x INT REFERENCES PARENT)").await;
    exec(&p, "CREATE TABLE sqliteX(value INT)").await;
    exec(
        &p,
        "CREATE TABLE child_explicit(x INT,FOREIGN KEY(X) REFERENCES PARENT(ID))",
    )
    .await;
    let metadata = sqlite(&p, "main").await.unwrap();
    assert!(metadata.failures.is_empty());
    assert_eq!(metadata.tables.len(), 4);
    let fk = &table(&metadata, "child").foreign_keys[0];
    assert_eq!(fk.ref_table, "Parent");
    assert_eq!(fk.ref_columns, vec!["id"]);
    let explicit = &table(&metadata, "child_explicit").foreign_keys[0];
    assert_eq!(explicit.ref_table, "Parent");
    assert_eq!(explicit.columns, vec!["x"]);
    assert_eq!(explicit.ref_columns, vec!["id"]);
    p.close().await;
}
#[test]
fn native_queries_are_namespace_bounded_without_count_or_mysql_catalog_join() {
    for sql in [
        MY_TABLES, MY_COLUMNS, MY_INDEXES, MY_FK, MY_RULES, PG_TABLES, PG_COLUMNS, PG_INDEXES,
        PG_FK,
    ] {
        assert!(!sql.to_ascii_uppercase().contains("COUNT("));
        assert!(sql.contains("WHERE"));
        assert!(!sql.contains("LIMIT ")); // no silent arbitrary catalog truncation
    }
    assert!(!MY_FK.contains("JOIN"));
    assert!(!MY_RULES.contains("JOIN"));
    assert!(!MY_COLUMNS.contains("TABLE_NAME = ?"));
    assert!(!PG_COLUMNS.contains("c.relname=$2"));
    assert!(PG_FK.contains("ref_key.ordinal = local_key.ordinal"));
    assert!(!PG_FK.contains("ANY("));
    assert_eq!(pg_action("c").as_deref(), Some("CASCADE"));
    assert_eq!(pg_action("unknown"), None);
}
