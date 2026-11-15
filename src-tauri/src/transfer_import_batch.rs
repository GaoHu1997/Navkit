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

/// Exact native MySQL 8.0 gate, measured through the same import entry point.
/// No aliases, metadata guesses or replay; unsupported cases keep row execution.
pub(super) async fn mysql_can_batch(conn: &mut sqlx::MySqlConnection, mapper: &ImportMapper) -> Result<bool> {
    if mapper.kind.id() != "mysql" || mapper.strategy != "error" || mapper.schema.is_empty() || mapper.cols.is_empty() {
        return Ok(false);
    }
    let version: String = crate::sql_history::query_scalar("SELECT VERSION()").fetch_one(&mut *conn).await?;
    if !version.starts_with("8.0.") || version.to_ascii_lowercase().contains("tidb") { return Ok(false); }
    let mode: String = crate::sql_history::query_scalar("SELECT @@session.sql_mode").fetch_one(&mut *conn).await?;
    let modes = mode.split(',').collect::<HashSet<_>>();
    if !modes.contains("NO_BACKSLASH_ESCAPES") || !(modes.contains("STRICT_ALL_TABLES") || modes.contains("STRICT_TRANS_TABLES")) { return Ok(false); }
    let packet: u64 = crate::sql_history::query_scalar("SELECT @@session.max_allowed_packet").fetch_one(&mut *conn).await?;
    if packet < TransferLimits::default().max_record_bytes.saturating_add(4096) { return Ok(false); }
    // Without global visibility an empty trigger/FK result could mean hidden metadata.
    // Accept only direct global metadata visibility, without partial-revoke ambiguity.
    let grants = crate::sql_history::query("SHOW GRANTS").fetch_all(&mut *conn).await?;
    let grants = grants.iter().map(|row| row.try_get::<String, _>(0)).collect::<std::result::Result<Vec<_>, _>>()?;
    let mut privileges = HashSet::new();
    for grant in &grants {
        if let Some((list, _)) = grant.strip_prefix("GRANT ").and_then(|value| value.split_once(" ON *.* TO ")) {
            privileges.extend(list.split(", "));
        }
    }
    if !(privileges.contains("ALL PRIVILEGES") || ["SELECT", "TRIGGER", "REFERENCES"].iter().all(|value| privileges.contains(value)))
        || grants.iter().any(|grant| grant.starts_with("REVOKE ")) { return Ok(false); }
    // SHOW CREATE exposes session temporary shadow tables; LIMIT 0 takes an MDL
    // retained by the existing transaction before reading the final metadata.
    crate::sql_history::query(&format!("SELECT * FROM {} LIMIT 0", mapper.full)).fetch_all(&mut *conn).await?;
    let create = crate::sql_history::query(&format!("SHOW CREATE TABLE {}", mapper.full)).fetch_one(&mut *conn).await?;
    let definition: String = create.try_get(1)?;
    if !definition.starts_with("CREATE TABLE ") { return Ok(false); }
    let engine: Option<String> = crate::sql_history::query_scalar(
        "SELECT ENGINE FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND TABLE_TYPE='BASE TABLE'")
        .bind(&mapper.schema).bind(&mapper.table).fetch_optional(&mut *conn).await?.flatten();
    if engine.as_deref() != Some("InnoDB") { return Ok(false); }
    let triggers: i64 = crate::sql_history::query_scalar(
        "SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND EVENT_OBJECT_TABLE=?")
        .bind(&mapper.schema).bind(&mapper.table).fetch_one(&mut *conn).await?;
    if triggers != 0 { return Ok(false); }
    let foreign: i64 = crate::sql_history::query_scalar(
        "SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE (TABLE_SCHEMA=? AND TABLE_NAME=? AND REFERENCED_TABLE_NAME IS NOT NULL) OR (REFERENCED_TABLE_SCHEMA=? AND REFERENCED_TABLE_NAME=?)")
        .bind(&mapper.schema).bind(&mapper.table).bind(&mapper.schema).bind(&mapper.table).fetch_one(&mut *conn).await?;
    if foreign != 0 { return Ok(false); }
    let columns = crate::sql_history::query(
        "SELECT CAST(COLUMN_NAME AS CHAR) AS COLUMN_NAME,CAST(COLUMN_TYPE AS CHAR) AS COLUMN_TYPE,CAST(COLUMN_DEFAULT AS CHAR) AS COLUMN_DEFAULT,CAST(EXTRA AS CHAR) AS EXTRA,CAST(GENERATION_EXPRESSION AS CHAR) AS GENERATION_EXPRESSION FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION")
        .bind(&mapper.schema).bind(&mapper.table).fetch_all(&mut *conn).await?;
    if columns.len() != mapper.cols.len() || mapper.cols.iter().collect::<HashSet<_>>().len() != columns.len() { return Ok(false); }
    for column in columns {
        let name: String = column.try_get("COLUMN_NAME")?;
        let ty: String = column.try_get("COLUMN_TYPE")?;
        let default: Option<String> = column.try_get("COLUMN_DEFAULT")?;
        let extra: String = column.try_get("EXTRA")?;
        let generated: String = column.try_get("GENERATION_EXPRESSION")?;
        let Some(index) = mapper.cols.iter().position(|value| value == &name) else { return Ok(false); };
        if default.is_some() || !extra.is_empty() || !generated.is_empty() || !ty.eq_ignore_ascii_case(&mapper.types[index]) { return Ok(false); }
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

    async fn mysql_live_pool() -> sqlx::MySqlPool {
        let password = std::env::var("NAVKIT_IMPORT_MYSQL_PASSWORD").expect("explicit disposable native MySQL test credentials required");
        let options = sqlx::mysql::MySqlConnectOptions::new().host("127.0.0.1").port(3306).username("root").password(&password)
            .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
        sqlx::mysql::MySqlPoolOptions::new().max_connections(1).after_connect(|conn, _| Box::pin(async move {
            sqlx::query("SET SESSION sql_mode='STRICT_ALL_TABLES,NO_BACKSLASH_ESCAPES'").execute(conn).await?; Ok(())
        })).connect_with(options).await.unwrap()
    }
    fn mysql_mapper(schema: &str) -> ImportMapper {
        let mut mapper = mapper(); mapper.kind = DbKind::of("mysql"); mapper.schema = schema.into(); mapper.table = "t".into();
        mapper.full = format!("`{schema}`.`t`"); mapper.quoted = "`id`, `v`".into(); mapper.types = vec!["int".into(), "text".into()]; mapper
    }
    async fn mysql_import(pool: &sqlx::MySqlPool, schema: &str, file: &Path, batch: bool) -> Result<u64> {
        let mut opts = options(file); opts.schema = schema.into();
        super::super::run_import_data_with_policy(&Db::Mysql(pool.clone()), &DbKind::of("mysql"), &opts, &[], None, batch).await
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable local native MySQL 8.0 credentials"]
    async fn mysql_live_gate_and_import_parity() {
        let pool = mysql_live_pool().await;
        let schema = format!("navkit_r4_import_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE DATABASE `{schema}`")).execute(&pool).await.unwrap();
        let result = async {
            let table = format!("`{schema}`.`t`");
            let make = |definition: &str| format!("DROP TABLE IF EXISTS {table}; CREATE TABLE {table}({definition}) ENGINE=InnoDB");
            sqlx::raw_sql(&make("id INT PRIMARY KEY,v TEXT")).execute(&pool).await.unwrap();
            let mapper = mysql_mapper(&schema);
            let version: String = sqlx::query_scalar("SELECT VERSION()").fetch_one(&pool).await.unwrap();
            let packet: u64 = sqlx::query_scalar("SELECT @@session.max_allowed_packet").fetch_one(&pool).await.unwrap();
            println!("MYSQL_IMPORT_NATIVE version={version} max_allowed_packet={packet}");
            let mut tx = pool.begin().await.unwrap();
            assert!(mysql_can_batch(&mut tx, &mapper).await.unwrap());
            let ddl_pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect_with(pool.connect_options().as_ref().clone()).await.unwrap();
            let (sent, ready) = tokio::sync::oneshot::channel();
            let ddl_table = table.clone(); let worker_pool = ddl_pool.clone();
            let mut ddl = tokio::spawn(async move {
                let _ = sent.send(());
                sqlx::raw_sql(&format!("ALTER TABLE {ddl_table} ADD COLUMN mdl_guard INT NULL")).execute(&worker_pool).await
            });
            ready.await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(150), &mut ddl).await.is_err(), "DDL must wait for transaction MDL");
            tx.rollback().await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), ddl).await.unwrap().unwrap().unwrap(); ddl_pool.close().await;
            sqlx::raw_sql(&format!("ALTER TABLE {table} DROP COLUMN mdl_guard")).execute(&pool).await.unwrap();
            for kind in ["mariadb", "tidb"] {
                let mut alias = mysql_mapper(&schema); alias.kind = DbKind::of(kind);
                let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &alias).await.unwrap()); tx.rollback().await.unwrap();
            }
            sqlx::raw_sql(&format!("CREATE TEMPORARY TABLE {table}(id INT PRIMARY KEY,v TEXT) ENGINE=InnoDB")).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &mapper).await.unwrap()); tx.rollback().await.unwrap();
            sqlx::raw_sql(&format!("DROP TEMPORARY TABLE {table}; ALTER TABLE {table} ENGINE=MyISAM")).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &mapper).await.unwrap()); tx.rollback().await.unwrap();
            for definition in ["id INT PRIMARY KEY,v TEXT DEFAULT ('x')", "id INT AUTO_INCREMENT PRIMARY KEY,v TEXT", "id INT PRIMARY KEY,v TEXT,g INT GENERATED ALWAYS AS (id+1) STORED"] {
                sqlx::raw_sql(&make(definition)).execute(&pool).await.unwrap();
                let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &mapper).await.unwrap()); tx.rollback().await.unwrap();
            }
            sqlx::raw_sql(&make("id INT PRIMARY KEY,v TEXT")).execute(&pool).await.unwrap();
            sqlx::raw_sql(&format!("CREATE TRIGGER `{schema}`.`tr` BEFORE INSERT ON {table} FOR EACH ROW SET NEW.v=CONCAT(NEW.v,'!')")).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &mapper).await.unwrap()); tx.rollback().await.unwrap();
            sqlx::raw_sql(&format!("DROP TRIGGER `{schema}`.`tr`; CREATE TABLE `{schema}`.`child`(id INT REFERENCES {table}(id), CONSTRAINT fk FOREIGN KEY(id) REFERENCES {table}(id)) ENGINE=InnoDB")).execute(&pool).await.unwrap();
            let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &mapper).await.unwrap()); tx.rollback().await.unwrap();
            sqlx::raw_sql(&format!("DROP TABLE `{schema}`.`child`")).execute(&pool).await.unwrap();
            let mut incomplete = mysql_mapper(&schema); incomplete.cols.pop(); incomplete.types.pop(); incomplete.pairs.pop();
            let mut tx = pool.begin().await.unwrap(); assert!(!mysql_can_batch(&mut tx, &incomplete).await.unwrap()); tx.rollback().await.unwrap();
            let fixture = Fixture::new(); let file = fixture.file("mysql.csv");
            let mut writer = csv::Writer::from_path(&file).unwrap(); writer.write_record(["id", "v"]).unwrap();
            for i in 0..1000 { writer.write_record([i.to_string(), format!("quote'中文,{i}\n\\tail")]).unwrap(); } writer.flush().unwrap(); drop(writer);
            assert_eq!(mysql_import(&pool, &schema, &file, false).await.unwrap(), 1000);
            let baseline = sqlx::query_as::<_, (i32,String)>(&format!("SELECT id,v FROM {table} ORDER BY id")).fetch_all(&pool).await.unwrap();
            sqlx::raw_sql(&format!("DELETE FROM {table}")).execute(&pool).await.unwrap();
            let mut opts = options(&file); opts.schema = schema.clone();
            assert_eq!(super::super::run_import_data(&Db::Mysql(pool.clone()), &DbKind::of("mysql"), &opts, &[], None).await.unwrap(), 1000);
            assert_eq!(sqlx::query_as::<_, (i32,String)>(&format!("SELECT id,v FROM {table} ORDER BY id")).fetch_all(&pool).await.unwrap(), baseline);
            sqlx::raw_sql(&format!("DELETE FROM {table};INSERT INTO {table} VALUES(500,'existing')")).execute(&pool).await.unwrap();
            assert!(mysql_import(&pool, &schema, &file, true).await.is_err());
            assert_eq!(sqlx::query_as::<_, (i32,String)>(&format!("SELECT id,v FROM {table}")).fetch_all(&pool).await.unwrap(), vec![(500,"existing".into())]);
        };
        // Cleanup also runs after an assertion panic in this future.
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(result)).await;
        sqlx::raw_sql(&format!("DROP DATABASE `{schema}`")).execute(&pool).await.unwrap(); pool.close().await;
        if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
    }

    #[tokio::test]
    #[ignore = "native release A/B; explicit local credentials; verifies every value"]
    async fn benchmark_mysql_actual_csv_import_row_vs_batch() {
        let pool = mysql_live_pool().await;
        let schema = format!("navkit_r4_import_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(&format!("CREATE DATABASE `{schema}`")).execute(&pool).await.unwrap();
        let result = async {
            let rows = std::env::var("NAVKIT_IMPORT_BENCH_ROWS").unwrap().parse::<usize>().unwrap();
            assert!([1000,100_000,1_000_000].contains(&rows));
            let fixture = Fixture::new(); let file = fixture.file("input.csv");
            let mut writer = csv::Writer::from_path(&file).unwrap(); writer.write_record(["id","v"]).unwrap();
            let expected_value = |index: usize| format!("value-中文-{index},quote' and newline\nnext\\end");
            for index in 0..rows { writer.write_record([index.to_string(),expected_value(index)]).unwrap(); } writer.flush().unwrap(); drop(writer);
            let table = format!("`{schema}`.`t`"); let mut row_times = Vec::new(); let mut batch_times = Vec::new();
            for round in 0..if rows == 1_000_000 { 3 } else { 5 } {
                for batch in if round % 2 == 0 { [false,true] } else { [true,false] } {
                    sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table};CREATE TABLE {table}(id INT PRIMARY KEY,v TEXT) ENGINE=InnoDB")).execute(&pool).await.unwrap();
                    let mut check = pool.begin().await.unwrap();
                    assert!(mysql_can_batch(&mut check, &mysql_mapper(&schema)).await.unwrap(), "benchmark requires the real gate to admit multi-row execution");
                    check.rollback().await.unwrap();
                    let started = std::time::Instant::now();
                    assert_eq!(mysql_import(&pool, &schema, &file, batch).await.unwrap(), rows as u64);
                    let elapsed = started.elapsed().as_secs_f64();
                    use futures::TryStreamExt;
                    let query = format!("SELECT id,v FROM {table} ORDER BY id");
                    let mut stream = sqlx::query_as::<_, (i32,String)>(&query).fetch(&pool);
                    let mut seen = 0usize;
                    while let Some((id, value)) = stream.try_next().await.unwrap() {
                        assert_eq!(id, seen as i32); assert_eq!(value, expected_value(seen)); seen += 1;
                    }
                    assert_eq!(seen, rows);
                    println!("MYSQL_IMPORT_AB round={round} mode={} rows={rows} seconds={elapsed:.6} full_parity=true", if batch {"batch"} else {"row"});
                    if batch { batch_times.push(elapsed); } else { row_times.push(elapsed); }
                }
            }
            row_times.sort_by(f64::total_cmp); batch_times.sort_by(f64::total_cmp);
            let middle = row_times.len()/2;
            println!("MYSQL_IMPORT_AB_MEDIAN rows={rows} row_seconds={:.6} batch_seconds={:.6} ratio={:.3}", row_times[middle],batch_times[middle],row_times[middle]/batch_times[middle]);
        };
        let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(result)).await;
        sqlx::raw_sql(&format!("DROP DATABASE `{schema}`")).execute(&pool).await.unwrap(); pool.close().await;
        if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
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
