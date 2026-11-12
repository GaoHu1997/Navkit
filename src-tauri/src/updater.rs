//! 软件更新：检查 → 下载（SHA-256 校验 + 断点续传 + 进度事件）→ 换名替换 → 重启 → 清理旧版。
//!
//! 参考 magpie 的自更新实现（不依赖任何更新框架）：
//!  · Windows 上运行中的 exe 不能被覆盖/删除，但可以被改名 —— 借此完成换名替换；
//!  · 新版本启动时清理上次留下的 `*.old` 与下载残留；
//!  · 下载始终校验整个文件（含续传保留的字节），哈希不匹配一律拒装。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, CmdResult};

/// 当前版本号（由 Cargo.toml 提供，与 tauri.conf.json / package.json 保持一致）。
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 默认更新清单地址：挂在自有镜像仓库 Navkit-drivers 上，与驱动下载同一托管域。
const DEFAULT_FEED_URL: &str =
    "https://raw.githubusercontent.com/GaoHu1997/Navkit-drivers/main/updates/navkit-win64.json";

/// 单块读取超时（视为网络停滞）。
const CHUNK_TIMEOUT_SECS: u64 = 30;
/// 网络类失败的最大重试次数。
const MAX_RETRIES: u32 = 3;

// ---------------------------------------------------------------- 数据结构

/// 更新清单（feed JSON）。字段与 magpie 的 Release/Asset 对齐，做扁平化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// 检查结果。
#[derive(Debug, Serialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub update_available: bool,
    pub manifest: Option<UpdateManifest>,
}

/// 下载进度事件载荷（`update-progress` 事件）。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateProgress {
    /// checking | downloading | verifying | ready | error
    pub phase: String,
    pub done: u64,
    pub total: u64,
    /// 字节/秒（滑动窗口）。
    pub speed: u64,
    pub message: String,
}

/// 当前更新状态（前端轮询兜底，事件丢失时用）。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub speed: u64,
    pub message: String,
    pub staged_file: Option<String>,
}

// ---------------------------------------------------------------- 路径与清理

/// ./data/update 目录（便携数据目录内，不写 AppData）。
pub fn update_dir() -> PathBuf {
    let dir = crate::storage::data_dir().join("update");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn staged_path() -> PathBuf {
    update_dir().join("navkit-new.exe")
}

fn partial_path() -> PathBuf {
    update_dir().join("navkit-new.exe.part")
}

/// feed 地址优先级：环境变量 NAVKIT_UPDATE_FEED → data/update/feed.json（内容即 URL）→ 默认清单地址。
fn feed_url() -> String {
    if let Ok(url) = std::env::var("NAVKIT_UPDATE_FEED") {
        if !url.trim().is_empty() {
            return url.trim().to_string();
        }
    }
    let override_file = update_dir().join("feed.json");
    if let Ok(text) = std::fs::read_to_string(&override_file) {
        let url = text.trim().to_string();
        if url.starts_with("http://") || url.starts_with("https://") {
            return url;
        }
    }
    DEFAULT_FEED_URL.to_string()
}

/// 启动时清理：删除上次替换留下的旧版 exe 与下载残留。删除失败（仍被占用）静默留给下次。
pub fn cleanup_old_versions() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    if let Some(dir) = exe.parent() {
        let stem = exe
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if !stem.is_empty() {
            // 主 exe 同目录的 *.old / *.old-2 / *.old-3 …
            for entry in std::fs::read_dir(dir).into_iter().flatten() {
                let Ok(entry) = entry else { continue };
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{stem}.old")) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    let _ = std::fs::remove_file(partial_path());
    if std::env::var("NAVKIT_UPDATED").as_deref().unwrap_or("") == "1" {
        crate::storage::log_line("info", "已更新到新版本并清理旧版文件");
    }
}

// ---------------------------------------------------------------- 版本比较

/// 语义化版本比较：按 `.` 拆数字段比较；带预发布（-rc/-beta）的版本小于不带预发布的同号版本。
pub fn version_is_newer(candidate: &str, current: &str) -> bool {
    let (cand_base, cand_pre) = split_prerelease(candidate);
    let (curr_base, curr_pre) = split_prerelease(current);
    let cand = parse_segments(cand_base);
    let curr = parse_segments(curr_base);
    if cand != curr {
        return cand > curr;
    }
    // 主版本相同：无预发布 > 有预发布；都有则按字典序（简化处理 rc/beta/alpha）
    match (cand_pre.is_empty(), curr_pre.is_empty()) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => cand_pre > curr_pre,
        (true, true) => false,
    }
}

fn split_prerelease(v: &str) -> (&str, &str) {
    match v.split_once('-') {
        Some((base, pre)) => (base, pre),
        None => (v, ""),
    }
}

fn parse_segments(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|s| s.trim().parse::<u64>().unwrap_or(0))
        .collect()
}

// ---------------------------------------------------------------- HTTP

fn http_client(total_timeout_secs: u64) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(total_timeout_secs))
        .user_agent(concat!("NavKit/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP 客户端构建失败: {e}"))
}

/// 拉取并解析更新清单。
async fn fetch_manifest() -> anyhow::Result<UpdateManifest> {
    let url = feed_url();
    let client = http_client(30)?;
    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1000 * (1 << attempt))).await;
        }
        match client.get(&url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => match resp.json::<UpdateManifest>().await {
                    Ok(m) => {
                        if m.version.trim().is_empty() || m.url.trim().is_empty() {
                            last_err = "清单缺少 version 或 url 字段".to_string();
                        } else {
                            return Ok(m);
                        }
                    }
                    Err(e) => last_err = format!("清单解析失败: {e}"),
                },
                Err(e) => last_err = format!("清单下载失败: {e}"),
            },
            Err(e) => last_err = format!("清单请求失败: {e}"),
        }
    }
    Err(anyhow::anyhow!("{last_err}"))
}

/// 检查更新。
pub async fn check() -> anyhow::Result<UpdateCheckResult> {
    let manifest = fetch_manifest().await?;
    let manifest = if version_is_newer(&manifest.version, CURRENT_VERSION) {
        Some(manifest)
    } else {
        None
    };
    Ok(UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        update_available: manifest.is_some(),
        manifest,
    })
}

// ---------------------------------------------------------------- 下载

/// 下载进度快照（轮询兜底）。
static LAST_PROGRESS: parking_lot::Mutex<Option<UpdateProgress>> = parking_lot::Mutex::new(None);
/// 是否已有下载在途（防止重复触发）。
static DOWNLOAD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn emit(app: &tauri::AppHandle, phase: &str, done: u64, total: u64, speed: u64, message: &str) {
    let payload = UpdateProgress {
        phase: phase.to_string(),
        done,
        total,
        speed,
        message: message.to_string(),
    };
    *LAST_PROGRESS.lock() = Some(payload.clone());
    use tauri::Emitter;
    let _ = app.emit("update-progress", &payload);
}

/// 下载更新（断点续传 + SHA-256 校验 + 停滞检测 + 指数退避重试）。
/// 完成后落位为 navkit-new.exe，等待 update_apply 换名替换。
pub async fn download(app: &tauri::AppHandle) -> anyhow::Result<PathBuf> {
    if DOWNLOAD_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Err(anyhow::anyhow!("已有下载任务进行中"));
    }
    let result = download_inner(app).await;
    DOWNLOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
    result
}

async fn download_inner(app: &tauri::AppHandle) -> anyhow::Result<PathBuf> {
    let manifest = fetch_manifest().await?;
    let expected_hash = manifest.sha256.trim().to_lowercase();
    let known_size = manifest.size;

    emit(app, "downloading", 0, known_size, 0, "开始下载");
    // 下载请求不设整体超时（大文件），由单块 30s 停滞检测兜底。
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("NavKit/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP 客户端构建失败: {e}"))?;

    let mut last_err = String::new();
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let wait = 2000u64 * (1 << attempt);
            emit(
                app,
                "downloading",
                0,
                known_size,
                0,
                &format!("下载失败，{wait} 毫秒后重试（第 {attempt}/{MAX_RETRIES} 次）"),
            );
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        match download_once(&client, &manifest, app).await {
            Ok(done) => {
                emit(app, "verifying", done, known_size.max(done), 0, "校验 SHA-256");
                let file_bytes = tokio::fs::read(partial_path())
                    .await
                    .map_err(|e| anyhow::anyhow!("读取下载文件失败: {e}"))?;
                if known_size > 0 && file_bytes.len() as u64 != known_size {
                    last_err = format!("文件大小不匹配（期望 {known_size} 字节，实际 {}）", file_bytes.len());
                    let _ = tokio::fs::remove_file(partial_path()).await;
                    continue;
                }
                let mut hasher = Sha256::new();
                hasher.update(&file_bytes);
                let actual = format!("{:x}", hasher.finalize());
                if !expected_hash.is_empty() && actual != expected_hash {
                    let _ = tokio::fs::remove_file(partial_path()).await;
                    last_err = "SHA-256 校验不匹配，已清除损坏数据".to_string();
                    continue;
                }
                tokio::fs::rename(partial_path(), staged_path())
                    .await
                    .map_err(|e| anyhow::anyhow!("落位失败: {e}"))?;
                emit(app, "ready", file_bytes.len() as u64, known_size.max(file_bytes.len() as u64), 0, "更新已就绪，重启后生效");
                crate::storage::log_line("info", &format!("更新包下载完成：{} 字节，版本 {}", file_bytes.len(), manifest.version));
                return Ok(staged_path());
            }
            Err(e) => {
                // 网络失败保留 .part 供断点续传
                last_err = format!("{e}");
                crate::storage::log_line("error", &format!("更新下载失败（第 {} 次）：{e}", attempt + 1));
            }
        }
    }
    Err(anyhow::anyhow!("下载更新失败：{last_err}"))
}

async fn download_once(
    client: &reqwest::Client,
    manifest: &UpdateManifest,
    app: &tauri::AppHandle,
) -> anyhow::Result<u64> {
    let mut have: u64 = tokio::fs::metadata(partial_path())
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if manifest.size > 0 && have == manifest.size {
        // 已完整但尚未校验：直接返回交给校验环节
        return Ok(have);
    }

    let mut response = send_range_request(client, manifest, have).await?;
    // 416 = Range 起点越界（文件损坏）；200 = 服务器不支持续传（从头写）
    if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        have = 0;
        response = send_range_request(client, manifest, 0).await?;
    }
    if response.status() == reqwest::StatusCode::OK && have > 0 {
        have = 0; // 服务器忽略 Range：整段重来
    } else if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        if let Some(range) = response.headers().get(reqwest::header::CONTENT_RANGE) {
            let range = range.to_str().unwrap_or("");
            if !range.starts_with(&format!("bytes {have}-")) {
                have = 0; // 服务器返回的起点不对：整段重来
            }
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(have == 0)
        .open(partial_path())
        .map_err(|e| anyhow::anyhow!("无法写入下载文件: {e}"))?;
    if have > 0 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(have))?;
    }

    let mut done: u64 = have;
    let mut meter = SpeedMeter::new();
    let mut total = if manifest.size > 0 { manifest.size } else { have };
    use std::io::Write as _;

    loop {
        let next = match tokio::time::timeout(
            std::time::Duration::from_secs(CHUNK_TIMEOUT_SECS),
            response.chunk(),
        )
        .await
        {
            // 超时：网络停滞
            Err(_) => return Err(anyhow::anyhow!("下载停滞（超过 {CHUNK_TIMEOUT_SECS} 秒没有收到数据）")),
            Ok(inner) => match inner {
                Err(e) => return Err(anyhow::anyhow!("读取数据失败: {e}")),
                Ok(Some(chunk)) => chunk,
                Ok(None) => break, // 流结束
            },
        };
        file.write_all(&next)?;
        done += next.len() as u64;
        if manifest.size == 0 {
            total = done;
        }
        let speed = meter.sample(next.len() as u64);
        emit(app, "downloading", done, total, speed, "正在下载");
    }
    file.flush()?;
    Ok(done)
}

async fn send_range_request(
    client: &reqwest::Client,
    manifest: &UpdateManifest,
    have: u64,
) -> anyhow::Result<reqwest::Response> {
    let mut request = client.get(&manifest.url);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("请求失败: {e}"))?;
    response
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("服务器错误: {e}"))
}

/// 5 秒滑动窗口速度计。
struct SpeedMeter {
    window_start: Instant,
    window_bytes: u64,
    last_speed: u64,
}

impl SpeedMeter {
    fn new() -> Self {
        SpeedMeter { window_start: Instant::now(), window_bytes: 0, last_speed: 0 }
    }
    fn sample(&mut self, bytes: u64) -> u64 {
        self.window_bytes += bytes;
        let elapsed = self.window_start.elapsed().as_secs_f64();
        if elapsed >= 5.0 {
            self.last_speed = (self.window_bytes as f64 / elapsed) as u64;
            self.window_bytes = 0;
            self.window_start = Instant::now();
        } else if self.last_speed == 0 && self.window_bytes > 0 && elapsed > 0.5 {
            self.last_speed = (self.window_bytes as f64 / elapsed) as u64;
        }
        self.last_speed
    }
}

// ---------------------------------------------------------------- 安装（换名替换 + 重启）

/// 就绪检查：navkit-new.exe 已存在则视为可安装。
pub fn is_ready() -> bool {
    staged_path().exists()
}

/// 当前状态（前端轮询兜底）。
pub fn status() -> UpdateStatus {
    let ready = is_ready();
    let snap = LAST_PROGRESS.lock().clone().unwrap_or(UpdateProgress {
        phase: if ready { "ready".into() } else { "idle".into() },
        done: 0,
        total: 0,
        speed: 0,
        message: String::new(),
    });
    let phase = if ready { "ready".to_string() } else { snap.phase };
    UpdateStatus {
        phase,
        done: snap.done,
        total: snap.total,
        speed: snap.speed,
        message: snap.message,
        staged_file: if ready {
            Some(staged_path().to_string_lossy().to_string())
        } else {
            None
        },
    }
}

/// 换名替换并重启：运行中的 exe 改名为 `*.old` → 新版落位 → 分离启动新进程 → 退出。
/// 落位失败时把旧版改回原名（回滚），应用继续以旧版运行。
pub fn apply_and_restart(app: &tauri::AppHandle) -> anyhow::Result<()> {
    use tauri::Manager;
    let exe = std::env::current_exe()?;
    let staged = staged_path();
    if !staged.exists() {
        anyhow::bail!("没有已下载的更新包，请先下载更新");
    }
    // 旧版落点：exe.old（已存在则 .old-2 递增，避免覆盖更早的回滚点）
    let old = aside_path(&exe);
    rename_with_retry(&exe, &old)
        .map_err(|e| anyhow::anyhow!("旧版程序改名失败（可能被杀毒软件占用）: {e}"))?;
    if let Err(e) = std::fs::rename(&staged, &exe) {
        // 回滚：旧版放回原位
        let _ = std::fs::rename(&old, &exe);
        return Err(anyhow::anyhow!("新版程序落位失败，已回滚: {e}"));
    }
    crate::storage::log_line("info", "更新落位完成，重启进入新版本");

    // 分离启动新进程，带更新标记（新版本启动时打日志并清理旧版）
    spawn_detached(&exe)?;

    // 优雅退出
    app.exit(0);
    Ok(())
}

/// `navkit.exe` → `navkit.exe.old`（占用时 `.old-2`、`.old-3` 递增）。
fn aside_path(exe: &Path) -> PathBuf {
    let mut path = exe.with_extension("exe.old");
    let mut n = 2;
    while path.exists() {
        if n > 9 {
            path = exe.with_extension(format!("exe.old-{}", uuid::Uuid::new_v4().simple()));
            break;
        }
        path = exe.with_extension(format!("exe.old-{n}"));
        n += 1;
    }
    path
}

/// 改名重试（杀毒软件短暂占用文件时退避重试）。
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let waits = [100, 200, 400, 800, 1500];
    for ms in waits {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(ms)),
        }
    }
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn spawn_detached(exe: &Path) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(exe)
        .env("NAVKIT_UPDATED", "1")
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| anyhow::anyhow!("启动新版本失败: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached(exe: &Path) -> anyhow::Result<()> {
    std::process::Command::new(exe)
        .env("NAVKIT_UPDATED", "1")
        .spawn()
        .map_err(|e| anyhow::anyhow!("启动新版本失败: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------- Tauri 命令

/// 检查更新。
#[tauri::command]
pub async fn update_check() -> CmdResult<UpdateCheckResult> {
    check().await.map_err(|e| AppError::new(format!("{e:#}")))
}

/// 下载更新（进度通过 `update-progress` 事件推送；前端亦可 update_status 轮询）。
#[tauri::command]
pub async fn update_download(app: tauri::AppHandle) -> CmdResult<String> {
    download(&app)
        .await
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| AppError::new(format!("{e:#}")))
}

/// 当前更新状态。
#[tauri::command]
pub fn update_status() -> UpdateStatus {
    status()
}

/// 安装更新：换名替换 + 重启为新版本。
#[tauri::command]
pub fn update_apply(app: tauri::AppHandle) -> CmdResult<()> {
    apply_and_restart(&app).map_err(|e| AppError::new(format!("{e:#}")))
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 版本比较_主版本号() {
        assert!(version_is_newer("1.1.0", "1.0.0"));
        assert!(version_is_newer("2.0.0", "1.9.9"));
        assert!(!version_is_newer("1.0.0", "1.1.0"));
        assert!(!version_is_newer("1.0.0", "1.0.0"));
        assert!(version_is_newer("1.0.1", "1.0.0"));
        assert!(version_is_newer("1.10.0", "1.9.0"));
    }

    #[test]
    fn 版本比较_预发布() {
        assert!(version_is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!version_is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(version_is_newer("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(!version_is_newer("1.0.0-beta", "1.0.0-rc.1"));
    }

    #[test]
    fn 版本比较_非数字段兜底() {
        assert!(version_is_newer("1.2.3", "1.2.x"));
        assert!(!version_is_newer("garbage", "1.0.0"));
    }

    #[test]
    fn 换名落点递增() {
        let dir = std::env::temp_dir().join(format!("navkit-updater-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("navkit.exe");
        std::fs::write(&exe, b"x").unwrap();
        let aside1 = aside_path(&exe);
        assert!(aside1.to_string_lossy().ends_with("exe.old"));
        std::fs::write(&aside1, b"x").unwrap();
        let aside2 = aside_path(&exe);
        assert!(aside2.to_string_lossy().ends_with("exe.old-2"), "{aside2:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 清单解析_完整字段() {
        let text = r#"{"version":"1.1.0","notes":"修复若干问题","pub_date":"2026-11-11","url":"https://example.com/navkit.exe","sha256":"abc","size":123}"#;
        let m: UpdateManifest = serde_json::from_str(text).unwrap();
        assert_eq!(m.version, "1.1.0");
        assert_eq!(m.size, 123);
    }

    #[test]
    fn 清单解析_缺省字段() {
        let text = r#"{"version":"1.1.0","url":"https://example.com/navkit.exe"}"#;
        let m: UpdateManifest = serde_json::from_str(text).unwrap();
        assert_eq!(m.notes, "");
        assert_eq!(m.sha256, "");
        assert_eq!(m.size, 0);
    }

    #[test]
    fn 就绪与状态_无下载时() {
        // 不实际创建 staged 文件：验证状态返回 idle/ready 相位不报错
        let s = status();
        assert!(["idle", "ready", "checking", "downloading", "verifying", "error"].contains(&s.phase.as_str()));
    }
}
