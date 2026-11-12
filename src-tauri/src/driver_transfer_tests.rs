//! Transport contracts use actual HTTP sockets, not mocked reqwest responses. Production still
//! admits only pinned HTTPS origins; these private-engine fixtures never enter the source registry.
use super::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, task::{JoinHandle, JoinSet}};

// 本组测试使用真实 HTTP socket + 毫秒级超时；并行执行时 CPU 饱和会使 tokio 定时器抖动，
// 导致「必须重试而非接受停滞响应」的断言偶发失败。统一串行执行（断言与超时值均不改变）。
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|e| e.into_inner()) }
#[derive(Clone, Copy)]
enum Fault { None, IgnoreRange, WrongRange, WrongTotal, Encoded, Html, Truncated, WrongBytes, ChangedEtag, FullLater, Retry503, RetryTwice503, SlowLater, SlowFirstHeaders, SlowFirstBody, SlowFullBody, Always503, CloseConnection }
struct State { bytes: Vec<u8>, fault: Fault, requests: AtomicUsize, first_range_requests: AtomicUsize, active: AtomicUsize, peak: AtomicUsize, later_started: tokio::sync::Notify }
struct Active(Arc<State>);
impl Drop for Active { fn drop(&mut self) { self.0.active.fetch_sub(1, Ordering::SeqCst); } }
struct Server { url: Url, state: Arc<State>, task: JoinHandle<()> }
impl Server {
    async fn start(fault: Fault) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/official.bin", listener.local_addr().unwrap())).unwrap();
        let state = Arc::new(State { bytes: (0..(CHUNK_BYTES as usize * 5 + 37)).map(|index| (index % 251) as u8).collect(), fault, requests: AtomicUsize::new(0), first_range_requests: AtomicUsize::new(0), active: AtomicUsize::new(0), peak: AtomicUsize::new(0), later_started: tokio::sync::Notify::new() });
        let shared = state.clone();
        let task = tokio::spawn(async move {
            let mut workers = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let state = shared.clone();
                        workers.spawn(async move { let _ = respond(stream, state).await; });
                    },
                    _ = workers.join_next(), if !workers.is_empty() => {},
                }
            }
        });
        Self { url, state, task }
    }
    fn digest(&self) -> String { format!("{:x}", Sha256::digest(&self.state.bytes)) }
    fn client(&self) -> Client { Client::builder().no_proxy().no_gzip().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build().unwrap() }
    async fn run(&self, path: &Path) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), transfer(&self.client(), &self.url, self.state.bytes.len() as u64, &self.digest(), path, super::super::MAX_DOWNLOAD)).await.unwrap()
    }
    async fn stop(&mut self) { self.task.abort(); let _ = (&mut self.task).await; }
}
impl Drop for Server { fn drop(&mut self) { self.task.abort(); } }
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self { let path = std::env::temp_dir().join(format!("navkit-transfer-test-{}", uuid::Uuid::new_v4())); fs::create_dir(&path).unwrap(); Self(path) }
    fn path(&self) -> PathBuf { self.0.join("official.partial") }
}
impl Drop for Directory { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

async fn respond(mut stream: TcpStream, state: Arc<State>) -> Result<()> {
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    let _active = Active(state.clone());
    let index = state.requests.fetch_add(1, Ordering::SeqCst);
    let mut request = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut byte)).await??;
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") { break }
        if request.len() > 16384 { bail!("unbounded fixture request") }
    }
    let request = String::from_utf8(request)?;
    assert!(request.starts_with("GET /official.bin HTTP/1.1\r\n"));
    assert!(request.to_ascii_lowercase().contains("accept-encoding: identity\r\n"));
    assert!(request.to_ascii_lowercase().contains("accept: application/octet-stream\r\n"));
    let ranges = request.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned)).unwrap();
    let (start, end) = ranges.split_once('-').unwrap();
    let start: usize = start.parse()?; let end: usize = end.parse()?;
    assert!(start <= end && end < state.bytes.len());
    if start == 0 { state.first_range_requests.fetch_add(1, Ordering::SeqCst); }
    if matches!(state.fault, Fault::CloseConnection) { stream.shutdown().await?; return Ok(()) }
    if matches!(state.fault, Fault::Always503) || (matches!(state.fault, Fault::Retry503) && index == 0) || (matches!(state.fault, Fault::RetryTwice503) && index < 2) {
        stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(())
    }
    if start > 0 {
        assert!(request.to_ascii_lowercase().contains("if-range: \"fixed-package\"\r\n"));
        state.later_started.notify_one();
        // Force out-of-order completion and overlap, independently of scheduler timing.
        tokio::time::sleep(Duration::from_millis(if matches!(state.fault, Fault::SlowLater) { 500 } else if start == CHUNK_BYTES as usize { 40 } else { 10 })).await;
    }
    if index == 0 && matches!(state.fault, Fault::SlowFirstHeaders) { tokio::time::sleep(Duration::from_millis(1500)).await; }
    let full = matches!(state.fault, Fault::IgnoreRange | Fault::SlowFullBody) || (start > 0 && matches!(state.fault, Fault::FullLater));
    let (status, body) = if full { ("200 OK", state.bytes.as_slice()) } else { ("206 Partial Content", &state.bytes[start..=end]) };
    let mut body = body.to_vec();
    if matches!(state.fault, Fault::WrongBytes) { body[0] ^= 0x80; }
    let mut headers = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    if !full {
        let actual_start = start + usize::from(matches!(state.fault, Fault::WrongRange));
        let total = state.bytes.len() + usize::from(matches!(state.fault, Fault::WrongTotal));
        headers.push_str(&format!("Content-Range: bytes {actual_start}-{end}/{total}\r\n"));
    }
    headers.push_str(if start > 0 && matches!(state.fault, Fault::ChangedEtag) { "ETag: \"changed-package\"\r\n" } else { "ETag: \"fixed-package\"\r\n" });
    headers.push_str(if matches!(state.fault, Fault::Html) { "Content-Type: text/html\r\n" } else { "Content-Type: application/octet-stream\r\n" });
    if matches!(state.fault, Fault::Encoded) { headers.push_str("Content-Encoding: gzip\r\n"); }
    headers.push_str("\r\n"); stream.write_all(headers.as_bytes()).await?;
    if index == 0 && matches!(state.fault, Fault::SlowFirstBody | Fault::SlowFullBody) {
        stream.write_all(&body[..32]).await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        stream.write_all(&body[32..]).await?;
        stream.shutdown().await?;
        return Ok(())
    }
    if matches!(state.fault, Fault::Truncated) { body.truncate(body.len() / 2); }
    stream.write_all(&body).await?;
    stream.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn segmented_download_is_complete_out_of_order_and_bounded() {
    let _serial = serial();
    let mut server = Server::start(Fault::None).await; let directory = Directory::new();
    server.run(&directory.path()).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), server.state.bytes);
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 6);
    let peak = server.state.peak.load(Ordering::SeqCst);
    assert!((2..=CONCURRENT_RANGES).contains(&peak), "peak requests={peak}");
    server.stop().await;
}

#[tokio::test]
async fn range_ignored_consumes_exactly_one_verified_full_response() {
    let _serial = serial();
    let mut server = Server::start(Fault::IgnoreRange).await; let directory = Directory::new();
    server.run(&directory.path()).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), server.state.bytes);
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 1);
    server.stop().await;
}

#[tokio::test]
async fn malformed_ranges_content_and_integrity_never_leave_partial_files() {
    let _serial = serial();
    for fault in [Fault::WrongRange, Fault::WrongTotal, Fault::Encoded, Fault::Html, Fault::Truncated, Fault::WrongBytes, Fault::ChangedEtag, Fault::FullLater] {
        let mut server = Server::start(fault).await; let directory = Directory::new();
        assert!(server.run(&directory.path()).await.is_err());
        assert!(!directory.path().exists(), "failed transfer kept an unverified partial");
        server.stop().await;
    }
}

#[tokio::test]
async fn retryable_http_failure_has_one_bounded_retry_and_full_digest() {
    let _serial = serial();
    let mut server = Server::start(Fault::Retry503).await; let directory = Directory::new();
    server.run(&directory.path()).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), server.state.bytes);
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 7);
    server.stop().await;
}

#[tokio::test]
async fn two_temporary_failures_require_three_bounded_attempts() {
    let _serial = serial();
    let mut server = Server::start(Fault::RetryTwice503).await; let directory = Directory::new();
    server.run(&directory.path()).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), server.state.bytes);
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 8);
    server.stop().await;
}

#[tokio::test]
async fn cancellation_closes_writers_removes_partial_and_stops_new_requests() {
    let _serial = serial();
    let mut server = Server::start(Fault::SlowLater).await; let directory = Directory::new();
    let client = server.client(); let digest = server.digest();
    let path = directory.path();
    {
        let future = transfer(&client, &server.url, server.state.bytes.len() as u64, &digest, &path, super::super::MAX_DOWNLOAD);
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("slow transfer completed before cancellation: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(3), server.state.later_started.notified()) => result.expect("fixture must receive a later range before cancellation"),
        }
        assert!(path.exists(), "test must cancel an actual in-flight partial");
        // Leaving this scope drops the live transfer and all borrowed request futures.
    }
    assert!(!path.exists());
    let requests = server.state.requests.load(Ordering::SeqCst);
    assert!(requests > 1 && requests <= 5);
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(server.state.requests.load(Ordering::SeqCst), requests, "cancelled engine issued new requests");
    server.stop().await;
}

#[tokio::test]
async fn first_range_header_and_body_stalls_retry_inside_the_card_deadline() {
    let _serial = serial();
    let timeouts = TransferTimeouts { headers: Duration::from_millis(800), idle: Duration::from_millis(800), range: Duration::from_millis(2400) };
    for fault in [Fault::SlowFirstHeaders, Fault::SlowFirstBody] {
        let mut server = Server::start(fault).await; let directory = Directory::new();
        tokio::time::timeout(Duration::from_secs(30), transfer_with_timeouts(&server.client(), &server.url, server.state.bytes.len() as u64, &server.digest(), &directory.path(), super::super::MAX_DOWNLOAD, timeouts)).await.unwrap().unwrap();
        assert_eq!(fs::read(directory.path()).unwrap(), server.state.bytes);
        assert!((2..=MAX_SEGMENT_RETRIES).contains(&server.state.first_range_requests.load(Ordering::SeqCst)), "the stalled first range must be retried, not accepted after its idle deadline");
        assert!((7..=6 * MAX_SEGMENT_RETRIES).contains(&server.state.requests.load(Ordering::SeqCst)), "each of the six ranges has a bounded retry budget even under scheduler load");
        server.stop().await;
    }
}

#[tokio::test]
async fn ignored_range_full_response_idle_timeout_cleans_up_without_duplicate_downloads() {
    let _serial = serial();
    let timeouts = TransferTimeouts { headers: Duration::from_millis(800), idle: Duration::from_millis(800), range: Duration::from_millis(2400) };
    let mut server = Server::start(Fault::SlowFullBody).await; let directory = Directory::new();
    let result = tokio::time::timeout(Duration::from_secs(10), transfer_with_timeouts(&server.client(), &server.url, server.state.bytes.len() as u64, &server.digest(), &directory.path(), super::super::MAX_DOWNLOAD, timeouts)).await.unwrap();
    let error = result.unwrap_err();
    assert!(retryable(&error));
    assert!(format!("{error:#}").contains("读取停滞超时"));
    assert!(!directory.path().exists());
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 1, "HTTP 200 fallback must never launch parallel complete responses");
    server.stop().await;
}

#[test]
fn fallback_is_only_the_exact_official_asset_not_a_generic_api_mirror() {
    let _serial = serial();
    for (filename, size, digest, asset) in [
        ("amd64", 14769106, "0319071a1d1f5eab93ae01a69d9aa7d39c772ebcf3ff4e856fc5ad654460e63e", "696564620"),
        ("arm64", 14932307, "d194bf0ce148e14c6b824c349b62486d898311bdfb7a5ba1eb554584f428f5cc", "696564653"),
    ] {
        let url = if filename == "amd64" { "https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-amd64.zip" } else { "https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-arm64.zip" };
        let spec = DownloadSpec { url, sha256: digest, size };
        let fallback = official_fallback(&spec).unwrap();
        assert_eq!(fallback.as_str(), format!("https://api.github.com/repos/duckdb/duckdb-odbc/releases/assets/{asset}"));
        assert!(trusted_url(&fallback));
        assert!(official_fallback(&DownloadSpec { size: size + 1, ..spec.clone() }).is_none());
        assert!(official_fallback(&DownloadSpec { sha256: "0000000000000000000000000000000000000000000000000000000000000000", ..spec }).is_none());
    }
    assert!(official_fallback(&DownloadSpec { url: "https://github.com/another/repository/releases/download/v2.0.0.1/asset.zip", size: 14769106, sha256: "0319071a1d1f5eab93ae01a69d9aa7d39c772ebcf3ff4e856fc5ad654460e63e" }).is_none());
}

#[tokio::test]
async fn transport_failure_uses_a_fresh_full_digest_fallback_but_integrity_failure_does_not() {
    let _serial = serial();
    let mut primary = Server::start(Fault::Always503).await;
    let mut fallback = Server::start(Fault::None).await;
    let directory = Directory::new();
    transfer_from_origins(&primary.client(), &primary.url, Some(&fallback.url), primary.state.bytes.len() as u64, &primary.digest(), &directory.path(), super::super::MAX_DOWNLOAD).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), fallback.state.bytes);
    assert_eq!(primary.state.requests.load(Ordering::SeqCst), MAX_SEGMENT_RETRIES);
    assert_eq!(fallback.state.requests.load(Ordering::SeqCst), 6);
    primary.stop().await; fallback.stop().await;

    for fault in [Fault::WrongBytes, Fault::WrongRange, Fault::Html] {
        let mut primary = Server::start(fault).await;
        let mut fallback = Server::start(Fault::None).await;
        let directory = Directory::new();
        assert!(transfer_from_origins(&primary.client(), &primary.url, Some(&fallback.url), primary.state.bytes.len() as u64, &primary.digest(), &directory.path(), super::super::MAX_DOWNLOAD).await.is_err());
        assert!(!directory.path().exists());
        assert_eq!(fallback.state.requests.load(Ordering::SeqCst), 0, "integrity or protocol failure must not be masked by a fallback");
        primary.stop().await; fallback.stop().await;
    }
}

#[tokio::test]
async fn aborted_socket_uses_verified_fallback_without_accepting_protocol_errors() {
    let _serial = serial();
    let mut primary = Server::start(Fault::CloseConnection).await;
    let mut fallback = Server::start(Fault::None).await;
    let directory = Directory::new();
    transfer_from_origins(&primary.client(), &primary.url, Some(&fallback.url), primary.state.bytes.len() as u64, &primary.digest(), &directory.path(), super::super::MAX_DOWNLOAD).await.unwrap();
    assert_eq!(fs::read(directory.path()).unwrap(), fallback.state.bytes);
    assert_eq!(primary.state.requests.load(Ordering::SeqCst), MAX_SEGMENT_RETRIES);
    assert_eq!(fallback.state.requests.load(Ordering::SeqCst), 6);
    assert!(!retryable(&anyhow::anyhow!("驱动包 SHA-256 与官方固定记录不符")));
    primary.stop().await; fallback.stop().await;
}

#[tokio::test]
async fn invalid_specs_and_existing_files_cannot_be_overwritten() {
    let _serial = serial();
    let mut server = Server::start(Fault::None).await; let directory = Directory::new();
    let path = directory.path(); fs::write(&path, b"previous verified driver").unwrap();
    assert!(server.run(&path).await.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"previous verified driver");
    assert_eq!(server.state.requests.load(Ordering::SeqCst), 0);
    assert!(validate_spec(0, &server.digest(), 100).is_err());
    assert!(validate_spec(101, &server.digest(), 100).is_err());
    assert!(validate_spec(1, "not-a-digest", 100).is_err());
    for url in ["http://github.com/package.zip", "https://github.com.evil.test/package.zip", "https://user:secret@github.com/package.zip", "https://127.0.0.1/package.zip"] {
        assert!(!trusted_url(&Url::parse(url).unwrap()));
    }
    server.stop().await;
}
