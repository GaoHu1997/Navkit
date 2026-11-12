use super::*;
use std::path::PathBuf;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("navkit-export-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap(); Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
}
impl Drop for TempDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn options(path: &Path, format: &str, encoding: &str) -> ExportOptions {
    ExportOptions { connection_id: "connection".into(), schema: "main".into(), table: "result".into(), file: path.to_string_lossy().into_owned(), format: format.into(), has_header: true, delimiter: ",".into(), encoding: encoding.into(), where_clause: None, limit: None }
}
fn names() -> Vec<String> { vec!["id".into(), "payload".into(), "nullable".into()] }
fn types() -> Vec<String> { vec!["TEXT".into(); 3] }
fn row(i: u64) -> Vec<Option<String>> {
    vec![Some(i.to_string()), Some(format!("中文🦀, \"quoted\" 'apostrophe'\r\nline #{i}")), if i % 2 == 0 { None } else { Some("\\N".into()) }]
}

#[test]
fn snapshot_exports_csv_json_sql_all_rows_and_special_values() {
    let dir = TempDir::new();
    let names = names(); let types = types();
    let set = crate::query_spool::test_set(&dir.0, &names, &types, u64::MAX);
    for i in 0..10_037 { set.push(&row(i)); }
    set.finish();
    let kind = DbKind::of("sqlite");
    for format in ["csv", "json", "sql"] {
        let output = dir.path(&format!("result.{format}"));
        assert_eq!(export_spool_set(&set, &options(&output, format, "utf-8"), &kind, &names, &types).unwrap(), 10_037);
        match format {
            "csv" => {
                let mut reader = csv::Reader::from_path(&output).unwrap();
                assert_eq!(reader.headers().unwrap().iter().collect::<Vec<_>>(), names.iter().map(String::as_str).collect::<Vec<_>>());
                let records = reader.records().collect::<std::result::Result<Vec<_>, _>>().unwrap();
                assert_eq!(records.len(), 10_037);
                assert_eq!(records[0].get(1), row(0)[1].as_deref());
                assert_eq!(records[0].get(2), Some("\\N"));
                assert_eq!(records[1].get(2), Some("\\\\N"));
                assert_eq!(records.last().unwrap().get(0), Some("10036"));
            }
            "json" => {
                let records: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
                assert_eq!(records.len(), 10_037);
                assert_eq!(records[0]["payload"].as_str(), row(0)[1].as_deref());
                assert!(records[0]["nullable"].is_null());
                assert_eq!(records[1]["nullable"], "\\N");
                assert_eq!(records.last().unwrap()["id"], "10036");
            }
            "sql" => {
                let text = std::fs::read_to_string(&output).unwrap();
                assert_eq!(text.matches("INSERT INTO").count(), 10_037);
                assert!(text.contains("''apostrophe''"));
                assert!(text.contains("中文🦀"));
                assert!(text.contains("NULL"));
                assert!(text.contains("'10036'"));
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn failed_exports_preserve_old_file_and_remove_staging_files() {
    let dir = TempDir::new();
    let names = names(); let types = types();
    let kind = DbKind::of("sqlite");
    let output = dir.path("result.json");
    let original = b"original destination";
    std::fs::write(&output, original).unwrap();
    for failure in ["quota", "eof", "json", "encoding", "unfinished"] {
        let fresh = dir.path(&format!("fresh-{failure}.json"));
        let set = crate::query_spool::test_set(&dir.0, &names, &types, if failure == "quota" { 1 } else { u64::MAX });
        set.push(&row(0));
        if failure != "unfinished" { set.finish(); }
        if failure == "eof" { crate::query_spool::corrupt_test_set(&set, b""); }
        if failure == "json" { crate::query_spool::corrupt_test_set(&set, b"broken JSON\n"); }
        let encoding = if failure == "encoding" { "latin1" } else { "utf-8" };
        assert!(export_spool_set(&set, &options(&fresh, "json", encoding), &kind, &names, &types).is_err(), "fresh {failure}");
        assert!(!fresh.exists(), "failed {failure} must not publish output");
        assert!(export_spool_set(&set, &options(&output, "json", encoding), &kind, &names, &types).is_err(), "existing {failure}");
        assert_eq!(std::fs::read(&output).unwrap(), original, "{failure}");
        assert!(!std::fs::read_dir(&dir.0).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")), "{failure}");
    }
}

#[test]
fn file_encoder_crosses_utf8_and_output_buffer_boundaries_without_loss() {
    let dir = TempDir::new();
    for (encoding, text) in [
        ("utf-8", "a".repeat(65_535) + "🦀中文\n" + &"尾🦀".repeat(40_000)),
        ("gb18030", "a".repeat(65_535) + "🦀中文\n" + &"尾🦀".repeat(40_000)),
        ("big5", "a".repeat(65_535) + "繁體中文\n" + &"尾中文".repeat(40_000)),
        ("latin1", "a".repeat(65_535) + "\u{0080}éÿ\n" + &"éÿ".repeat(40_000)),
    ] {
        let target = dir.path(&format!("{encoding}.encoded"));
        let stage = StagedOutput::new(target.to_str().unwrap()).unwrap();
        let mut encoder = stage.encoder(encoding).unwrap();
        for chunk in text.as_bytes().chunks(65_536) { encoder.write_all(chunk).unwrap(); encoder.flush().unwrap(); }
        encoder.finish().unwrap().get_ref().sync_all().unwrap();
        stage.publish(|| Ok(())).unwrap();
        assert_eq!(read_text(target.to_str().unwrap(), encoding).unwrap(), text, "{encoding}");
    }
}

#[test]
fn csv_null_and_backslash_literals_roundtrip_with_any_delimiter() {
    let dir = TempDir::new();
    let values = vec![None, Some(String::new()), Some("\\N".into()), Some("\\\\N".into()), Some("\\path".into()), Some("quoted\"\r\n中文".into())];
    let names = (0..values.len()).map(|i| format!("column{i}")).collect::<Vec<_>>();
    for delimiter in [",", "\\", "N"] {
        let output = dir.path(&format!("roundtrip-{}.csv", delimiter.as_bytes()[0]));
        export_csv(output.to_str().unwrap(), &names, &[values.clone()], delimiter, "utf-8", false).unwrap();
        let mut reader = csv::ReaderBuilder::new().has_headers(false).delimiter(delimiter.as_bytes()[0]).from_path(&output).unwrap();
        let record = reader.records().next().unwrap().unwrap();
        assert_eq!(record.iter().map(|v| csv_value(v.to_owned())).collect::<Vec<_>>(), values);
    }
}

#[test]
fn invalid_utf8_and_unmappable_characters_leave_no_output() {
    let dir = TempDir::new();
    for (bytes, encoding) in [(b"bad\xff".as_slice(), "gb18030"), ("emoji🦀".as_bytes(), "big5"), ("中文".as_bytes(), "latin1")] {
        let target = dir.path("invalid.out");
        {
            let stage = StagedOutput::new(target.to_str().unwrap()).unwrap();
            let mut encoder = stage.encoder(encoding).unwrap();
            assert!(encoder.write_all(bytes).is_err());
            assert!(encoder.finish().is_err());
        }
        assert!(!target.exists());
    }
    let target = dir.path("existing.out");
    std::fs::write(&target, b"old").unwrap();
    assert!(StagedOutput::new(target.to_str().unwrap()).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
}

#[test]
fn snapshot_streams_each_supported_encoding_without_replacement() {
    let dir = TempDir::new();
    let names = vec!["payload".to_string()]; let types = vec!["TEXT".to_string()];
    for (encoding, text) in [("utf-8", "中文🦀\r\n"), ("gb18030", "中文🦀\r\n"), ("big5", "繁體中文\r\n"), ("latin1", "\u{0080}éÿ\r\n")] {
        let set = crate::query_spool::test_set(&dir.0, &names, &types, u64::MAX);
        let value = "a".repeat(65_535) + &text.repeat(20_000);
        set.push(&[Some(value.clone())]); set.finish();
        let output = dir.path(&format!("stream-{encoding}.json"));
        assert_eq!(export_spool_set(&set, &options(&output, "json", encoding), &DbKind::of("sqlite"), &names, &types).unwrap(), 1);
        let text = read_text(output.to_str().unwrap(), encoding).unwrap();
        let exported: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(exported[0]["payload"].as_str(), Some(value.as_str()), "{encoding}");
    }
}

#[test]
fn oversized_preview_does_not_disable_complete_snapshot_export() {
    let dir = TempDir::new();
    let names = vec!["payload".to_string()]; let types = vec!["TEXT".to_string()];
    let set = crate::query_spool::test_set(&dir.0, &names, &types, u64::MAX);
    let value = "x".repeat(9 * 1024 * 1024);
    set.push(&[Some(value.clone())]); set.finish();
    assert!(set.fetch(0, 1).is_err());
    let output = dir.path("large-preview.json");
    assert_eq!(export_spool_set(&set, &options(&output, "json", "utf-8"), &DbKind::of("sqlite"), &names, &types).unwrap(), 1);
    let exported: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(exported[0]["payload"].as_str(), Some(value.as_str()));
}

#[test]
fn snapshot_refuses_partial_limit_existing_target_and_oversized_encoded_record() {
    let dir = TempDir::new();
    let names = vec!["payload".to_string()]; let types = vec!["TEXT".to_string()];
    let set = crate::query_spool::test_set(&dir.0, &names, &types, u64::MAX);
    set.push(&[Some("first".into())]); set.push(&[Some("last".into())]); set.finish();
    let output = dir.path("partial.json");
    let mut opts = options(&output, "json", "utf-8"); opts.limit = Some(1);
    assert!(export_spool_set(&set, &opts, &DbKind::of("sqlite"), &names, &types).unwrap_err().to_string().contains("完整查询快照"));
    assert!(!output.exists());
    opts.limit = Some(2);
    std::fs::write(&output, b"old destination").unwrap();
    assert!(export_spool_set(&set, &opts, &DbKind::of("sqlite"), &names, &types).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"old destination");
    let large = crate::query_spool::test_set(&dir.0, &names, &types, u64::MAX);
    large.push(&[Some("x".repeat(16 * 1024 * 1024 + 1))]); large.finish();
    let fresh = dir.path("oversized.json");
    assert!(export_spool_set(&large, &options(&fresh, "json", "utf-8"), &DbKind::of("sqlite"), &names, &types).is_err());
    assert!(!fresh.exists());
    assert!(!std::fs::read_dir(&dir.0).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".navkit-export-")));
}

