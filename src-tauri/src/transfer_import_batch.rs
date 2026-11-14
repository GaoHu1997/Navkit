//! Conservative import batching: exact literals and one existing target transaction.
//! Unsupported/default/trigger/foreign-key cases retain the old per-row execution.
use super::*;

pub(super) const MAX_ROWS: usize = 256;
pub(super) const MAX_SQL_BYTES: usize = 1024 * 1024;

/// The gate runs after BEGIN and any prefix DDL, on the same locked SQLite session.
/// Full explicit columns avoid changes to statement-scoped default evaluation.
pub(super) async fn sqlite_can_batch(conn: &mut sqlx::SqliteConnection, mapper: &ImportMapper) -> Result<bool> {
    if mapper.strategy != "error" || mapper.cols.is_empty()
        || !matches!(mapper.schema.as_str(), "" | "main") {
        return Ok(false);
    }
    let shadow: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM temp.sqlite_schema WHERE name=? COLLATE NOCASE")
        .bind(&mapper.table).fetch_one(&mut *conn).await?;
    if shadow != 0 { return Ok(false); }
    let ordinary: i64 = crate::sql_history::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_list WHERE schema='main' AND name=? COLLATE NOCASE AND type='table'")
        .bind(&mapper.table).fetch_one(&mut *conn).await?;
    if ordinary != 1 { return Ok(false); }
    let triggers: i64 = crate::sql_history::query_scalar(
        "SELECT (SELECT COUNT(*) FROM main.sqlite_schema WHERE type='trigger' AND tbl_name=? COLLATE NOCASE) + (SELECT COUNT(*) FROM temp.sqlite_schema WHERE type='trigger' AND tbl_name=? COLLATE NOCASE)")
        .bind(&mapper.table).bind(&mapper.table).fetch_one(&mut *conn).await?;
    if triggers != 0 { return Ok(false); }
    let foreign: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_list(?)")
        .bind(&mapper.table).fetch_one(&mut *conn).await?;
    if foreign != 0 { return Ok(false); }
    let rows = crate::sql_history::query(&format!("PRAGMA main.table_xinfo({})", quote_ident(&mapper.kind, &mapper.table)))
        .fetch_all(&mut *conn).await?;
    if rows.len() != mapper.cols.len() { return Ok(false); }
    for row in rows {
        if row.try_get::<i64, _>("hidden")? != 0 { return Ok(false); }
        let name: String = row.try_get("name")?;
        let ty: String = row.try_get("type")?;
        let Some(index) = mapper.cols.iter().position(|column| column == &name) else { return Ok(false); };
        if !ty.eq_ignore_ascii_case(&mapper.types[index]) { return Ok(false); }
    }
    Ok(true)
}

pub(super) async fn no_batch<C>(_conn: &mut C, _mapper: &ImportMapper) -> Result<bool> { Ok(false) }

pub(super) fn statements(mapper: &ImportMapper, rows: &[Vec<Option<String>>], limits: &TransferLimits) -> Result<Vec<(String, bool)>> {
    if mapper.strategy != "error" { return mapper.statements(rows); }
    let prefix = format!("INSERT INTO {} ({}) VALUES ", mapper.full, mapper.quoted);
    let hard_limit = usize::try_from(limits.max_record_bytes).unwrap_or(usize::MAX);
    let byte_target = MAX_SQL_BYTES.min(usize::try_from(limits.batch_bytes).unwrap_or(usize::MAX));
    let row_target = MAX_ROWS.min(usize::try_from(limits.batch_rows).unwrap_or(MAX_ROWS)).max(1);
    let mut result = Vec::with_capacity(rows.len().div_ceil(row_target));
    let mut sql = String::new();
    let mut pending = 0usize;
    for row in rows {
        let values = mapper.pairs.iter().zip(&mapper.types).map(|((index, _), ty)| {
            crate::sync::value_sql(row.get(*index).and_then(Option::as_deref), &mapper.kind, ty, ty)
        }).collect::<Result<Vec<_>>>()?;
        let tuple = format!("({})", values.join(", "));
        let row_bytes = prefix.len().checked_add(tuple.len()).ok_or_else(|| anyhow!("导入 SQL 大小溢出"))?;
        if row_bytes > hard_limit { bail!("导入单行 SQL 转义后超过上限；没有截断或重放数据"); }
        if pending > 0 && (pending == row_target || sql.len().saturating_add(tuple.len()).saturating_add(2) > byte_target) {
            result.push((std::mem::take(&mut sql), true)); pending = 0;
        }
        if pending == 0 { sql.push_str(&prefix); } else { sql.push_str(", "); }
        sql.push_str(&tuple); pending += 1;
        // An oversized single row is sent alone, but only after the hard limit check.
        if sql.len() >= byte_target { result.push((std::mem::take(&mut sql), true)); pending = 0; }
    }
    if pending > 0 { result.push((sql, true)); }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mapper() -> ImportMapper {
        ImportMapper { pairs: vec![(0, "id".into()), (1, "v".into())], kind: DbKind::of("sqlite"),
            full: "\"t\"".into(), quoted: "\"id\", \"v\"".into(), strategy: "error".into(),
            conflict_cols: vec![], cols: vec!["id".into(), "v".into()], types: vec!["INTEGER".into(), "TEXT".into()],
            schema: "main".into(), table: "t".into() }
    }
    #[test]
    fn row_and_encoded_byte_limits_preserve_every_value() {
        let m = mapper();
        let rows = (0..1000).map(|i| vec![Some(i.to_string()), Some("quote'中\\value".into())]).collect::<Vec<_>>();
        let sql = statements(&m, &rows, &TransferLimits::default()).unwrap();
        assert_eq!(sql.len(), 4);
        assert_eq!(sql.iter().map(|(s, _)| s.matches("quote''中\\value").count()).sum::<usize>(), 1000);
        assert!(sql.iter().all(|(s, _)| s.len() <= MAX_SQL_BYTES));
        let mut limits = TransferLimits::default(); limits.batch_bytes = 150;
        let sql = statements(&m, &rows[..10], &limits).unwrap();
        assert!(sql.len() > 1); assert!(sql.iter().all(|(s, _)| s.len() <= 150));
        limits.max_record_bytes = 40;
        assert!(statements(&m, &rows[..1], &limits).is_err());
    }
    #[tokio::test]
    async fn gate_requires_full_explicit_ordinary_table_without_side_effects() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").execute(&mut *conn).await.unwrap();
        assert!(sqlite_can_batch(&mut conn, &mapper()).await.unwrap());
        sqlx::raw_sql("CREATE TRIGGER tr AFTER INSERT ON t BEGIN SELECT 1; END").execute(&mut *conn).await.unwrap();
        assert!(!sqlite_can_batch(&mut conn, &mapper()).await.unwrap());
        sqlx::raw_sql("DROP TRIGGER tr").execute(&mut *conn).await.unwrap();
        let mut omitted = mapper(); omitted.cols.pop(); omitted.types.pop(); omitted.pairs.pop();
        assert!(!sqlite_can_batch(&mut conn, &omitted).await.unwrap());
        let mut skip = mapper(); skip.strategy = "skip".into();
        assert!(!sqlite_can_batch(&mut conn, &skip).await.unwrap());
        sqlx::raw_sql("DROP TABLE t; CREATE TABLE p(id INTEGER PRIMARY KEY); CREATE TABLE t(id INTEGER REFERENCES p(id),v TEXT)").execute(&mut *conn).await.unwrap();
        assert!(!sqlite_can_batch(&mut conn, &mapper()).await.unwrap());
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("navkit-import-batch-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap(); Self(root)
        }
        fn file(&self, name: &str) -> std::path::PathBuf { self.0.join(name) }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn options(file: &Path) -> ImportOptions {
        serde_json::from_value(serde_json::json!({"connection_id":"isolated-test", "schema":"main", "table":"t", "format":"csv",
            "file":file.to_str().unwrap(), "encoding":"utf8", "delimiter":",", "has_header":true})).unwrap()
    }
    async fn import_into(pool: &sqlx::SqlitePool, file: &Path, batch: bool) -> Result<u64> {
        super::super::run_import_data_with_policy(&Db::Sqlite(pool.clone()), &DbKind::of("sqlite"), &options(file), &[], None, batch).await
    }
    #[tokio::test]
    async fn actual_csv_import_matches_row_execution_and_preserves_atomic_failure() {
        let fixture = Fixture::new();
        let file = fixture.file("input.csv");
        let mut writer = csv::Writer::from_path(&file).unwrap();
        writer.write_record(["id", "v"]).unwrap();
        for index in 0..1000 { writer.write_record([index.to_string(), format!("quote'中文,{index}\n\\tail")]).unwrap(); }
        writer.flush().unwrap(); drop(writer);
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").execute(&pool).await.unwrap();
        assert_eq!(import_into(&pool, &file, false).await.unwrap(), 1000);
        let baseline = sqlx::query_as::<_, (i64, String)>("SELECT id,v FROM t ORDER BY id").fetch_all(&pool).await.unwrap();
        sqlx::raw_sql("DELETE FROM t").execute(&pool).await.unwrap();
        assert_eq!(import_into(&pool, &file, true).await.unwrap(), 1000);
        assert_eq!(sqlx::query_as::<_, (i64, String)>("SELECT id,v FROM t ORDER BY id").fetch_all(&pool).await.unwrap(), baseline);
        sqlx::raw_sql("DELETE FROM t; INSERT INTO t VALUES(500,'existing')").execute(&pool).await.unwrap();
        let error = import_into(&pool, &file, true).await.unwrap_err();
        assert!(format!("{error:#}").contains("回滚已确认"));
        assert_eq!(sqlx::query_as::<_, (i64, String)>("SELECT id,v FROM t").fetch_all(&pool).await.unwrap(), vec![(500,"existing".into())]);
        // Trigger tables keep their per-row effect/order: the same input yields the same audit.
        sqlx::raw_sql("DELETE FROM t; CREATE TABLE audit(id INTEGER); CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO audit VALUES(new.id); END").execute(&pool).await.unwrap();
        assert_eq!(import_into(&pool, &file, true).await.unwrap(), 1000);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit").fetch_one(&pool).await.unwrap(), 1000);
    }
    #[tokio::test]
    async fn omitted_default_columns_retain_row_path_and_values() {
        let fixture = Fixture::new(); let file = fixture.file("input.csv");
        let mut output = String::from("id,v\n");
        for index in 0..96 { output.push_str(&format!("{index},value\n")); }
        std::fs::write(&file, output).unwrap();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT, d INTEGER DEFAULT 42)").execute(&pool).await.unwrap();
        assert_eq!(import_into(&pool, &file, true).await.unwrap(), 96);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM t WHERE d=42").fetch_one(&pool).await.unwrap(), 96);
    }

    #[tokio::test]
    #[ignore = "isolated file-backed end-to-end A/B benchmark; no external DB"]
    async fn benchmark_actual_csv_import_row_vs_batch() {
        let fixture = Fixture::new();
        let rows = std::env::var("NAVKIT_IMPORT_BENCH_ROWS").ok().and_then(|value| value.parse::<usize>().ok()).unwrap_or(100_000);
        assert!((1..=1_000_000).contains(&rows));
        let file = fixture.file("input.csv");
        let mut writer = csv::Writer::from_path(&file).unwrap(); writer.write_record(["id","v"]).unwrap();
        for index in 0..rows { writer.write_record([index.to_string(), format!("value-中文-{index},quote' and newline\nnext")]).unwrap(); }
        writer.flush().unwrap(); drop(writer);
        let database = fixture.file("benchmark.sqlite");
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(database).create_if_missing(true)).await.unwrap();
        let mut row_times = Vec::new(); let mut batch_times = Vec::new();
        for round in 0..6 {
            let modes = if round % 2 == 0 { [false,true] } else { [true,false] };
            for batch in modes {
                sqlx::raw_sql("DROP TABLE IF EXISTS t; CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT)").execute(&pool).await.unwrap();
                let start = std::time::Instant::now();
                assert_eq!(import_into(&pool, &file, batch).await.unwrap(), rows as u64);
                let elapsed = start.elapsed().as_secs_f64();
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM t").fetch_one(&pool).await.unwrap(), rows as i64);
                assert_eq!(sqlx::query_scalar::<_, String>("SELECT v FROM t WHERE id=?").bind((rows-1) as i64).fetch_one(&pool).await.unwrap(), format!("value-中文-{},quote' and newline\nnext",rows-1));
                println!("IMPORT_AB round={round} mode={} rows={rows} seconds={elapsed:.6}",if batch {"batch"} else {"row"});
                if round > 0 { if batch {batch_times.push(elapsed)} else {row_times.push(elapsed)} }
            }
        }
        row_times.sort_by(f64::total_cmp); batch_times.sort_by(f64::total_cmp);
        println!("IMPORT_AB_MEDIAN rows={rows} row_seconds={:.6} batch_seconds={:.6} ratio={:.3}", row_times[2], batch_times[2], row_times[2]/batch_times[2]);
        // No wall-clock assertion: real repeated measurements determine whether auto mode is retained.
        pool.close().await;
    }
}
