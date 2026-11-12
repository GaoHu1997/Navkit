use super::*;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("navkit-spool-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TempDir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
fn names() -> Vec<String> { vec!["id".into(), "text".into()] }
fn row(id: u64) -> Vec<Option<String>> { vec![Some(id.to_string()), Some(format!("引号\"、换行\n、逗号,、emoji🦀 #{id}"))] }

#[test]
fn pages_cross_ten_thousand_and_index_boundaries() {
    let dir = TempDir::new();
    let set = test_set(&dir.0, &names(), &[], MAX_SET_BYTES);
    for i in 0..10_037 { set.push(&row(i)); }
    assert!(set.fetch(0, 1).is_err(), "运行中的快照不得被当成完整页");
    set.finish();
    assert!(!set.truncated());
    assert_eq!(set.total(), 10_037);
    for offset in [0, 1023, 1024, 9999, 10_000, 10_036] {
        let page = set.fetch(offset, 5000).unwrap();
        assert_eq!(page.first().unwrap(), &row(offset));
        assert_eq!(page.last().unwrap(), &row((offset + 4999).min(10_036)));
    }
    assert!(set.fetch(10_037, 5000).unwrap().is_empty());
    assert!(set.fetch(0, 5001).is_err());
    let mut count = 0;
    assert_eq!(set.stream(2000, |rows| {
        for values in rows { assert_eq!(values, row(count)); count += 1; }
        Ok(())
    }).unwrap(), 10_037);
    assert_eq!(count, 10_037);
}

#[test]
fn page_bytes_are_bounded_and_large_first_row_is_explicit() {
    let dir = TempDir::new();
    let set = test_set(&dir.0, &["value".into()], &[], MAX_SET_BYTES);
    for _ in 0..6 { set.push(&[Some("x".repeat(2 * 1024 * 1024))]); }
    set.finish();
    let page = set.fetch(0, 5000).unwrap();
    assert_eq!(page.len(), 3);
    assert!(page.iter().map(|r| row_bytes(r) + std::mem::size_of_val(r)).sum::<usize>() <= MAX_PAGE_BYTES);
    let mut count = 0;
    set.stream(2000, |rows| { assert!(rows.len() <= 1); count += rows.len(); Ok(()) }).unwrap();
    assert_eq!(count, 6);
    let giant = test_set(&dir.0, &["value".into()], &[], MAX_SET_BYTES);
    giant.push(&[Some("大".repeat(MAX_PAGE_BYTES / 3 + 1))]);
    giant.finish();
    assert!(giant.fetch(0, 1).unwrap_err().to_string().contains("8 MiB"));
    assert_eq!(giant.stream(2000, |rows| { assert_eq!(rows.len(), 1); Ok(()) }).unwrap(), 1);
}

#[test]
fn quota_and_flush_failure_never_claim_complete() {
    let dir = TempDir::new();
    let quota = test_set(&dir.0, &names(), &[], 128);
    for i in 0..10 { quota.push(&row(i)); }
    quota.finish();
    assert!(quota.truncated());
    assert!(quota.total() < 10);
    assert!(quota.ensure_complete().is_err());
    assert!(quota.stream(1, |_| Ok(())).is_err());

    let failed = test_set(&dir.0, &names(), &[], MAX_SET_BYTES);
    // 只读文件句柄允许 BufWriter 先接受短行，但 flush 必然失败。
    failed.writer.lock().out = Some(BufWriter::new(File::open(&failed.file).unwrap()));
    failed.push(&row(1));
    failed.finish();
    failed.finish();
    assert!(failed.truncated());
    assert!(failed.stop_reason().unwrap().contains("刷新"));
    assert!(failed.ensure_complete().is_err());
}

#[test]
fn unexpected_eof_invalid_json_extra_rows_and_columns_fail() {
    let dir = TempDir::new();
    for corrupted in [b"".as_slice(), b"[null,\"unterminated\"]", b"[null]\n", b"[null,null,null]\n", b"[null,null]\n[null,null]\n", b"not JSON\n"] {
        let set = test_set(&dir.0, &names(), &[], MAX_SET_BYTES);
        set.push(&row(1)); set.finish();
        corrupt_test_set(&set, corrupted);
        assert!(set.stream(1, |_| Ok(())).is_err(), "损坏缓存不得导出成功: {corrupted:?}");
    }
}

#[test]
fn dropping_registry_preserves_active_set_until_last_arc() {
    let dir = TempDir::new();
    let spool = Arc::new(Spool {
        id: uuid::Uuid::new_v4().to_string(), connection: "connection".into(), owner: "tab".into(), namespace: "schema".into(), principal: "alice".into(),
        dir: dir.0.clone(), sets: parking_lot::Mutex::new(Vec::new()), created: Instant::now(), completed: AtomicBool::new(false),
    });
    registry().lock().insert(spool.id.clone(), spool.clone());
    let set = spool.begin_set(0, &names(), &[]);
    set.push(&row(0)); spool.finish_all();
    assert!(!spool.idle(), "导出持有 set Arc 时不得淘汰缓存");
    assert!(authorize(&spool.id, "connection", "tab", "schema", "mallory").is_err());
    assert!(authorize(&spool.id, "connection", "tab", "schema", "alice").is_ok());
    let path = set.file.clone();
    drop_spool(&spool.id);
    assert!(!exists(&spool.id));
    drop(spool);
    assert!(path.exists());
    assert_eq!(set.fetch(0, 1).unwrap(), vec![row(0)]);
    drop(set);
    assert!(!path.exists());
}

#[test]
fn failed_creation_is_retained_and_finished() {
    let dir = TempDir::new();
    let obstructed = dir.0.join("not-a-directory");
    std::fs::write(&obstructed, b"file").unwrap();
    let spool = Spool {
        id: uuid::Uuid::new_v4().to_string(), connection: String::new(), owner: String::new(), namespace: String::new(), principal: String::new(),
        dir: obstructed, sets: parking_lot::Mutex::new(Vec::new()), created: Instant::now(), completed: AtomicBool::new(false),
    };
    let set = spool.begin_set(0, &names(), &[]);
    assert!(Arc::ptr_eq(&set, &spool.set(0).unwrap()));
    spool.finish_all();
    assert!(set.writer.lock().finished);
    assert!(set.truncated());
    assert!(set.ensure_complete().is_err());
}
