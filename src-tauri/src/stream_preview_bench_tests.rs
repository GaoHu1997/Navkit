//! 环境门控真实 MySQL 服务基准；不改变产品执行、缓存或前端。
//! 由 tools/tests/stream-preview-bench.mjs 创建并拥有 UUID 数据库。
use super::*;
use crate::execution_stream::StreamOptions;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::time::Instant;

const CONNECTION: &str = "stream-bench";
const OWNER: &str = "stream-bench-tab";
const PRINCIPAL: &str = "main";
const PAYLOAD: &str = "abcdefghijklmnop";

struct MysqlFixture { state: Arc<AppState>, root: PathBuf, namespace: String }
impl MysqlFixture {
    async fn new() -> Self {
        assert_eq!(std::env::var("NAVKIT_STREAM_BENCH").as_deref(), Ok("1"));
        let namespace = std::env::var("NAVKIT_STREAM_BENCH_DATABASE").expect("独立数据库必填");
        let suffix = namespace.strip_prefix("navkit_stream_bench_").expect("只接受本轮独立库");
        assert_eq!(suffix.len(), 32);
        assert!(suffix.bytes().all(|b| b.is_ascii_hexdigit()));
        let host = std::env::var("NAVKIT_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        assert!(["127.0.0.1", "localhost", "::1"].contains(&host.as_str()));
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        // 测试 exe 的便携 data 也必须在当前 worktree 内，禁止写主树。
        assert!(crate::storage::portable_root().canonicalize().unwrap().starts_with(&cwd));
        let root = cwd.join("artifacts/stream-preview-eval-20261115/runtime")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::empty_at(&root);
        let password = zeroize::Zeroizing::new(std::env::var("NAVKIT_MYSQL_PASSWORD").expect("仅允许已授权 fixture 凭据"));
        let cfg = ConnectionConfig {
            id: CONNECTION.into(), name: "隔离基准".into(), kind: DbKind::of("mysql"), host,
            port: std::env::var("NAVKIT_MYSQL_PORT").unwrap_or_else(|_| "3306".into()).parse().unwrap(),
            user: std::env::var("NAVKIT_MYSQL_USER").unwrap_or_else(|_| "root".into()),
            password: store.encrypt_password(&password).unwrap_or_else(|_| panic!("夹具凭据加密失败")),
            database: namespace.clone(), file: String::new(), params: String::new(), group: String::new(),
            read_only: false, keep_alive: false,
        };
        store.save_connections(vec![cfg.clone()], vec![]).unwrap_or_else(|_| panic!("夹具保存失败（凭据隐藏）"));
        let state = Arc::new(AppState { store, registry: Default::default(), manual_transactions: Default::default(),
            activity: Default::default(), executions: Default::default(), mutations: Default::default(),
            connection_runtime: Default::default(), query_results: Default::default() });
        let conn = Arc::new(db::open_saved(&db::request_from_config(&cfg, &password), false, &state.store.settings())
            .await.unwrap_or_else(|_| panic!("本机夹具连接失败（凭据隐藏）")));
        let fingerprint = db::runtime::binding_fingerprint_at(&cfg, &root).unwrap();
        state.connection_runtime.publish(CONNECTION, &fingerprint, &conn, None,
            db::runtime::keepalive_policy(&cfg).unwrap()).unwrap();
        state.registry.write().insert(CONNECTION.into(), conn);
        // 预先建立产品命名空间连接；两种服务都使用同一池及热连接。
        crate::commands::scoped_db(&state, CONNECTION, &namespace).await
            .unwrap_or_else(|_| panic!("命名空间连接失败（凭据隐藏）"));
        Self { state, root, namespace }
    }
    fn request(&self, sql: &str) -> ExecutionRequest {
        let mut request = ExecutionRequest { execution_id: String::new(), connection_id: CONNECTION.into(),
            namespace: self.namespace.clone(), owner_id: OWNER.into(), sql: sql.into(), transaction_owner: None,
            timeout_ms: Some(1_800_000), origin: Some("query".into()), cancel_before_send: false, execution_grant: None };
        let cfg = self.state.store.connections().into_iter().find(|c| c.id == CONNECTION).unwrap();
        let binding = crate::commands::execution_binding(&self.state, &cfg, PRINCIPAL, OWNER, &self.namespace).unwrap();
        self.state.executions.grant_request(&mut request, binding).unwrap();
        request
    }
    async fn cleanup(self) {
        crate::sql_history::flush();
        assert!(!self.state.executions.has_active(CONNECTION), "不能清理仍在执行的夹具");
        let conns: Vec<_> = self.state.registry.write().drain().map(|(_, conn)| conn).collect();
        for conn in conns { db::shutdown_db(conn.as_ref()).await; }
        let root = self.root.clone();
        drop(self);
        assert!(root.canonicalize().unwrap().starts_with(std::env::current_dir().unwrap().canonicalize().unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }
}

// 系统 API 读取本测试进程内存和 CPU；无可用 API 时保留 null。
#[derive(Clone, Copy, Default)]
struct Counters { rss: Option<u64>, private: Option<u64>, cpu_ms: Option<f64>, free_ram: Option<u64>, free_disk: Option<u64> }
#[cfg(windows)]
fn counters() -> Counters {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[repr(C)]
    struct Memory { cb: u32, faults: u32, peak_rss: usize, rss: usize, peak_paged: usize, paged: usize,
        peak_nonpaged: usize, nonpaged: usize, pagefile: usize, peak_pagefile: usize, private: usize }
    #[repr(C)]
    #[derive(Default)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    struct Global { length: u32, load: u32, total: u64, available: u64, page_total: u64, page_available: u64,
        virtual_total: u64, virtual_available: u64, extended: u64 }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn K32GetProcessMemoryInfo(p: *mut c_void, info: *mut Memory, size: u32) -> i32;
        fn GetProcessTimes(p: *mut c_void, created: *mut FileTime, exited: *mut FileTime,
            kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GlobalMemoryStatusEx(info: *mut Global) -> i32;
        fn GetDiskFreeSpaceExW(path: *const u16, free: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    unsafe {
        let process = GetCurrentProcess();
        let mut m: Memory = std::mem::zeroed(); m.cb = std::mem::size_of::<Memory>() as u32;
        let mut c = Counters::default();
        if K32GetProcessMemoryInfo(process, &mut m, std::mem::size_of::<Memory>() as u32) != 0 {
            c.rss = Some(m.rss as u64); c.private = Some(m.private as u64);
        }
        let (mut created, mut exited, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        if GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
            let ticks = |t: FileTime| (u64::from(t.high) << 32) | u64::from(t.low);
            c.cpu_ms = Some((ticks(kernel) + ticks(user)) as f64 / 10_000.0);
        }
        let mut g: Global = std::mem::zeroed(); g.length = std::mem::size_of::<Global>() as u32;
        if GlobalMemoryStatusEx(&mut g) != 0 { c.free_ram = Some(g.available); }
        let path: Vec<u16> = std::env::current_dir().unwrap().as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0;
        if GetDiskFreeSpaceExW(path.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) != 0 { c.free_disk = Some(free); }
        c
    }
}
#[cfg(not(windows))]
fn counters() -> Counters { Counters::default() }

fn stop_reason(c: Counters) -> Option<&'static str> {
    if c.private.is_some_and(|v| v > 4 * 1024 * 1024 * 1024) { Some("客户端 private bytes 超过 4GiB") }
    else if c.free_ram.is_some_and(|v| v < 2 * 1024 * 1024 * 1024) { Some("系统可用内存低于 2GiB") }
    else if c.free_disk.is_some_and(|v| v < 40 * 1024 * 1024 * 1024) { Some("磁盘可用空间低于 40GiB") }
    else { None }
}

fn cache_file(spool_id: &str) -> PathBuf {
    let dir = crate::storage::data_dir().join("cache/spool");
    dir.join(format!("{spool_id}-0.jsonl"))
}

async fn measure(f: &MysqlFixture, sql: &str, streaming: bool, source_rows: u64, expected: u64, round: usize, control: bool) -> Value {
    let table_rows: u64 = std::env::var("NAVKIT_STREAM_BENCH_TABLE_ROWS").ok()
        .map(|v| v.parse().unwrap()).unwrap_or(source_rows);
    assert!(table_rows >= source_rows);
    let request = f.request(sql); // 两边授权均在 submit 之前颁发。
    let execution_id = request.execution_id.clone();
    let baseline = counters();
    assert!(stop_reason(baseline).is_none(), "开始前资源止损触发");
    let started = Instant::now();
    let mut first_ms = None;
    let mut preview_state = None;
    let mut pending_without_rows = 0;
    let mut preview_first = None;
    let mut preview_last = None;
    let entry;
    let worker;
    if streaming {
        let registered = crate::commands::register_streaming_execution(&f.state, &request, PRINCIPAL, StreamOptions::default()).unwrap();
        entry = registered.2.clone();
        let state = f.state.clone();
        crate::execution_stream::launch(entry.clone(), async move {
            crate::commands::execute_registered_sql_service(&state, request, PRINCIPAL, registered).await
        });
        worker = None;
    } else {
        let state = f.state.clone();
        let id = execution_id.clone();
        let work = tokio::spawn(async move { execute_sql_service(&state, request, PRINCIPAL).await.unwrap() });
        loop {
            if let Some(e) = f.state.executions.entry_for_test(&id) { entry = e; break; }
            tokio::task::yield_now().await;
        }
        worker = Some(work);
    }
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
    let watched = entry.clone();
    let monitor = tokio::spawn(async move {
        let mut peak = baseline;
        let mut stopped = None;
        loop {
            let c = counters();
            peak.rss = peak.rss.max(c.rss); peak.private = peak.private.max(c.private);
            peak.free_ram = peak.free_ram.into_iter().chain(c.free_ram).min();
            peak.free_disk = peak.free_disk.into_iter().chain(c.free_disk).min();
            if stopped.is_none() {
                let reason = stop_reason(c).or_else(|| (started.elapsed() > Duration::from_secs(1800)).then_some("单轮超过30分钟"));
                if let Some(reason) = reason { watched.request_stop(false); stopped = Some(reason); }
            }
            tokio::select! { _ = &mut done_rx => break, _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
        }
        (peak, stopped)
    });
    let outcome;
    let final_ms;
    if streaming {
        // wait_first 的 1秒 pending 不记作首屏；继续同一次 SQL 的认证快照。
        let progress = entry.stream().unwrap();
        loop {
            let snapshot = progress.wait_first(&entry).await;
            if let Some(preview) = snapshot.preview {
                first_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(preview.rows.len(), 200);
                assert_eq!(preview.rows[0], vec![Some("1".into()), Some(PAYLOAD.into())]);
                assert_eq!(preview.rows[199][0].as_deref(), Some("200"));
                preview_state = Some(format!("{:?}", snapshot.status.state));
                preview_first = Some(1u64); preview_last = Some(200u64);
                break;
            }
            if snapshot.outcome.is_some() { break; }
            pending_without_rows += 1;
        }
        loop {
            let status = entry.status();
            if status.state.terminal() && !status.resources_pending { break; }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        outcome = crate::commands::stream_snapshot_service(&f.state, PRINCIPAL, &execution_id, CONNECTION, OWNER, &f.namespace)
            .unwrap().outcome.expect("终态必须有 outcome");
        final_ms = started.elapsed().as_secs_f64() * 1000.0;
        if outcome.state == ExecutionState::Succeeded { assert!(outcome.result.as_ref().unwrap().sets[0].rows.is_empty()); }
    } else {
        outcome = worker.unwrap().await.unwrap();
        final_ms = started.elapsed().as_secs_f64() * 1000.0;
    }
    // 不把不确定终态作为资源完成；必须等待真实排空再读缓存和下一轮。
    while entry.status().resources_pending { tokio::time::sleep(Duration::from_millis(10)).await; }
    let drained_ms = started.elapsed().as_secs_f64() * 1000.0;
    let cpu_end = counters();
    let _ = done_tx.send(());
    let (peak, stopped) = monitor.await.unwrap();
    let mut result = json!({"source_rows":table_rows,"range_upper_bound":source_rows,"output_rows":expected,"limit_200_control":control,"round":round,
        "mode":if streaming {"stream"} else {"old"},"sql":sql,"execution_id":execution_id,
        "preview_200_ms":first_ms,"preview_state":preview_state,"preview_first_id":preview_first,"preview_last_id":preview_last,
        "pending_without_rows":pending_without_rows,"final_ms":final_ms,"drained_ms":drained_ms,
        "state":format!("{:?}",outcome.state),"resources_pending":entry.status().resources_pending,
        "stop_reason":stopped,"rss_peak_bytes":peak.rss,"private_peak_bytes":peak.private,
        "minimum_free_ram_bytes":peak.free_ram,"minimum_free_disk_bytes":peak.free_disk,
        "process_cpu_ms":baseline.cpu_ms.zip(cpu_end.cpu_ms).map(|(a,b)|b-a),
        "build_mode":if cfg!(debug_assertions) {"debug"} else {"release"},
        // 基线必须由隔离脚本显式传入；不再写死历史提交，避免合并后身份记录失真。
        "baseline":std::env::var("NAVKIT_STREAM_BENCH_BASELINE").ok().filter(|value| !value.trim().is_empty()),
        "baseline_declared":std::env::var("NAVKIT_STREAM_BENCH_BASELINE").is_ok(),
        "measurement_scope":"Rust命令服务，不含IPC/WebView/DOM绘制；内存100ms采样，最终终态1ms轮询"});
    if outcome.state == ExecutionState::Succeeded {
        assert!(!outcome.resources_pending);
        let multi = outcome.result.as_ref().unwrap();
        assert_eq!(multi.sets.len(), 1);
        assert_eq!(multi.sets[0].total, expected);
        if !streaming { assert_eq!(multi.sets[0].rows[0][0].as_deref(), Some("1")); }
        let id = multi.spool_id.as_ref().unwrap();
        let spool = crate::query_spool::authorize(id, CONNECTION, OWNER, &f.namespace, PRINCIPAL).unwrap();
        let set = spool.set(0).unwrap();
        let first_page = set.fetch(0, 200).unwrap();
        assert_eq!(first_page.len(), 200);
        assert_eq!(first_page[0][0].as_deref(), Some("1"));
        result["first_200_available_ms"] = json!(if streaming {
            first_ms.unwrap_or_else(|| started.elapsed().as_secs_f64() * 1000.0)
        } else {final_ms});
        result["first_200_source"] = json!(if !streaming {"old_final_receipt"} else if first_ms.is_some() {"running_preview"} else {"terminal_spool_fetch"});
        let file = cache_file(id);
        let bytes = std::fs::metadata(&file).unwrap().len();
        let mut count = 0u64;
        let mut tail = None;
        for line in BufReader::new(std::fs::File::open(&file).unwrap()).lines() {
            let line = line.unwrap();
            count += 1;
            if count == 1 { assert_eq!(serde_json::from_str::<Vec<Option<String>>>(&line).unwrap()[0].as_deref(), Some("1")); }
            tail = Some(line);
        }
        assert!(count > 0);
        // SpoolSet::total 是已成功缓存的行数；完整读取总数由 result.sets[0].total 验证。
        assert_eq!(set.total(), count);
        assert!(bytes <= 512 * 1024 * 1024);
        let tail: Vec<Option<String>> = serde_json::from_str(&tail.unwrap()).unwrap();
        assert_eq!(tail[0], Some(count.to_string())); assert_eq!(tail[1].as_deref(), Some(PAYLOAD));
        assert_eq!(set.fetch(count - 1, 200).unwrap()[0], tail);
        if set.truncated() { assert!(set.ensure_complete().is_err()); assert!(count < expected); }
        else { set.ensure_complete().unwrap(); assert_eq!(count, expected); }
        result["total_drained_rows"] = json!(multi.sets[0].total);
        result["cached_rows"] = json!(count); result["cached_bytes"] = json!(bytes);
        result["cache_truncated"] = json!(set.truncated());
        result["complete_export_allowed"] = json!(set.ensure_complete().is_ok());
        result["cache_stop_reason"] = json!(set.stop_reason());
        result["cached_first_id"] = json!(1); result["cached_last_id"] = json!(count);
        result["old_receipt_rows"] = json!(if streaming {0} else {multi.sets[0].rows.len()});
        crate::query_spool::drop_spool(id);
        drop(set); drop(spool);
        assert!(!file.exists(), "每轮必须真实释放缓存配额和文件");
    }
    crate::sql_history::flush();
    let logs = std::fs::read_to_string(f.root.join("logs/sql_log.jsonl")).unwrap();
    let executions = logs.lines().filter(|line| {
        let v: Value = serde_json::from_str(line).unwrap();
        v["operation_id"] == execution_id && v["sql"] == sql && v["origin"] == "query"
    }).count();
    assert_eq!(executions, 1, "同一次 SQL 必须只记录一次");
    result["sql_log_count"] = json!(executions);
    assert!(!f.state.executions.has_active(CONNECTION));
    let path = std::env::var("NAVKIT_STREAM_BENCH_OUTPUT").expect("逐轮证据文件必填");
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    writeln!(file, "{}", result).unwrap(); file.sync_all().unwrap();
    eprintln!("STREAM_BENCH {}", result);
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "必须由隔离脚本创建真实本机 MySQL UUID 夹具；不计入离线通过数"]
async fn live_mysql_old_vs_stream_preview_benchmark() {
    let f = MysqlFixture::new().await;
    let n: u64 = std::env::var("NAVKIT_STREAM_BENCH_ROWS").unwrap().parse().unwrap();
    assert!([10_000,100_000,1_000_000,10_000_000,100_000_000].contains(&n));
    let rounds = if n <= 1_000_000 {3} else {1};
    let warm_sql = format!("SELECT id,payload FROM bench_rows WHERE id<={n} ORDER BY id LIMIT 200");
    for streaming in [false, true] {
        let record = measure(&f, &warm_sql, streaming, n, 200, 0, true).await;
        assert_eq!(record["state"], "Succeeded", "预热及200行正确性检查失败");
    }
    for control in [false, true] {
        let sql = format!("SELECT id,payload FROM bench_rows WHERE id<={n} ORDER BY id{}", if control {" LIMIT 200"} else {""});
        for round in 1..=rounds {
            let order = if round % 2 == 1 {[false,true]} else {[true,false]};
            for streaming in order {
                let record = measure(&f, &sql, streaming, n, if control {200} else {n}, round, control).await;
                if record["state"] != "Succeeded" {
                    f.cleanup().await;
                    panic!("本档已止损，失败证据已写入；不冒充成功数值");
                }
            }
        }
    }
    f.cleanup().await;
}

#[tokio::test]
async fn empty_stream_service_keeps_columns_and_no_preview() {
    let f = Fixture::new(false).await;
    let request = f.request("SELECT id,text FROM items WHERE id<0", 0);
    let registered = crate::commands::register_streaming_execution(&f.state, &request, "main", StreamOptions::default()).unwrap();
    let entry = registered.2.clone();
    let state = f.state.clone();
    crate::execution_stream::launch(entry.clone(), async move {
        crate::commands::execute_registered_sql_service(&state, request, "main", registered).await
    });
    let snapshot = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let snapshot = entry.stream().unwrap().snapshot(&entry);
            if snapshot.outcome.is_some() && !snapshot.status.resources_pending { break snapshot; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(snapshot.preview.is_none());
    let outcome = snapshot.outcome.unwrap(); assert_eq!(outcome.state, ExecutionState::Succeeded);
    let result = outcome.result.unwrap();
    assert_eq!(result.sets[0].total, 0); assert_eq!(result.sets[0].columns.len(), 2); assert!(result.sets[0].rows.is_empty());
    let id = result.spool_id.unwrap();
    let spool = crate::query_spool::authorize(&id, "c1", "tab1", "main", "main").unwrap();
    let set = spool.set(0).unwrap(); set.ensure_complete().unwrap(); assert!(set.fetch(0,200).unwrap().is_empty());
    crate::query_spool::drop_spool(&id); drop(set); drop(spool); drop(entry);
    f.cleanup().await;
}

#[tokio::test]
async fn unsupported_stream_adapter_rejects_before_registration_and_sql_send() {
    let f = Fixture::new(false).await;
    let mut configs = f.state.store.connections();
    configs[0].kind = DbKind::of("redis");
    f.state.store.save_connections(configs, vec![]).unwrap();
    let request = f.request("DELETE FROM items", 0);
    let id = request.execution_id.clone();
    let error = crate::commands::register_streaming_execution(&f.state, &request, "main", StreamOptions::default())
        .err().expect("不支持适配器必须在发送前拒绝");
    assert_eq!(error.code, "STREAM_ADAPTER_UNSUPPORTED");
    assert!(f.state.executions.entry_for_test(&id).is_none());
    assert!(!f.state.executions.has_active("c1"));
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM items").fetch_one(&f.observer).await.unwrap(), 1);
    crate::sql_history::flush();
    if let Ok(text) = std::fs::read_to_string(f.root.join("logs/sql_log.jsonl")) {
        assert!(!text.lines().any(|line| serde_json::from_str::<Value>(line).unwrap()["operation_id"] == id));
    }
    f.cleanup().await;
}
