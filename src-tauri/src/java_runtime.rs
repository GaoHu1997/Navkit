//! 一键安装托管 Java 运行时（OpenJDK / Temurin 17，Windows x64）。
//!
//! NavKit 的 JDBC / H2 连接侧车依赖 **Java 11+ 的 source-file launcher**，因此需要完整
//! JDK（含 jdk.compiler），而非仅 JRE。很多用户机器没有 Java 环境；这里提供「检测 + 一键
//! 下载安装」：缺失时把 Temurin JDK 17 解压到 `data/runtime/`，之后连接优先使用它。
//!
//! 安装只写入便携数据目录（`data/runtime`），不会进入安装包 / 便携包（build.ps1 只创建
//! 空的 data 目录），符合「需下载的运行时才不打包、由用户按需安装」的约定。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::driver_manager::downloads::{self, DownloadSpec};
use crate::storage;

/// 官方 Temurin JDK 17（Windows x64 zip）。固定 URL / 大小 / SHA-256，与仓库其它驱动一致。
const JAVA_URL: &str = "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.21%2B9/OpenJDK17U-jdk_x64_windows_hotspot_17.0.21_9.zip";
const JAVA_SHA256: &str = "98518ecf554465057ee5509462631deeee374bc368c12a411d225cd29381f84b";
const JAVA_SIZE: u64 = 191_117_380;
/// 解压后顶层目录名（Adoptium 固定布局），java 在 `jdk-17.0.21+9/bin/java.exe`。
const JAVA_TOP_DIR: &str = "jdk-17.0.21+9";
/// 下载上限：包大小 182MiB，留足余量（驱动通用上限 128MiB 不够用）。
const MAX_JAVA_DOWNLOAD: u64 = 320 * 1024 * 1024;
/// 解压上限：JDK 全量解压约 300MiB。
const MAX_JAVA_EXPANDED: u64 = 1024 * 1024 * 1024;
const MAX_JAVA_ENTRIES: usize = 30_000;

/// 全局安装闸门：同一时刻只允许一个 Java 安装任务。
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// 安装锁守卫，Drop 时释放闸门。
struct InstallLock;
impl Drop for InstallLock {
    fn drop(&mut self) {
        INSTALLING.store(false, AtomicOrdering::SeqCst);
    }
}
fn try_lock_install() -> Option<InstallLock> {
    INSTALLING
        .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
        .ok()
        .map(|_| InstallLock)
}

/// 运行时状态（前端面板展示 + 决定是否显示「一键安装」按钮）。
#[derive(Debug, Clone, Serialize)]
pub struct JavaRuntimeStatus {
    /// 是否已安装托管运行时（data/runtime 下的 JDK）。
    pub installed: bool,
    /// 已识别的版本字符串（托管运行时为固定版本；系统 Java 尽力探测）。
    pub version: Option<String>,
    /// 实际使用的 java 可执行文件路径。
    pub path: Option<String>,
    /// 运行时来源：managed（托管）/ system（系统）/ missing。
    pub source: String,
    /// 人类可读说明。
    pub message: String,
    /// 是否允许一键安装托管运行时（尚未安装托管运行时时为 true）。
    pub can_install: bool,
    /// 下载大小（字节），用于前端提示。
    pub download_size: u64,
}

impl JavaRuntimeStatus {
    fn missing() -> Self {
        Self {
            installed: false,
            version: None,
            path: None,
            source: "missing".into(),
            message: "未检测到 Java 运行时。JDBC / H2 连接需要 Java 11+（完整 JDK）；可一键安装，或手动设置 NAVKIT_JAVA / JAVA_HOME / PATH。ODBC 连接使用 Windows 内置驱动管理器，无需额外安装。".into(),
            can_install: true,
            download_size: JAVA_SIZE,
        }
    }
}

/// 托管运行时的 java(.exe) 路径：`data/runtime/jdk-17.0.21+9/bin/java.exe`。
pub fn managed_java_path() -> PathBuf {
    storage::data_dir()
        .join("runtime")
        .join(JAVA_TOP_DIR)
        .join("bin")
        .join(if cfg!(windows) { "java.exe" } else { "java" })
}

/// 若托管运行时已就绪，返回其 java 路径。
pub fn managed_java() -> Option<PathBuf> {
    let path = managed_java_path();
    path.is_file().then_some(path)
}

/// 在 PATH 中查找 java（用于 System 来源探测）。
fn java_in_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 系统已有的 Java（NAVKIT_JAVA → JAVA_HOME → PATH）。
fn system_java() -> Option<PathBuf> {
    if let Some(java) = std::env::var_os("NAVKIT_JAVA") {
        let path = PathBuf::from(java);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let path = PathBuf::from(home)
            .join("bin")
            .join(if cfg!(windows) { "java.exe" } else { "java" });
        if path.is_file() {
            return Some(path);
        }
    }
    java_in_path()
}

/// 尽力读取 `java -version` 的版本号（失败时返回 None，不阻塞安装判定）。
fn probe_version(java: &Path) -> Option<String> {
    let output = crate::background_process::command(java)
        .arg("-version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stderr);
    text.split('"').nth(1).map(|s| s.to_string())
}

/// 检测当前 Java 运行时状态：优先托管运行时，其次系统 Java。
pub fn detect() -> JavaRuntimeStatus {
    if let Some(path) = managed_java() {
        return JavaRuntimeStatus {
            installed: true,
            version: Some(JAVA_TOP_DIR.replace('+', " + ")),
            path: Some(path.to_string_lossy().into_owned()),
            source: "managed".into(),
            message: format!(
                "已安装托管 Java 运行时（Temurin JDK {}），JDBC / H2 连接将优先使用它。ODBC 使用 Windows 内置驱动管理器。",
                JAVA_TOP_DIR
            ),
            can_install: false,
            download_size: JAVA_SIZE,
        };
    }
    if let Some(path) = system_java() {
        let version = probe_version(&path);
        let detail = match &version {
            Some(v) => format!("系统 Java {v}"),
            None => "系统 Java".into(),
        };
        return JavaRuntimeStatus {
            installed: false,
            version,
            path: Some(path.to_string_lossy().into_owned()),
            source: "system".into(),
            message: format!(
                "检测到{detail}（{}）。若其版本 < 11 或缺少 jdk.compiler，仍可一键安装托管运行时。",
                path.display()
            ),
            can_install: true,
            download_size: JAVA_SIZE,
        };
    }
    JavaRuntimeStatus::missing()
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn safe_entry(name: &str) -> bool {
    let path = Path::new(name);
    if path.is_absolute() {
        return false;
    }
    path.components().all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// 解压 JDK zip 到 destination（限制条目数与解压总大小，拒绝穿越路径与符号链接）。
fn extract_jdk(archive: &Path, destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive).context("下载内容不是完整有效的 ZIP")?;
    let mut zip = zip::ZipArchive::new(file).context("下载内容不是完整有效的 ZIP")?;
    if zip.is_empty() || zip.len() > MAX_JAVA_ENTRIES {
        bail!("JDK 压缩包条目数量异常");
    }
    let mut expanded = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.enclosed_name().is_none()
            || !safe_entry(if entry.is_dir() { entry.name().trim_end_matches('/') } else { entry.name() })
        {
            bail!("JDK 压缩包包含不安全路径");
        }
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            bail!("JDK 压缩包不得包含符号链接");
        }
        if entry.is_dir() {
            continue;
        }
        expanded = expanded.saturating_add(entry.size());
        if expanded > MAX_JAVA_EXPANDED {
            bail!("JDK 解压大小超过限制");
        }
        let out_path = destination.join(entry.name());
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

/// 下载并安装托管 JDK 运行时。已安装时直接返回当前状态。
pub async fn install() -> Result<JavaRuntimeStatus, String> {
    if managed_java().is_some() {
        return Ok(detect());
    }
    let lock = try_lock_install()
        .ok_or_else(|| "已有 Java 运行时正在安装，请等待完成后再试".to_string())?;

    let runtime_dir = storage::data_dir().join("runtime");
    std::fs::create_dir_all(&runtime_dir).map_err(|error| format!("创建运行时目录失败：{error}"))?;
    let staging = Staging(runtime_dir.join(format!(".download-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&staging.0).map_err(|error| format!("创建临时目录失败：{error}"))?;

    let spec = DownloadSpec { url: JAVA_URL, sha256: JAVA_SHA256, size: JAVA_SIZE };
    let archive = staging.0.join("jdk.zip.partial");
    tokio::time::timeout(
        std::time::Duration::from_secs(900),
        downloads::download_file_limited(&spec, &archive, MAX_JAVA_DOWNLOAD),
    )
    .await
    .map_err(|_| "Java 运行时下载超时（partial 已清理）".to_string())?
    .map_err(|error| format!("Java 运行时下载失败：{error:#}"))?;

    let staging_root = staging.0.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        let package = staging_root.join("package");
        std::fs::create_dir(&package).map_err(|error| format!("创建解压目录失败：{error}"))?;
        extract_jdk(&staging_root.join("jdk.zip.partial"), &package)
            .map_err(|error| format!("Java 运行时解压失败：{error:#}"))?;
        let top = locate_java_home(&package)?;
        let dest = storage::data_dir().join("runtime").join(JAVA_TOP_DIR);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|error| format!("清理旧运行时失败：{error}"))?;
        }
        match std::fs::rename(&top, &dest) {
            Ok(()) => Ok(()),
            Err(_) => {
                // 跨卷等情况下 rename 失败：递归复制。
                copy_dir(&top, &dest).map_err(|error| format!("安装运行时失败：{error:#}"))
            }
        }
    })
    .await
    .map_err(|error| format!("Java 运行时安装任务失败：{error}"))??;

    Ok(detect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_entry_rejects_traversal_and_absolute_paths() {
        assert!(safe_entry("jdk-17.0.21+9/bin/java.exe"));
        assert!(!safe_entry("../evil.bin"));
        assert!(!safe_entry("/abs/path/java"));
        assert!(!safe_entry("C:/windows/system32/x.dll"));
    }

    #[test]
    fn managed_java_lives_under_data_runtime() {
        let path = managed_java_path().to_string_lossy().replace('\\', "/");
        assert!(path.contains("/data/runtime/jdk-17.0.21+9/bin/"), "unexpected managed java path: {path}");
    }
}

/// 定位解压后的 JDK 根目录（含 bin/java）。兼容顶层目录名与常量不一致的情况。
fn locate_java_home(package: &Path) -> Result<PathBuf, String> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let preferred = package.join(JAVA_TOP_DIR);
    if preferred.join("bin").join(exe).is_file() {
        return Ok(preferred);
    }
    let mut found = None;
    for entry in std::fs::read_dir(package).map_err(|error| format!("读取解压目录失败：{error}"))? {
        let entry = entry.map_err(|error| format!("读取解压目录失败：{error}"))?;
        if entry.path().join("bin").join(exe).is_file() {
            found = Some(entry.path());
            break;
        }
    }
    found.ok_or_else(|| "JDK 解压后未找到 bin/java，包结构异常".to_string())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 前端查询 Java 运行时状态。
#[tauri::command]
pub async fn java_runtime_status() -> Result<JavaRuntimeStatus, String> {
    Ok(detect())
}

/// 前端一键安装托管 Java 运行时。
#[tauri::command]
pub async fn install_java_runtime() -> Result<JavaRuntimeStatus, String> {
    install().await
}
