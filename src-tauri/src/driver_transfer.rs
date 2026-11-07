//! Bounded official downloads. Ranges improve slow CDN transfers without weakening package checks.
use std::{fs, io::{Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}, time::Duration};
use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt};
use reqwest::{header, Client, Response, StatusCode, Url};
use sha2::{Digest, Sha256};
use super::{client, trusted_url, DownloadSpec};

const CHUNK_BYTES: u64 = 1024 * 1024;
const CONCURRENT_RANGES: usize = 4;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(180);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(40);
const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
#[derive(Clone, Copy)]
struct TransferTimeouts { headers: Duration, idle: Duration, range: Duration }
const TIMEOUTS: TransferTimeouts = TransferTimeouts { headers: REQUEST_TIMEOUT, idle: READ_IDLE_TIMEOUT, range: REQUEST_TIMEOUT };
// GitHub release assets occasionally stall or decline parallel transfers on specific CDNs.
// Each card click stays bounded; a second user click receives a fresh signed release URL.
const MAX_SEGMENT_RETRIES: usize = 3;

// Only remove a file this invocation created. Close it before deleting on Windows, including
// when the outer timeout/cancel drops an in-flight HTTP future. No detached download tasks exist.
struct PartialFile { path: PathBuf, file: Option<fs::File>, verified: bool }
impl PartialFile {
    fn create(path: &Path) -> Result<Self> {
        let file = fs::OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        Ok(Self { path: path.to_path_buf(), file: Some(file), verified: false })
    }
    fn file(&mut self) -> &mut fs::File { self.file.as_mut().expect("owned partial file") }
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.verified { let _ = fs::remove_file(&self.path); }
    }
}

fn validate_spec(size: u64, sha256: &str, limit: u64) -> Result<()> {
    if size == 0 || size > limit { bail!("驱动包大小超出固定下载限制") }
    if sha256.len() != 64 || !sha256.bytes().all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch)) {
        bail!("驱动包缺少固定 SHA-256")
    }
    Ok(())
}

fn range_header(start: u64, end: u64, total: u64, actual: Option<&str>) -> Result<()> {
    // Exact fixed boundaries/total reject overlaps, omitted data, wildcard totals and unsolicited ranges.
    let expected = format!("bytes {start}-{end}/{total}");
    if actual != Some(expected.as_str()) { bail!("驱动分段 Content-Range 与固定大小/范围不一致") }
    Ok(())
}

fn check_headers(response: &Response, expected_size: u64) -> Result<()> {
    if response.headers().get(header::CONTENT_ENCODING).is_some_and(|value| value.as_bytes() != b"identity") {
        bail!("驱动下载不得包含内容编码，不能对压缩响应拼接分段")
    }
    let content_type = response.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if content_type.contains("text/") || content_type.contains("html") || content_type.contains("json") {
        bail!("下载源返回网页/文本而非驱动包")
    }
    if response.content_length().is_some_and(|size| size != expected_size) { bail!("驱动响应大小与固定记录不一致") }
    Ok(())
}

fn retryable(error: &anyhow::Error) -> bool {
    if error.chain().any(|cause| cause.is::<tokio::time::error::Elapsed>()) { return true }
    let transport_interrupted = error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| matches!(error.kind(),
            std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset |
            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::NotConnected |
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::UnexpectedEof))
            // reqwest 0.13 can wrap these socket failures as SendRequest rather than Connect.
            // Restrict the fallback to exact transport causes; never accept TLS/validation errors.
            || matches!(cause.to_string().as_str(), "connection aborted" | "connection reset" |
                "connection reset by peer" | "connection closed before message completed")
    });
    error.chain().filter_map(|cause| cause.downcast_ref::<reqwest::Error>()).any(|cause| {
        cause.is_timeout() || cause.is_connect() || cause.is_body()
            || (cause.is_request() && transport_interrupted)
            || cause.status().is_some_and(|status| matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504))
    })
}

// Opt-in diagnostic output contains only byte ranges/statuses, never signed URLs or credentials.
fn trace(message: impl std::fmt::Display) {
    if std::env::var_os("NAVKIT_DRIVER_TRANSFER_TRACE").is_some() { eprintln!("[driver-transfer] {message}"); }
}
struct RangePiece { start: u64, bytes: Vec<u8>, url: Url, validator: Option<String> }
enum Piece { Range(RangePiece), Full(Response) }

async fn request_piece_once(client: &Client, url: &Url, start: u64, end: u64, total: u64, validator: Option<&str>, allow_full: bool, timeouts: TransferTimeouts) -> Result<Piece> {
    let started = std::time::Instant::now();
    trace(format_args!("request {} bytes={start}-{end}/{total}", url.host_str().unwrap_or("official")));
    let mut request = client.get(url.clone()).timeout(if allow_full { TOTAL_TIMEOUT } else { REQUEST_TIMEOUT })
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::ACCEPT, "application/octet-stream")
        .header(header::RANGE, format!("bytes={start}-{end}"));
    if let Some(validator) = validator { request = request.header(header::IF_RANGE, validator); }
    let mut response = tokio::time::timeout(timeouts.headers, request.send()).await
        .context("等待驱动下载响应超时")?
        .map_err(reqwest::Error::without_url)?.error_for_status().map_err(reqwest::Error::without_url)?;
    trace(format_args!("headers status={} bytes={start}-{end}/{total} elapsed={:?}", response.status(), started.elapsed()));
    if response.status() == StatusCode::OK && allow_full {
        if response.headers().contains_key(header::CONTENT_RANGE) { bail!("完整驱动响应不得伪装为分段") }
        check_headers(&response, total)?;
        return Ok(Piece::Full(response));
    }
    if response.status() != StatusCode::PARTIAL_CONTENT { bail!("驱动分段请求未返回 HTTP 206") }
    range_header(start, end, total, response.headers().get(header::CONTENT_RANGE).and_then(|value| value.to_str().ok()))?;
    let expected = end - start + 1;
    check_headers(&response, expected)?;
    let returned_validator = response.headers().get(header::ETAG).and_then(|value| value.to_str().ok())
        .filter(|value| value.starts_with('"') && value.ends_with('"')).map(str::to_owned);
    if let (Some(expected), Some(actual)) = (validator, returned_validator.as_deref()) {
        if expected != actual { bail!("驱动分段 ETag 发生变化") }
    }
    let final_url = response.url().clone();
    // A range, including the first range, cannot consume the entire card deadline. A stalled
    // response is dropped before retrying; an HTTP 200 fallback retains the full-file deadline.
    let bytes = tokio::time::timeout(timeouts.range, async {
        let mut bytes = Vec::with_capacity(expected as usize);
        while let Some(chunk) = tokio::time::timeout(timeouts.idle, response.chunk()).await
            .context("驱动分段响应读取停滞超时")?.map_err(reqwest::Error::without_url)? {
            if bytes.len() as u64 + chunk.len() as u64 > expected { bail!("驱动分段响应超过固定范围") }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() as u64 != expected { bail!("驱动分段响应被截断") }
        Ok::<_, anyhow::Error>(bytes)
    }).await.context("驱动分段响应超过单段时限")??;
    trace(format_args!("complete bytes={start}-{end}/{total} elapsed={:?}", started.elapsed()));
    Ok(Piece::Range(RangePiece { start, bytes, url: final_url, validator: returned_validator }))
}

async fn request_piece(client: &Client, url: &Url, start: u64, end: u64, total: u64, validator: Option<&str>, allow_full: bool, timeouts: TransferTimeouts) -> Result<Piece> {
    // A retry stays within the fixed 180-second outer limit. Integrity, protocol and TLS errors
    // never retry; only body/connect/timeouts and selected temporary HTTP errors qualify.
    for attempt in 0..MAX_SEGMENT_RETRIES {
        match request_piece_once(client, url, start, end, total, validator, allow_full, timeouts).await {
            Ok(piece) => return Ok(piece),
            Err(error) if attempt + 1 < MAX_SEGMENT_RETRIES && retryable(&error) => {
                trace(format_args!("retry bytes={start}-{end}/{total} attempt={} error={error:#}", attempt + 1));
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Err(error) => return Err(error).with_context(|| format!("驱动分段下载失败（{}，字节 {start}-{end}/{total}，第 {} 次尝试）", url.host_str().unwrap_or("official"), attempt + 1)),
        }
    }
    unreachable!("bounded attempts always return")
}

async fn transfer(client: &Client, url: &Url, size: u64, sha256: &str, path: &Path, limit: u64) -> Result<()> {
    transfer_with_timeouts(client, url, size, sha256, path, limit, TIMEOUTS).await
}

async fn transfer_with_timeouts(client: &Client, url: &Url, size: u64, sha256: &str, path: &Path, limit: u64, timeouts: TransferTimeouts) -> Result<()> {
    validate_spec(size, sha256, limit)?;
    let mut output = PartialFile::create(path)?;
    let end = (size.min(CHUNK_BYTES)) - 1;
    match request_piece(client, url, 0, end, size, None, true, timeouts).await? {
        Piece::Full(mut response) => {
            // Servers may ignore Range. Consume that one full response with the same size/hash limits;
            // never concatenate HTTP 200 to other parts or launch duplicate full-body requests.
            let mut received = 0u64;
            while let Some(chunk) = tokio::time::timeout(timeouts.idle, response.chunk()).await
                .context("驱动完整响应读取停滞超时")?.map_err(reqwest::Error::without_url)? {
                received = received.checked_add(chunk.len() as u64).context("驱动响应大小溢出")?;
                if received > size { bail!("驱动下载超过允许大小") }
                output.file().write_all(&chunk)?;
            }
            if received != size { bail!("驱动包不完整") }
        },
        Piece::Range(first) => {
            output.file().write_all(&first.bytes)?;
            let ranges = ((end + 1)..size).step_by(CHUNK_BYTES as usize).map(|start| (start, (start + CHUNK_BYTES - 1).min(size - 1)));
            // buffer_unordered owns its request futures. Error, timeout or IPC cancellation drops all
            // pending transfers before closing/deleting the exclusive output; no spawned writers race.
            let mut pieces = stream::iter(ranges.map(|(start, end)| {
                request_piece(client, &first.url, start, end, size, first.validator.as_deref(), false, timeouts)
            })).buffer_unordered(CONCURRENT_RANGES);
            while let Some(piece) = pieces.next().await {
                let Piece::Range(piece) = piece? else { bail!("非首段返回完整响应") };
                output.file().seek(SeekFrom::Start(piece.start))?;
                output.file().write_all(&piece.bytes)?;
            }
        },
    }
    output.file().flush()?;
    if output.file().metadata()?.len() != size { bail!("驱动包完整大小与固定记录不符") }
    output.file().seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = output.file().read(&mut buffer)?;
        if count == 0 { break }
        digest.update(&buffer[..count]);
    }
    if format!("{:x}", digest.finalize()) != sha256 { bail!("驱动包 SHA-256 与官方固定记录不符") }
    output.file().sync_all()?;
    output.verified = true;
    Ok(())
}

// These are the same immutable release assets, not a mirror or an arbitrary API URL.
// Match all three pinned fields so a changed version, size or digest cannot inherit a fallback.
fn official_fallback(spec: &DownloadSpec) -> Option<Url> {
    let asset = match (spec.url, spec.size, spec.sha256) {
        ("https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-amd64.zip", 14769106,
         "0319071a1d1f5eab93ae01a69d9aa7d39c772ebcf3ff4e856fc5ad654460e63e") => "696564620",
        ("https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-arm64.zip", 14932307,
         "d194bf0ce148e14c6b824c349b62486d898311bdfb7a5ba1eb554584f428f5cc") => "696564653",
        _ => return None,
    };
    Url::parse(&format!("https://api.github.com/repos/duckdb/duckdb-odbc/releases/assets/{asset}")).ok()
}

async fn transfer_from_origins(client: &Client, primary: &Url, fallback: Option<&Url>, size: u64, sha256: &str, path: &Path, limit: u64) -> Result<()> {
    match transfer(client, primary, size, sha256, path, limit).await {
        Ok(()) => Ok(()),
        Err(primary_error) => {
            let Some(fallback) = fallback.filter(|_| retryable(&primary_error)) else { return Err(primary_error) };
            trace(format_args!("official fallback {} after a transport failure", fallback.host_str().unwrap_or("official")));
            // transfer owns and drops its failed partial before the fallback creates a new one.
            // Never concatenate bytes from two origins. Size, ranges, ETag and digest are rechecked.
            transfer(client, fallback, size, sha256, path, limit).await
                .with_context(|| format!("官方下载入口及固定资产备用入口均失败；首入口：{primary_error:#}"))
        },
    }
}

pub(super) async fn download(spec: &DownloadSpec, path: &Path, limit: u64) -> Result<()> {
    let url = Url::parse(spec.url)?;
    if !trusted_url(&url) { bail!("不允许的驱动下载源") }
    validate_spec(spec.size, spec.sha256, limit)?;
    let fallback = official_fallback(spec);
    if fallback.as_ref().is_some_and(|url| !trusted_url(url)) { bail!("不允许的驱动备用下载源") }
    // Both origins share one card deadline, semaphore permit and exclusive staging path.
    tokio::time::timeout(TOTAL_TIMEOUT, transfer_from_origins(&client()?, &url, fallback.as_ref(), spec.size, spec.sha256, path, limit))
        .await.context("驱动完整下载超过 180 秒，未完成文件已清理")?
}

#[cfg(test)]
#[path = "driver_transfer_tests.rs"]
mod tests;
