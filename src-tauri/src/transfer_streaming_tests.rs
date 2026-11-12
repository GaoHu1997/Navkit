use super::*;
use std::io::Read;
use std::path::{Path, PathBuf};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self { let path = std::env::temp_dir().join(format!("navkit-transfer-streaming-{}", uuid::Uuid::new_v4())); std::fs::create_dir(&path).unwrap(); Self(path) }
    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
    fn assert_no_stage(&self) { assert!(!std::fs::read_dir(&self.0).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-"))); }
}
impl Drop for Directory { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
async fn database(dir: &Directory) -> (Db, sqlx::SqlitePool) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path("source.sqlite")).create_if_missing(true)).await.unwrap();
    (Db::Sqlite(pool.clone()), pool)
}
fn export(path: &Path, format: &str, encoding: &str, limit: Option<u64>) -> ExportTaskOptions {
    ExportTaskOptions { task_id: uuid::Uuid::new_v4().to_string(), options: ExportOptions { connection_id: "test".into(), schema: "main".into(), table: "result".into(), file: path.to_str().unwrap().into(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: encoding.into(), where_clause: None, limit } }
}
fn import(path: &Path, encoding: &str) -> ImportOptions {
    serde_json::from_value(serde_json::json!({"connection_id":"test","schema":"main","table":"result","file":path.to_str().unwrap(),"format":"sql","encoding":encoding})).unwrap()
}
fn decoded(path: &Path, encoding: &str) -> String {
    let mut reader = crate::transfer_stream::DecodeReader::new(File::open(path).unwrap(), encoding).unwrap();
    let mut text = String::new(); reader.read_to_string(&mut text).unwrap(); text
}

#[tokio::test]
async fn sqlite_cursor_exports_actual_non_utf8_bytes_and_special_values() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (id INTEGER PRIMARY KEY, payload TEXT, nullable TEXT)").execute(&pool).await.unwrap();
    for (encoding, payload) in [("latin1", "\u{0080}éÿ,\n\"quote\""), ("gb18030", "中文🦀,\n\"quote\""), ("big5", "繁體中文,\n\"quote\"")] {
        sqlx::query("DELETE FROM result").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO result VALUES (1, ?, NULL)").bind(payload).execute(&pool).await.unwrap();
        for format in ["csv", "json"] {
            let output = dir.path(&format!("{encoding}.{format}"));
            let options = export(&output, format, encoding, None);
            assert_eq!(run_export_task(&db, &DbKind::of("sqlite"), &options).await.unwrap(), 1);
            let text = decoded(&output, encoding);
            if format == "csv" {
                let mut csv = csv::Reader::from_reader(text.as_bytes());
                assert_eq!(csv.headers().unwrap().iter().collect::<Vec<_>>(), vec!["id", "payload", "nullable"]);
                let row = csv.records().next().unwrap().unwrap(); assert_eq!(row.get(1), Some(payload)); assert_eq!(row.get(2), Some("\\N"));
            } else {
                let rows: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap(); assert_eq!(rows[0]["payload"], payload); assert!(rows[0]["nullable"].is_null());
            }
            assert_eq!(task_progress(&options.task_id).unwrap().status, "completed");
        }
    }
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_cursor_empty_results_keep_headers_and_limits_are_exact() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (id INTEGER PRIMARY KEY, payload TEXT)").execute(&pool).await.unwrap();
    for i in 0..103 { sqlx::query("INSERT INTO result VALUES (?, ?)").bind(i).bind(format!("row{i}")).execute(&pool).await.unwrap(); }
    for format in ["csv", "json", "sql"] {
        for limit in [0, 7] {
            let output = dir.path(&format!("limit{limit}.{format}"));
            assert_eq!(run_export_task(&db, &DbKind::of("sqlite"), &export(&output, format, "utf8", Some(limit))).await.unwrap(), limit);
            let text = decoded(&output, "utf8");
            match format {
                "csv" => { let mut reader = csv::Reader::from_reader(text.as_bytes()); assert_eq!(reader.headers().unwrap().iter().collect::<Vec<_>>(), vec!["id", "payload"]); assert_eq!(reader.records().count() as u64, limit); }
                "json" => assert_eq!(serde_json::from_str::<Vec<serde_json::Value>>(&text).unwrap().len() as u64, limit),
                "sql" => assert_eq!(text.matches("INSERT INTO").count() as u64, limit),
                _ => unreachable!(),
            }
        }
    }
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_cursor_preserves_binary_null_and_exact_int64() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (id INTEGER, payload BLOB, nullable TEXT)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO result VALUES (?, ?, NULL)").bind(i64::MAX).bind(vec![0u8, 255, 128, 39]).execute(&pool).await.unwrap();
    let output = dir.path("binary.json");
    assert_eq!(run_export_task(&db, &DbKind::of("sqlite"), &export(&output, "json", "utf8", None)).await.unwrap(), 1);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&decoded(&output, "utf8")).unwrap();
    assert_eq!(rows[0]["id"], i64::MAX.to_string()); assert_eq!(rows[0]["payload"], "0x00FF8027"); assert!(rows[0]["nullable"].is_null());
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_cursor_encoding_failure_or_existing_target_never_publishes_or_overwrites() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (payload TEXT)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO result VALUES ('cannot map 中文')").execute(&pool).await.unwrap();
    let existing = dir.path("existing.csv"); std::fs::write(&existing, b"keep original").unwrap();
    assert!(run_export_task(&db, &DbKind::of("sqlite"), &export(&existing, "csv", "utf8", None)).await.is_err()); assert_eq!(std::fs::read(&existing).unwrap(), b"keep original");
    let fresh = dir.path("invalid.csv"); assert!(run_export_task(&db, &DbKind::of("sqlite"), &export(&fresh, "csv", "latin1", None)).await.is_err()); assert!(!fresh.exists());
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM result").fetch_one(&pool).await.unwrap(), 1);
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_cursor_cancellation_before_send_has_no_output() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (id INTEGER)").execute(&pool).await.unwrap();
    let output = dir.path("cancelled.csv"); let options = export(&output, "csv", "utf8", None);
    let task = TaskControl::begin(&options.task_id).unwrap(); cancel_task(&options.task_id).unwrap();
    let stage = StagedOutput::new(output.to_str().unwrap()).unwrap();
    let result = transfer_export_cursor::export_cursor(&db, &DbKind::of("sqlite"), &options.options, &task, &stage).await;
    assert!(result.is_err()); task.finish(&result); assert_eq!(task_progress(&options.task_id).unwrap().status, "cancelled");
    drop(stage); assert!(!output.exists()); dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_sql_stream_preserves_trigger_body_and_counts_only_dml() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    let source = dir.path("trigger.sql");
    std::fs::write(&source, "CREATE TABLE result(id INTEGER); CREATE TABLE audit(value TEXT); CREATE TRIGGER tr AFTER INSERT ON result BEGIN INSERT INTO audit VALUES(CASE WHEN NEW.id=1 THEN 'a; END' ELSE 'b' END); END; INSERT INTO result VALUES(1);").unwrap();
    assert_eq!(run_import_data(&db, &DbKind::of("sqlite"), &import(&source, "utf8"), &[], None).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT value FROM audit").fetch_one(&pool).await.unwrap(), "a; END");
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_sql_stream_entire_preflight_rejects_late_controls_and_invalid_encoding_without_writes() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result(id INTEGER)").execute(&pool).await.unwrap();
    let source = dir.path("invalid.sql");
    for tail in [b"COMMIT;".as_slice(), b"BEGIN;", b"ATTACH 'other.sqlite' AS other;", b"\xe4\xb8"] {
        let mut file = File::create(&source).unwrap(); file.write_all(b"INSERT INTO result VALUES(1);").unwrap(); file.write_all(tail).unwrap(); drop(file);
        assert!(run_import_data(&db, &DbKind::of("sqlite"), &import(&source, "utf8"), &[], None).await.is_err());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM result").fetch_one(&pool).await.unwrap(), 0);
        dir.assert_no_stage();
    }
    pool.close().await;
}

#[tokio::test]
async fn sqlite_sql_stream_execution_error_confirms_same_transaction_rollback() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result(id INTEGER PRIMARY KEY)").execute(&pool).await.unwrap(); sqlx::query("INSERT INTO result VALUES(0)").execute(&pool).await.unwrap();
    let source = dir.path("duplicate.sql"); std::fs::write(&source, "INSERT INTO result VALUES(1);INSERT INTO result VALUES(1);").unwrap();
    let error = run_import_data(&db, &DbKind::of("sqlite"), &import(&source, "utf8"), &[], None).await.unwrap_err();
    assert!(format!("{error:#}").contains("回滚已确认")); assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM result").fetch_one(&pool).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT id FROM result").fetch_one(&pool).await.unwrap(), 0);
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_sql_stream_non_utf8_input_is_decoded_before_native_execution() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result(payload TEXT)").execute(&pool).await.unwrap();
    let source = dir.path("latin.sql"); std::fs::write(&source, b"INSERT INTO result VALUES('\x80\xe9\xff');").unwrap();
    assert_eq!(run_import_data(&db, &DbKind::of("sqlite"), &import(&source, "latin1"), &[], None).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_, String>("SELECT payload FROM result").fetch_one(&pool).await.unwrap(), "\u{0080}éÿ");
    dir.assert_no_stage(); pool.close().await;
}

#[tokio::test]
async fn sqlite_sql_stream_generated_large_source_does_not_need_whole_script_vector() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result(id INTEGER, payload TEXT)").execute(&pool).await.unwrap();
    let source = dir.path("large.sql"); let mut file = BufWriter::new(File::create(&source).unwrap());
    for id in 0..50_000 { writeln!(&mut file, "INSERT INTO result VALUES({id}, 'bounded statement payload abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz');").unwrap(); }
    file.flush().unwrap(); drop(file); assert!(std::fs::metadata(&source).unwrap().len() > 4 * 1024 * 1024);
    assert_eq!(run_import_data(&db, &DbKind::of("sqlite"), &import(&source, "utf8"), &[], None).await.unwrap(), 50_000);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM result").fetch_one(&pool).await.unwrap(), 50_000);
    dir.assert_no_stage(); pool.close().await;
}
