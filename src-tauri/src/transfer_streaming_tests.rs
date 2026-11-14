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
    ExportTaskOptions { sql_batch_rows: 0, sql_batch_bytes: 0, task_id: uuid::Uuid::new_v4().to_string(), options: ExportOptions { connection_id: "test".into(), schema: "main".into(), table: "result".into(), file: path.to_str().unwrap().into(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: encoding.into(), where_clause: None, limit } }
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

#[test]
fn csv_header_limits_include_escaping_and_allow_the_exact_boundary() {
    let dir = Directory::new();
    let max = TransferLimits::default().max_record_bytes as usize;
    let kind = DbKind::of("sqlite");
    let output = dir.path("exact.csv");
    let stage = StagedOutput::new(output.to_str().unwrap()).unwrap();
    // 每个名字均在 256 字节限制内；65536 列连同逗号与 CRLF 恰好 16 MiB。
    let mut names: Vec<String> = (0..max / 256)
        .map(|i| format!("{i:08}{}", "a".repeat(247)))
        .collect();
    names.last_mut().unwrap().pop();
    let sink = ExportSink::create(&stage, "csv", &kind, "main", "result", &names, &[], ",", true, "utf8").unwrap();
    sink.finish().unwrap();
    stage.publish(|| Ok(())).unwrap();
    assert_eq!(std::fs::metadata(&output).unwrap().len(), max as u64);
    let rejected = dir.path("expanded.csv");
    let stage = StagedOutput::new(rejected.to_str().unwrap()).unwrap();
    let names: Vec<String> = (0..max / 256)
        .map(|i| format!("{i:08}{}", char::from(34).to_string().repeat(247)))
        .collect();
    let error = match ExportSink::create(&stage, "csv", &kind, "main", "result", &names, &[], ",", true, "utf8") {
        Ok(_) => panic!("escaped header must be bounded before allocation"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("CSV escaped header"));
    drop(stage);
    assert!(!rejected.exists());
    dir.assert_no_stage();
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

// ---------------------------------------------------------------- P1：有界预览

#[test]
fn preview_csv_reads_only_header_and_bounded_sample_rows() {
    let dir = Directory::new();
    let path = dir.path("big.csv");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    writeln!(&mut file, "id,name").unwrap();
    for i in 0..5000 { writeln!(&mut file, "{i},row-{i}").unwrap(); }
    file.flush().unwrap(); drop(file);
    let (names, rows) = preview_csv(path.to_str().unwrap(), ",", "utf8", 20).unwrap();
    assert_eq!(names, vec!["id", "name"]);
    assert_eq!(rows.len(), 20);
    assert_eq!(rows[0], vec!["0", "row-0"]);
    assert_eq!(rows[19], vec!["19", "row-19"]);
    // limit=0 只读表头；超过 1000 的 limit 收敛到 1000。
    let (names, rows) = preview_csv(path.to_str().unwrap(), ",", "utf8", 0).unwrap();
    assert_eq!(names, vec!["id", "name"]); assert!(rows.is_empty());
    let (_, rows) = preview_csv(path.to_str().unwrap(), ",", "utf8", 5000).unwrap();
    assert_eq!(rows.len(), 1000);
    dir.assert_no_stage();
}

#[test]
fn preview_csv_keeps_bom_encoding_and_empty_file_semantics() {
    let dir = Directory::new();
    // UTF-8 BOM 必须被剥离，表头不携带 \u{feff}。
    let bom = dir.path("bom.csv");
    std::fs::write(&bom, "\u{feff}id,name\n1,中文\n").unwrap();
    let (names, rows) = preview_csv(bom.to_str().unwrap(), ",", "utf8", 10).unwrap();
    assert_eq!(names, vec!["id", "name"]); assert_eq!(rows, vec![vec!["1", "中文"]]);
    // GBK 编码与旧 read_text 路径一致。
    let gbk = dir.path("gbk.csv");
    let (bytes, _, _) = encoding_rs::GBK.encode("id,name\n1,中文\n");
    std::fs::write(&gbk, bytes.as_ref()).unwrap();
    let (names, rows) = preview_csv(gbk.to_str().unwrap(), ",", "gbk", 10).unwrap();
    assert_eq!(names, vec!["id", "name"]); assert_eq!(rows, vec![vec!["1", "中文"]]);
    // 空文件：空表头、空行，不报错。
    let empty = dir.path("empty.csv"); std::fs::write(&empty, b"").unwrap();
    let (names, rows) = preview_csv(empty.to_str().unwrap(), ",", "utf8", 10).unwrap();
    assert!(names.is_empty()); assert!(rows.is_empty());
    dir.assert_no_stage();
}

#[test]
fn preview_csv_does_not_parse_beyond_the_sample_window() {
    let dir = Directory::new();
    let path = dir.path("late-error.csv");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    writeln!(&mut file, "a,b").unwrap();
    for i in 0..30 { writeln!(&mut file, "{i},ok").unwrap(); }
    writeln!(&mut file, "1,2,3").unwrap(); // 窗口外的坏行：列数不一致
    file.flush().unwrap(); drop(file);
    // 预览只读前 20 行，窗口外坏行不影响预览（有界语义）。
    let (_, rows) = preview_csv(path.to_str().unwrap(), ",", "utf8", 20).unwrap();
    assert_eq!(rows.len(), 20);
    // 请求覆盖坏行时仍然如实报错，不静默吞掉。
    assert!(preview_csv(path.to_str().unwrap(), ",", "utf8", 1000).is_err());
    dir.assert_no_stage();
}

#[test]
fn preview_csv_wide_rows_stop_at_byte_budget() {
    let dir = Directory::new();
    let path = dir.path("wide.csv");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    writeln!(&mut file, "payload").unwrap();
    let wide = "a".repeat(512 * 1024);
    for _ in 0..40 { writeln!(&mut file, "{wide}").unwrap(); }
    file.flush().unwrap(); drop(file);
    // 40 行 × 512 KiB = 20 MiB 超过 8 MiB 预览字节闸：提前收尾而不是全量读入。
    let (names, rows) = preview_csv(path.to_str().unwrap(), ",", "utf8", 1000).unwrap();
    assert_eq!(names, vec!["payload"]);
    assert!(!rows.is_empty() && rows.len() < 40, "rows={}", rows.len());
    assert!(rows.iter().all(|r| r[0].len() == 512 * 1024));
    dir.assert_no_stage();
}

// ------------------------------------------------- P1：遗留导出统一到流式管线

#[tokio::test]
async fn legacy_streaming_export_matches_interactive_pipeline() {
    let dir = Directory::new(); let (db, pool) = database(&dir).await;
    sqlx::query("CREATE TABLE result (id INTEGER PRIMARY KEY, payload TEXT, nullable TEXT)").execute(&pool).await.unwrap();
    for i in 0..257 {
        sqlx::query("INSERT INTO result VALUES (?, ?, ?)").bind(i).bind(format!("值-{i},\"quote\"\nline")).bind(if i % 3 == 0 { None } else { Some(format!("n{i}")) }).execute(&pool).await.unwrap();
    }
    let kind = DbKind::of("sqlite");
    for format in ["csv", "json", "sql"] {
        let legacy = dir.path(&format!("legacy.{format}"));
        let interactive = dir.path(&format!("interactive.{format}"));
        let mut options = export(&legacy, format, "utf8", None).options;
        options.file = legacy.to_str().unwrap().into();
        let count = export_table_streaming(&db, &kind, &options).await.unwrap();
        assert_eq!(count, 257);
        let task_options = export(&interactive, format, "utf8", None);
        assert_eq!(run_export_task(&db, &kind, &task_options).await.unwrap(), 257);
        // 遗留入口与交互入口产出完全一致（同一游标/写入器/发布管线）。
        assert_eq!(std::fs::read(&legacy).unwrap(), std::fs::read(&interactive).unwrap(), "format={format}");
    }
    // where_clause 与 limit 语义保留。
    let filtered = dir.path("filtered.csv");
    let mut options = export(&filtered, "csv", "utf8", Some(5)).options;
    options.where_clause = Some("id >= 100".into());
    assert_eq!(export_table_streaming(&db, &kind, &options).await.unwrap(), 5);
    let text = decoded(&filtered, "utf8");
    assert!(text.contains("值-100")); assert!(!text.contains("值-99"));
    // SQL 输出可回灌到空表并还原全部行值（round-trip）。
    sqlx::query("CREATE TABLE restored (id INTEGER PRIMARY KEY, payload TEXT, nullable TEXT)").execute(&pool).await.unwrap();
    let sql_path = dir.path("legacy.sql");
    let original_target = crate::db::qualified(&kind, "main", "result");
    let restored_target = crate::db::qualified(&kind, "main", "restored");
    let original_prefix = format!("INSERT INTO {original_target} (");
    let text = decoded(&sql_path, "utf8");
    assert_eq!(text.matches(&original_prefix).count(), 257, "verify the producer's actual qualified target before rewriting");
    let text = text.replace(&original_prefix, &format!("INSERT INTO {restored_target} ("));
    let rewritten = dir.path("restored.sql"); std::fs::write(&rewritten, text.as_bytes()).unwrap();
    let mut import_opts = import(&rewritten, "utf8"); import_opts.table = "restored".into();
    run_import_data(&db, &kind, &import_opts, &[], None).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM restored").fetch_one(&pool).await.unwrap(), 257);
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM restored WHERE nullable IS NULL").fetch_one(&pool).await.unwrap(), 86);
    dir.assert_no_stage(); pool.close().await;
}
