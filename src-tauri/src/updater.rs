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

/// 更新清单地址：发布在 GitHub Releases 的资产（每次随版本在 Release 页上传 `navkit-win64.json`）。
/// 用 `latest/download` 指向最新正式版 Release，**无需每次改代码**；
/// 注意：`latest` 只认正式版（预发布 prerelease 不算）；v1.0.0 正式版发布后该地址即可直接命中，
/// 若仓库暂时只有预发布版本则该地址会 404，此时可用 `data/update/feed.json` 指向具体 tag 的清单地址。
const FEED_URL: &str =
    "https://github.com/GaoHu1997/Navkit/releases/latest/download/navkit-win64.json";

/// GitHub 加速镜像（前缀拼接式：`{加速器}{原始 GitHub/raw URL}`）。
///
/// 数组顺序即首选顺序：拉取清单/下载安装包时按顺序尝试，某个源连不上或
/// 校验失败会自动切到下一个。因此顺序只影响首选速度、不影响正确性——
/// 靠后的慢源只在前面的源都失败时才用。
///
/// 排序依据：用户本机测速（GitHub 加速选择器，ms 越小越快）。各代理的
/// 可用性会随网络与时间波动，这里保留全部作为逐级兜底；`gh-proxy.com`
/// 为通用稳定代理，实测可用，置于末尾兜底。
const GITHUB_ACCELERATORS: &[&str] = &[
    "https://git.tangbao.cc/",           // 48ms（用户图最快）
    "https://github.dpik.top/",          // 117ms
    "https://github-proxy.lixxing.top/", // 190ms
    "https://ghpxy.hwinzniej.top/",      // 268ms
    "https://kenyu.ggff.net/",           // 379ms
    "https://gh.aaa.team/",              // 509ms
    "https://hub.ddayh.com/",            // 662ms
    "https://gh.ddlc.top/",              // 773ms
    "https://y.whereisdoge.work/",       // 855ms
    "https://gh-proxy.com/",             // 新增，通用稳定代理，兜底
];

/// 把一个原始 GitHub / raw 地址展开为「直连 + 各加速器」候选列表。
/// 用于清单候选，也可用于生成清单 `mirrors`（安装包镜像）。
pub fn accelerated_urls(original: &str) -> Vec<String> {
    let mut urls = Vec::with_capacity(GITHUB_ACCELERATORS.len() + 1);
    urls.push(original.to_string());
    for acc in GITHUB_ACCELERATORS {
        urls.push(format!("{acc}{original}"));
    }
    urls
}

/// 默认清单候选：直连优先（可信），失败后逐个用加速器代理同一份清单。
/// 清单在 Release 资产上；jsDelivr 只镜像仓库文件、不镜像 Release 资产，故不用它兜底。
fn default_feed_urls() -> Vec<String> {
    accelerated_urls(FEED_URL)
}

/// 单块读取超时（视为网络停滞）。
const CHUNK_TIMEOUT_SECS: u64 = 30;
/// 单个源网络类失败的最大重试次数（用尽后切换到下一个源/镜像）。
const MAX_RETRIES: u32 = 3;
/// 清单探测的连接超时：连接不上要快速失败并切源，不能干等。
const MANIFEST_CONNECT_TIMEOUT_SECS: u64 = 8;
/// 清单探测的整体超时。
const MANIFEST_TOTAL_TIMEOUT_SECS: u64 = 15;
/// 更新包下载的连接超时。
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 10;
/// 下载源测速探测：单个源的首字节超时（超时即视为该源本次不可用，排在兜底位）。
const PROBE_TIMEOUT_MS: u64 = 4000;

// ---------------------------------------------------------------- 数据结构

/// 更新清单（feed JSON）。字段与 magpie 的 Release/Asset 对齐，做扁平化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    /// 主下载地址。
    pub url: String,
    /// 备用下载地址（镜像）。主地址网络失败时按顺序尝试，每个源都独立做完整校验。
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

impl UpdateManifest {
    /// 下载候选地址：主地址优先（GitHub 直链会自动展开为「直连 + 各加速器」），
    /// 其次清单显式声明的镜像；去重、过滤非 http(s) 与空白项。
    fn download_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = Vec::new();
        for candidate in expand_github_download(&self.url) {
            push_unique(&mut urls, &candidate);
        }
        for mirror in &self.mirrors {
            push_unique(&mut urls, mirror);
        }
        urls
    }
}

/// 追加一个 HTTP(S) 候选，去重并过滤空白/非法项。
fn push_unique(urls: &mut Vec<String>, candidate: &str) {
    let url = candidate.trim();
    if (url.starts_with("https://") || url.starts_with("http://")) && !urls.iter().any(|u| u == url) {
        urls.push(url.to_string());
    }
}

/// 若地址是 GitHub 直链（github.com / raw / release 资产域），展开为「直连 + 各加速器」；
/// 否则原样返回。加速器只负责传输，安装包仍以清单里的 SHA-256 完整校验兜底。
fn expand_github_download(url: &str) -> Vec<String> {
    let u = url.trim();
    if u.is_empty() {
        return Vec::new();
    }
    let is_github = [
        "://github.com/",
        "://raw.githubusercontent.com/",
        "://objects.githubusercontent.com/",
        "://release-assets.githubusercontent.com/",
    ]
    .iter()
    .any(|needle| u.contains(needle));
    if is_github {
        accelerated_urls(u)
    } else {
        vec![u.to_string()]
    }
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

/// 解析 feed 覆盖内容：支持单行一个 URL，或逗号/空白/换行分隔的多个 URL。
fn parse_feed_override(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        .map(str::to_string)
        .collect()
}

/// feed 地址候选，按优先级返回：
/// 1. 环境变量 `NAVKIT_UPDATE_FEED`（设置即只用它，便于强制指定单一源）；
/// 2. `data/update/feed.json`（内容可为单个或多个 URL）；
/// 3. 内置默认候选 `default_feed_urls()`（直连 + 各加速器 + jsDelivr）。
fn feed_urls() -> Vec<String> {
    if let Ok(url) = std::env::var("NAVKIT_UPDATE_FEED") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return vec![trimmed.to_string()];
        }
    }
    if let Ok(text) = std::fs::read_to_string(update_dir().join("feed.json")) {
        let urls = parse_feed_override(&text);
        if !urls.is_empty() {
            return urls;
        }
    }
    default_feed_urls()
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

/// 从环境变量取代理地址并构造 reqwest 代理。
/// 优先专用变量 `NAVKIT_UPDATE_PROXY`，其次通用 `HTTPS_PROXY` / `ALL_PROXY` / `HTTP_PROXY`
/// （大小写两种写法都认）。取不到返回 None，此时沿用 reqwest 自身的环境变量代理行为。
fn proxy_from_env() -> Option<reqwest::Proxy> {
    const KEYS: &[&str] = &[
        "NAVKIT_UPDATE_PROXY",
        "HTTPS_PROXY", "https_proxy",
        "ALL_PROXY", "all_proxy",
        "HTTP_PROXY", "http_proxy",
    ];
    for key in KEYS {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                if let Ok(proxy) = reqwest::Proxy::all(value) {
                    return Some(proxy);
                }
            }
        }
    }
    None
}

fn http_client(total_timeout_secs: u64) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(MANIFEST_CONNECT_TIMEOUT_SECS))
        .timeout(std::time::Duration::from_secs(total_timeout_secs))
        .user_agent(concat!("NavKit/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = proxy_from_env() {
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP 客户端构建失败: {e}"))
}

/// 下载用客户端：不设整体超时（大文件），连接超时较短，源不可用时尽快切镜像。
fn download_client() -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(DOWNLOAD_CONNECT_TIMEOUT_SECS))
        .user_agent(concat!("NavKit/", env!("CARGO_PKG_VERSION")));
    if let Some(proxy) = proxy_from_env() {
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP 客户端构建失败: {e}"))
}

/// 拉取并解析更新清单：逐个候选地址尝试，任一成功即返回。
async fn fetch_manifest() -> anyhow::Result<UpdateManifest> {
    let client = http_client(MANIFEST_TOTAL_TIMEOUT_SECS)?;
    let urls = feed_urls();
    let mut errors: Vec<String> = Vec::new();
    for url in &urls {
        // 每个源最多探测 2 次：连接不上就尽快换源，不对单源长重试。
        for attempt in 0..2u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            }
            match fetch_manifest_once(&client, url).await {
                Ok(m) => return Ok(m),
                Err(e) => errors.push(format!("{url}（第 {} 次）: {e:#}", attempt + 1)),
            }
        }
    }
    Err(anyhow::anyhow!(
        "拉取更新清单失败，所有候选源均不可用：{}",
        errors.join("；")
    ))
}

/// 从单个地址拉取并解析清单，校验必要字段。
async fn fetch_manifest_once(client: &reqwest::Client, url: &str) -> anyhow::Result<UpdateManifest> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("清单下载失败: {e}"))?;
    let manifest = resp
        .json::<UpdateManifest>()
        .await
        .map_err(|e| anyhow::anyhow!("清单解析失败: {e}"))?;
    if manifest.version.trim().is_empty() || manifest.url.trim().is_empty() {
        return Err(anyhow::anyhow!("清单缺少 version 或 url 字段"));
    }
    Ok(manifest)
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
pub async fn download(
    app: &tauri::AppHandle,
    expected_version: Option<&str>,
    expected_sha256: Option<&str>,
) -> anyhow::Result<PathBuf> {
    if DOWNLOAD_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Err(anyhow::anyhow!("已有下载任务进行中"));
    }
    let result = download_inner(app, expected_version, expected_sha256).await;
    DOWNLOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
    result
}

/// 并发探测各候选源的首字节延迟（发 Range 0-1023 的轻量请求），
/// 返回按延迟升序的可用源在前、探测失败的源按原序附后（仍作兜底，不丢弃）。
async fn rank_by_latency(client: &reqwest::Client, urls: &[String]) -> Vec<String> {
    let probes = urls.iter().map(|url| {
        let client = client.clone();
        let url = url.clone();
        async move {
            let started = Instant::now();
            let deadline = std::time::Duration::from_millis(PROBE_TIMEOUT_MS);
            let ok = match tokio::time::timeout(deadline, async {
                let resp = client
                    .get(&url)
                    .header(reqwest::header::RANGE, "bytes=0-1023")
                    .send()
                    .await?;
                let status = resp.status();
                // 206（分段）或 200（整段）都说明该源能提供此文件。
                Ok::<bool, reqwest::Error>(
                    status == reqwest::StatusCode::PARTIAL_CONTENT || status.is_success(),
                )
            })
            .await
            {
                Ok(Ok(v)) => v,
                _ => false, // 超时 / 连接 / 状态错误：本次不可用
            };
            (url, ok, started.elapsed().as_millis() as u64)
        }
    });
    let mut results = futures::future::join_all(probes).await;
    // 可用源排前（延迟升序）；不可用源随后（也按延迟升序，尽量把快速失败的排前面）。
    results.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    results.into_iter().map(|(url, _, _)| url).collect()
}

/// 从 URL 取 host，用于提示当前选用的下载源。
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

fn normalized_update_version(version: &str) -> &str {
    let version = version.trim();
    version.strip_prefix('v').or_else(|| version.strip_prefix('V')).unwrap_or(version)
}

fn normalized_sha256(hash: &str) -> anyhow::Result<String> {
    let hash = hash.trim();
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("SHA-256 无效：必须提供完整的 64 位十六进制哈希");
    }
    Ok(hash.to_ascii_lowercase())
}

/// 可选期望值兼容旧手动调用；新调用须绑定检查时的版本及安装包哈希。
fn verify_manifest_identity(
    manifest: &UpdateManifest,
    expected_version: Option<&str>,
    expected_sha256: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(expected) = expected_version {
        let expected = normalized_update_version(expected);
        if expected.is_empty() || expected != normalized_update_version(&manifest.version) {
            anyhow::bail!("更新版本已变化或期望版本无效，请重新检查更新");
        }
    }
    if let Some(expected) = expected_sha256 {
        let expected = normalized_sha256(expected)
            .map_err(|_| anyhow::anyhow!("期望 SHA-256 无效，请重新检查更新"))?;
        let actual = normalized_sha256(&manifest.sha256)
            .map_err(|_| anyhow::anyhow!("更新清单 SHA-256 无效，请重新检查更新"))?;
        if expected != actual {
            anyhow::bail!("更新包已变化（SHA-256 不一致），请重新检查更新");
        }
    }
    Ok(())
}

async fn download_inner(
    app: &tauri::AppHandle,
    expected_version: Option<&str>,
    expected_sha256: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let manifest = fetch_manifest().await?;
    verify_manifest_identity(&manifest, expected_version, expected_sha256)?;
    let expected_hash = manifest.sha256.trim().to_lowercase();
    let known_size = manifest.size;
    let mut urls = manifest.download_urls();
    if urls.is_empty() {
        return Err(anyhow::anyhow!("更新清单未提供有效的下载地址"));
    }

    // 下载请求不设整体超时（大文件），由单块 30s 停滞检测兜底；连接超时较短以便切源。
    let client = download_client()?;

    // 每次下载前并发探测各候选源的首字节延迟，选最快的先下（其余按延迟顺序做兜底）。
    if urls.len() > 1 {
        emit(app, "downloading", 0, known_size, 0, "正在探测各下载源速度…");
        urls = rank_by_latency(&client, &urls).await;
        if let Some(fastest) = urls.first() {
            emit(
                app,
                "downloading",
                0,
                known_size,
                0,
                &format!("已选择最快下载源：{}", host_of(fastest)),
            );
        }
    }

    emit(app, "downloading", 0, known_size, 0, "开始下载");
    let mut last_err = String::new();
    // 外层：逐个下载源（已按测速排序）。某个源失败时切到下一个，每个源独立做完整校验。
    for (origin_index, url) in urls.iter().enumerate() {
        if origin_index > 0 {
            // 切换下载源：清掉上一个源留下的半成品，绝不跨源拼接字节。
            let _ = tokio::fs::remove_file(partial_path()).await;
            emit(
                app,
                "downloading",
                0,
                known_size,
                0,
                &format!("当前下载源不可用，切换备用源（第 {} 个）", origin_index + 1),
            );
        }
        // 内层：同一源内断点续传 + 重试。
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
            match download_once(&client, url, known_size, app).await {
                Ok(done) => {
                    emit(app, "verifying", done, known_size.max(done), 0, "校验 SHA-256");
                    match verify_partial(&expected_hash, known_size).await {
                        Ok(len) => {
                            tokio::fs::rename(partial_path(), staged_path())
                                .await
                                .map_err(|e| anyhow::anyhow!("落位失败: {e}"))?;
                            emit(app, "ready", len, known_size.max(len), 0, "更新已就绪，重启后生效");
                            crate::storage::log_line(
                                "info",
                                &format!("更新包下载完成：{} 字节，版本 {}（源 {url}）", len, manifest.version),
                            );
                            return Ok(staged_path());
                        }
                        Err(e) => {
                            // 校验失败：清掉坏包并换下一个源（同源重试大概率仍是坏包）。
                            let _ = tokio::fs::remove_file(partial_path()).await;
                            last_err = format!("{e}");
                            crate::storage::log_line("error", &format!("更新包校验失败（{url}）：{e}"));
                            break;
                        }
                    }
                }
                Err(e) => {
                    // 网络失败保留 .part 供同源断点续传
                    last_err = format!("{e}");
                    crate::storage::log_line(
                        "error",
                        &format!("更新下载失败（第 {} 次，{url}）：{e}", attempt + 1),
                    );
                }
            }
        }
    }
    Err(anyhow::anyhow!("下载更新失败，已尝试所有下载源：{last_err}"))
}

/// 校验已下载的 `.part`：文件大小 + SHA-256。通过返回实际字节数；失败返回错误。
/// 是否删除 `.part` 由调用方决定（切源时要清掉旧源半成品）。
async fn verify_partial(expected_hash: &str, known_size: u64) -> anyhow::Result<u64> {
    let file_bytes = tokio::fs::read(partial_path())
        .await
        .map_err(|e| anyhow::anyhow!("读取下载文件失败: {e}"))?;
    let actual_len = file_bytes.len() as u64;
    if known_size > 0 && actual_len != known_size {
        return Err(anyhow::anyhow!("文件大小不匹配（期望 {known_size} 字节，实际 {actual_len}）"));
    }
    if !expected_hash.is_empty() {
        let mut hasher = Sha256::new();
        hasher.update(&file_bytes);
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected_hash {
            return Err(anyhow::anyhow!("SHA-256 校验不匹配，已清除损坏数据"));
        }
    }
    Ok(actual_len)
}

async fn download_once(
    client: &reqwest::Client,
    url: &str,
    known_size: u64,
    app: &tauri::AppHandle,
) -> anyhow::Result<u64> {
    let mut have: u64 = tokio::fs::metadata(partial_path())
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if known_size > 0 && have == known_size {
        // 已完整但尚未校验：直接返回交给校验环节
        return Ok(have);
    }

    let mut response = send_range_request(client, url, have).await?;
    // 416 = Range 起点越界（文件损坏）；200 = 服务器不支持续传（从头写）
    if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        have = 0;
        response = send_range_request(client, url, 0).await?;
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
    let mut total = if known_size > 0 { known_size } else { have };
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
        if known_size == 0 {
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
    url: &str,
    have: u64,
) -> anyhow::Result<reqwest::Response> {
    let mut request = client.get(url);
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

/// 安装前重新校验实际暂存文件；固定 64 KiB 缓冲，不把整个安装包读入内存。
fn verify_staged_sha256(staged: &Path, expected_sha256: &str) -> anyhow::Result<()> {
    use std::io::Read;
    let expected = normalized_sha256(expected_sha256)?;
    let mut file = std::fs::File::open(staged)
        .map_err(|e| anyhow::anyhow!("读取待安装更新包失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)
            .map_err(|e| anyhow::anyhow!("读取待安装更新包失败: {e}"))?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        anyhow::bail!("待安装更新包 SHA-256 不匹配，已取消安装，请重新检查并下载更新");
    }
    Ok(())
}

/// 换名替换并重启：运行中的 exe 改名为 `*.old` → 新版落位 → 分离启动新进程 → 退出。
/// 落位失败时把旧版改回原名（回滚），应用继续以旧版运行。
pub fn apply_and_restart(app: &tauri::AppHandle, expected_sha256: Option<&str>) -> anyhow::Result<()> {
    use tauri::Manager;
    let exe = std::env::current_exe()?;
    let staged = staged_path();
    if !staged.exists() {
        anyhow::bail!("没有已下载的更新包，请先下载更新");
    }
    if let Some(expected) = expected_sha256 {
        verify_staged_sha256(&staged, expected)?;
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
pub async fn update_download(
    app: tauri::AppHandle,
    expected_version: Option<String>,
    expected_sha256: Option<String>,
) -> CmdResult<String> {
    download(&app, expected_version.as_deref(), expected_sha256.as_deref())
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
pub fn update_apply(app: tauri::AppHandle, expected_sha256: Option<String>) -> CmdResult<()> {
    apply_and_restart(&app, expected_sha256.as_deref()).map_err(|e| AppError::new(format!("{e:#}")))
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_manifest(hash: &str) -> UpdateManifest {
        UpdateManifest {
            version: "1.2.0".into(),
            notes: String::new(),
            pub_date: String::new(),
            url: "https://example.test/navkit.exe".into(),
            mirrors: vec![],
            sha256: hash.into(),
            size: 0,
        }
    }

    #[test]
    fn 下载身份_版本变化必须重新检查() {
        let hash = "a".repeat(64);
        let manifest = identity_manifest(&hash);
        let error = verify_manifest_identity(&manifest, Some("1.1.0"), Some(&hash)).unwrap_err();
        assert!(error.to_string().contains("重新检查"));
        assert!(verify_manifest_identity(&manifest, Some("v"), Some(&hash)).is_err());
    }

    #[test]
    fn 下载身份_同版本换包必须重新检查() {
        let expected = "a".repeat(64);
        let manifest = identity_manifest(&"b".repeat(64));
        let error = verify_manifest_identity(&manifest, Some("1.2.0"), Some(&expected)).unwrap_err();
        assert!(error.to_string().contains("SHA-256 不一致"));
        assert!(error.to_string().contains("重新检查"));
    }

    #[test]
    fn 下载身份_前缀空白和哈希大小写标准化() {
        let manifest = identity_manifest(&format!(" {} ", "A".repeat(64)));
        assert!(verify_manifest_identity(&manifest, Some(" V1.2.0 "), Some(&"a".repeat(64))).is_ok());
        let legacy = identity_manifest("");
        assert!(verify_manifest_identity(&legacy, None, None).is_ok());
    }

    #[test]
    fn 下载身份_无效期望或清单哈希拒绝() {
        let valid = "a".repeat(64);
        let manifest = identity_manifest(&valid);
        for invalid in ["".to_string(), " ".to_string(), "a".repeat(63), "g".repeat(64), "é".repeat(32)] {
            assert!(verify_manifest_identity(&manifest, Some("1.2.0"), Some(&invalid)).is_err());
            assert!(verify_manifest_identity(&identity_manifest(&invalid), Some("1.2.0"), Some(&valid)).is_err());
        }
    }

    #[test]
    fn 安装前校验_流式多块读取且损坏文件拒绝() {
        let dir = std::env::temp_dir().join(format!("navkit-updater-hash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join("staged-test.bin");
        let contents = vec![0x5a; 128 * 1024 + 17];
        let expected = format!("{:x}", Sha256::digest(&contents));
        std::fs::write(&staged, &contents).unwrap();
        let valid = verify_staged_sha256(&staged, &format!(" {} ", expected.to_ascii_uppercase()));
        std::fs::write(&staged, b"corrupted").unwrap();
        let corrupted = verify_staged_sha256(&staged, &expected);
        let untouched = std::fs::read(&staged).unwrap();
        // 只清理本测试新建的 UUID 目录，不访问应用的实际暂存包或 exe。
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(valid.is_ok(), "{valid:?}");
        assert!(corrupted.unwrap_err().to_string().contains("已取消安装"));
        assert_eq!(untouched, b"corrupted");
    }

    #[test]
    fn 安装前校验_空无效哈希与缺失文件拒绝() {
        let missing = std::env::temp_dir().join(format!("navkit-updater-missing-{}", uuid::Uuid::new_v4()));
        for invalid in ["".to_string(), " ".to_string(), "a".repeat(63), "g".repeat(64)] {
            let error = verify_staged_sha256(&missing, &invalid).unwrap_err();
            assert!(error.to_string().contains("SHA-256 无效"));
        }
        assert!(verify_staged_sha256(&missing, &"a".repeat(64)).unwrap_err().to_string().contains("读取待安装"));
    }

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
    fn 清单解析_含镜像() {
        let text = r#"{"version":"1.2.0","url":"https://example.com/navkit.exe","mirrors":["https://mirror.example.com/navkit.exe"],"size":9}"#;
        let m: UpdateManifest = serde_json::from_str(text).unwrap();
        assert_eq!(m.mirrors, vec!["https://mirror.example.com/navkit.exe".to_string()]);
        assert_eq!(
            m.download_urls(),
            vec![
                "https://example.com/navkit.exe".to_string(),
                "https://mirror.example.com/navkit.exe".to_string(),
            ]
        );
    }

    #[test]
    fn 下载地址候选_主源优先且去重过滤() {
        let m = UpdateManifest {
            version: "1.0.0".into(),
            notes: String::new(),
            pub_date: String::new(),
            url: "https://main.example.com/navkit.exe".into(),
            mirrors: vec![
                "https://main.example.com/navkit.exe".into(), // 与主源重复
                "  ".into(),                                  // 空白
                "ftp://bad.example.com/x".into(),             // 非 http(s)
                "https://mirror.example.com/navkit.exe".into(),
            ],
            sha256: String::new(),
            size: 0,
        };
        assert_eq!(
            m.download_urls(),
            vec![
                "https://main.example.com/navkit.exe".to_string(),
                "https://mirror.example.com/navkit.exe".to_string(),
            ]
        );
    }

    #[test]
    fn feed覆盖解析_多地址() {
        let text = "https://a.example.com/feed.json,\nhttps://b.example.com/feed.json  not-a-url";
        assert_eq!(
            parse_feed_override(text),
            vec![
                "https://a.example.com/feed.json".to_string(),
                "https://b.example.com/feed.json".to_string(),
            ]
        );
        assert!(parse_feed_override("   ").is_empty());
    }

    #[test]
    fn 下载候选_GitHub直链自动展开加速器() {
        let m = UpdateManifest {
            version: "1.0.0".into(),
            notes: String::new(),
            pub_date: String::new(),
            url: "https://github.com/GaoHu1997/Navkit-drivers/releases/download/v1.0.1/navkit.exe".into(),
            mirrors: vec!["https://cdn.example.com/navkit.exe".into()],
            sha256: String::new(),
            size: 0,
        };
        let urls = m.download_urls();
        assert_eq!(urls[0], m.url);
        for acc in GITHUB_ACCELERATORS {
            assert!(urls.contains(&format!("{acc}{}", m.url)), "缺少加速候选");
        }
        assert_eq!(urls.last().unwrap(), "https://cdn.example.com/navkit.exe");
        assert_eq!(urls.len(), 1 + GITHUB_ACCELERATORS.len() + 1);
    }

    #[test]
    fn 下载候选_非GitHub地址不展开() {
        let m = UpdateManifest {
            version: "1.0.0".into(),
            notes: String::new(),
            pub_date: String::new(),
            url: "https://my-oss.example.com/navkit.exe".into(),
            mirrors: vec![],
            sha256: String::new(),
            size: 0,
        };
        assert_eq!(
            m.download_urls(),
            vec!["https://my-oss.example.com/navkit.exe".to_string()]
        );
    }

    #[test]
    fn 默认清单候选_直连优先并拼接加速器() {
        let urls = default_feed_urls();
        // 首项是可信直连
        assert_eq!(urls[0], FEED_URL);
        // 每个加速器都拼出一份清单候选
        for acc in GITHUB_ACCELERATORS {
            let expected = format!("{acc}{FEED_URL}");
            assert!(urls.contains(&expected), "缺少加速候选 {expected}");
        }
        // 直连 + 各加速器
        assert_eq!(urls.len(), GITHUB_ACCELERATORS.len() + 1);
        assert!(urls.iter().all(|u| u.starts_with("https://")));
        // accelerated_urls 只含直连 + 加速器
        let accelerated = accelerated_urls(FEED_URL);
        assert_eq!(accelerated[0], FEED_URL);
        assert_eq!(accelerated.len(), GITHUB_ACCELERATORS.len() + 1);
    }

    #[test]
    fn 就绪与状态_无下载时() {
        // 不实际创建 staged 文件：验证状态返回 idle/ready 相位不报错
        let s = status();
        assert!(["idle", "ready", "checking", "downloading", "verifying", "error"].contains(&s.phase.as_str()));
    }
}
