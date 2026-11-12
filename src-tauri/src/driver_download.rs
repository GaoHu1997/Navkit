//! Verified, portable driver acquisition. No installer is ever executed.
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}, sync::OnceLock, time::Duration};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::{catalog, models::Protocol, storage};
use super::{escape_value, validate_driver_file};
#[path = "driver_artifacts.rs"]
mod artifacts;
#[path = "managed_jdbc.rs"]
pub mod managed_jdbc;
#[path = "driver_transfer.rs"]
mod transfer;
#[path = "driver_publish.rs"]
mod publication;

const MAX_DOWNLOAD: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED: u64 = 384 * 1024 * 1024;
const ORACLE_ASSET_LIMIT: u64 = 192 * 1024 * 1024;
const ORACLE_TOTAL_DOWNLOAD: u64 = 256 * 1024 * 1024;

// Modern 23.26 assets have direct anonymous links (no license-lightbox/Accept step).
// https://www.oracle.com/database/technologies/instant-client/winx64-64-downloads.html
// Both original ZIPs retain BASIC_LICENSE/ODBC_LICENSE and all proprietary notices.
// Their embedded license is Oracle Free Distribution, Hosting, and Use Terms v1.0,
// 28 June 2022: unmodified distribution/internal business use is allowed subject to
// keeping the license/notices, no additional driver fees, and export compliance.
// No cookie, login, installer execution, or automatic license acceptance is used.
fn oracle_specs() -> Option<[DownloadSpec; 2]> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) { return None }
    Some([
        DownloadSpec { url: "https://download.oracle.com/otn_software/nt/instantclient/2326400/instantclient-basic-windows.x64-23.26.4.0.0.zip", sha256: "09e9b81cd492f5ce6f671317c23a0e1f5dee5e786258149d216321c3dad3023f", size: 144192755 },
        DownloadSpec { url: "https://download.oracle.com/otn_software/nt/instantclient/2326400/instantclient-odbc-windows.x64-23.26.4.0.0.zip", sha256: "621ed172606c2c1e6af9b5d13f0567e536d0960ea47aadbef8208a0e86d81548", size: 1065630 },
    ])
}
static DOWNLOAD_GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

/// 会话级驱动状态缓存：「新建连接」每次打开都会拉全量状态，此前每次都重新哈希
/// 全部托管包（最大 98MB）并重扫注册表。指纹（managed-drivers 文件树的相对路径+
/// 大小+mtime、ODBC 清单及其库文件、相关环境变量）不变时直接复用上次结果，配合
/// probe_cache 做到零哈希、零探针。任何下载/删除/替换都会改变指纹并自动重算，
/// 不存在手工失效，也不存在过期误报：指纹代表决定状态的全部输入。
static STATUS_CACHE: OnceLock<parking_lot::Mutex<Option<(u64, Vec<DriverStatus>)>>> = OnceLock::new();

fn cached_driver_statuses(fingerprint: u64) -> Option<Vec<DriverStatus>> {
    STATUS_CACHE.get_or_init(|| parking_lot::Mutex::new(None)).lock().clone()
        .filter(|(cached, _)| *cached == fingerprint).map(|(_, statuses)| statuses)
}
fn store_driver_statuses(fingerprint: u64, statuses: &[DriverStatus]) {
    *STATUS_CACHE.get_or_init(|| parking_lot::Mutex::new(None)).lock() = Some((fingerprint, statuses.to_vec()));
}

fn file_fingerprint(hasher: &mut Sha256, path: &Path) {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta.modified().ok()
                .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|since| (since.as_secs(), since.subsec_nanos()))
                .unwrap_or((0, 0));
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(&meta.len().to_le_bytes());
            hasher.update(&mtime.0.to_le_bytes());
            hasher.update(&mtime.1.to_le_bytes());
        }
        Err(_) => hasher.update(b"absent"),
    }
    hasher.update([0]);
}

fn walk_tree_fingerprint(hasher: &mut Sha256, dir: &Path, prefix: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        hasher.update(prefix.as_bytes()); hasher.update(b"absent\0"); return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}{name}");
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            hasher.update(relative.as_bytes()); hasher.update(b"/\0");
            walk_tree_fingerprint(hasher, &entry.path(), &format!("{relative}/"));
        } else {
            file_fingerprint(hasher, &entry.path());
        }
    }
    hasher.update(prefix.as_bytes()); hasher.update(b"end\0");
}

fn status_fingerprint() -> u64 {
    status_fingerprint_at(&storage::data_dir().join("managed-drivers"))
}

fn status_fingerprint_at(managed_root: &Path) -> u64 {
    let mut hasher = Sha256::new();
    // 1) 全部托管驱动文件：JDBC 包、依赖 JAR、Oracle 客户端、H2 jar、artifacts 下载件。
    walk_tree_fingerprint(&mut hasher, managed_root, "");
    // 2) ODBC 清单：驱动名/库路径与其库文件 size+mtime（安装/卸载/替换都会变化）。
    match super::list_odbc_inventory() {
        Ok(inventory) => {
            hasher.update(b"inventory\0");
            for driver in &inventory.drivers {
                hasher.update(driver.name.as_bytes()); hasher.update([0]);
                hasher.update(driver.library.as_bytes()); hasher.update([0]);
                file_fingerprint(&mut hasher, &driver_path(&driver.library).unwrap_or_default());
            }
        }
        Err(_) => hasher.update(b"inventory-error\0"),
    }
    // 3) 影响状态的环境变量与平台常量；NAVKIT_H2_JAR 指向的文件在托管树之外，单独入指纹。
    for key in ["NAVKIT_H2_JAR", "NAVKIT_JAVA", "JAVA_HOME"] {
        hasher.update(key.as_bytes()); hasher.update([0]);
        let value = std::env::var_os(key).map(|v| v.to_string_lossy().into_owned()).unwrap_or_default();
        hasher.update(value.as_bytes()); hasher.update([0]);
        if key == "NAVKIT_H2_JAR" && !value.is_empty() {
            file_fingerprint(&mut hasher, Path::new(value.trim().trim_matches('"')));
        }
    }
    hasher.update(std::env::consts::ARCH.as_bytes());
    u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap())
}

#[derive(Debug, Clone, Serialize)]
pub struct DriverStatus {
    pub kind: String,
    /// 托管驱动版本；空串表示该状态行没有版本维度（内置协议 / ODBC 安装检测）。
    #[serde(default)]
    pub version: String,
    pub status: String,
    pub architecture: String,
    pub message: String,
    pub can_download: bool,
    pub library: Option<String>,
    pub download_path: Option<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct DownloadSpec { pub(crate) url: &'static str, pub(crate) sha256: &'static str, pub(crate) size: u64 }

// Pinned official assets. Digests/sizes are published by the official GitHub Releases API:
// https://api.github.com/repos/duckdb/duckdb-odbc/releases/tags/v2.0.0.1
fn duckdb_spec() -> Option<DownloadSpec> {
    if !cfg!(target_os = "windows") { return None }
    match std::env::consts::ARCH {
        "x86_64" => Some(DownloadSpec {
            url: "https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-amd64.zip",
            sha256: "0319071a1d1f5eab93ae01a69d9aa7d39c772ebcf3ff4e856fc5ad654460e63e", size: 14769106,
        }),
        "aarch64" => Some(DownloadSpec {
            url: "https://github.com/duckdb/duckdb-odbc/releases/download/v2.0.0.1/duckdb_odbc-windows-arm64.zip",
            sha256: "d194bf0ce148e14c6b824c349b62486d898311bdfb7a5ba1eb554584f428f5cc", size: 14932307,
        }),
        _ => None,
    }
}
fn root(kind: &str) -> PathBuf { storage::data_dir().join("managed-drivers").join(kind).join(std::env::consts::ARCH) }
/// Cross-platform default shared with the JDBC connection adapter.
pub fn managed_h2_jar() -> PathBuf { storage::data_dir().join("managed-drivers").join("h2").join("h2.jar") }
/// H2 推荐版本（新下载写入版本化目录；旧布局 managed-drivers/h2/h2.jar 继续被识别）。
pub const H2_RECOMMENDED: &str = "2.3.232";
/// 已核验的 H2 官方版本（推荐版本在最前）；新增版本必须有官方 URL/大小/SHA-256。
pub const H2_VERSIONS: &[&str] = &["2.3.232", "2.2.224"];
/// 版本化 JAR 路径：data/managed-drivers/h2/<version>/h2.jar。
pub fn managed_h2_jar_for(version: &str) -> PathBuf {
    storage::data_dir().join("managed-drivers").join("h2").join(version).join("h2.jar")
}
fn h2_spec_for(version: &str) -> Option<DownloadSpec> {
    match version {
        "2.3.232" => Some(DownloadSpec { url: "https://repo.maven.apache.org/maven2/com/h2database/h2/2.3.232/h2-2.3.232.jar", sha256: "8dae62d22db8982c3dcb3826edb9c727c5d302063a67eef7d63d82de401f07d3", size: 2651157 }),
        "2.2.224" => Some(DownloadSpec { url: "https://repo.maven.apache.org/maven2/com/h2database/h2/2.2.224/h2-2.2.224.jar", sha256: "b9d8f19358ada82a4f6eb5b174c6cfe320a375b5a9cb5a4fe456d623e6e55497", size: 2614933 }),
        _ => None,
    }
}
fn h2_spec() -> DownloadSpec { h2_spec_for(H2_RECOMMENDED).expect("H2 推荐版本必须有固定校验值") }
/// 解析某版本的实际 JAR：版本化目录 → 旧布局（仅推荐版本）→ 未下载报错。
pub fn resolve_h2_jar(version: &str) -> Result<PathBuf> {
    if !H2_VERSIONS.contains(&version) {
        bail!("没有已核验的 H2 官方版本 {version}；可用版本：{}", H2_VERSIONS.join("、"))
    }
    let versioned = managed_h2_jar_for(version);
    if versioned.exists() { return Ok(versioned) }
    if version == H2_RECOMMENDED && managed_h2_jar().exists() { return Ok(managed_h2_jar()) }
    bail!("H2 官方驱动版本 {version} 未下载，请先在「驱动管理 → 官方驱动」下载后再连接")
}
fn verify_h2_jar_for(path: &Path, version: &str) -> Result<()> {
    let spec = h2_spec_for(version).ok_or_else(|| anyhow!("H2 版本 {version} 没有固定校验值"))?;
    if fs::metadata(path)?.len() != spec.size || digest_file(path)? != spec.sha256 { bail!("H2 JAR 大小或官方 SHA-256 不符") }
    verify_h2_structure(path)
}
fn verify_h2_jar(path: &Path) -> Result<()> {
    verify_h2_jar_for(path, H2_RECOMMENDED)
}
fn verify_h2_structure(path: &Path) -> Result<()> {
    let size = fs::metadata(path)?.len();
    if !(100_000..=64 * 1024 * 1024).contains(&size) { bail!("H2 JAR 文件大小异常") }
    let mut jar = zip::ZipArchive::new(fs::File::open(path)?).context("H2 文件不是完整的 JAR/ZIP")?;
    if jar.len() == 0 || jar.len() > 4096 { bail!("H2 JAR 条目数量异常") }
    let mut expanded = 0u64; let mut names = std::collections::HashSet::new();
    let mut driver_class = None; let mut manifest = None;
    for index in 0..jar.len() {
        let mut entry = jar.by_index(index)?;
        let name = entry.name().to_owned();
        if !safe_relative(if entry.is_dir() { name.trim_end_matches('/') } else { &name }) || !names.insert(name.clone()) {
            bail!("H2 JAR 包含不安全或重复路径")
        }
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) { bail!("H2 JAR 不得包含符号链接") }
        if entry.is_dir() { continue }
        expanded = expanded.saturating_add(entry.size());
        if expanded > MAX_EXPANDED { bail!("H2 JAR 解压大小异常") }
        let advertised = entry.size();
        if name == "org/h2/Driver.class" || name == "META-INF/MANIFEST.MF" {
            if advertised > 1024 * 1024 { bail!("H2 class/manifest 大小异常") }
            let mut bytes = Vec::new(); entry.by_ref().take(advertised + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != advertised { bail!("H2 class/manifest 被截断") }
            if name == "org/h2/Driver.class" { driver_class = Some(bytes) } else { manifest = Some(bytes) }
        } else {
            let copied = std::io::copy(&mut entry.by_ref().take(advertised + 1), &mut std::io::sink())?;
            if copied != advertised { bail!("H2 JAR 条目被截断：{name}") }
        }
    }
    let class = driver_class.ok_or_else(|| anyhow!("H2 JAR 缺少 org/h2/Driver.class"))?;
    if class.len() < 64 || !class.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
        || !(52..=70).contains(&u16::from_be_bytes([class[6], class[7]]))
        || u16::from_be_bytes([class[8], class[9]]) < 2 || !class.windows(13).any(|value| value == b"org/h2/Driver") {
        bail!("H2 Driver.class 结构不是合理的 Java class")
    }
    let manifest = String::from_utf8(manifest.ok_or_else(|| anyhow!("H2 JAR 缺少版本清单"))?).context("H2 JAR 版本清单不是 UTF-8")?;
    if !manifest.lines().any(|line| line.trim().starts_with("Implementation-Version: 2.") || line.trim().starts_with("Bundle-Version: 2.")) {
        bail!("当前 JDBC 适配器要求 H2 2.x 驱动；JAR 版本清单无有效 2.x 版本")
    }
    Ok(())
}
fn h2_status_for(path: &Path, explicit: bool, version: &str) -> DriverStatus {
    let source = if explicit { "NAVKIT_H2_JAR 指定的自备 H2 2.x JAR".to_string() } else { format!("H2 官方托管 JAR {version}") };
    let verification = if explicit { verify_h2_structure(path) } else { verify_h2_jar_for(path, version) };
    if let Err(error) = verification {
        let message = if explicit { format!("{source} 校验失败：{error:#}。下载只保存默认托管 JAR，不会覆盖环境变量；请先修正或移除 NAVKIT_H2_JAR。") }
            else { format!("H2 JAR 缺失或校验失败：{error:#}；可直接下载官方 {version} 包，另需 Java 11+ source launcher。下载不安装 Java。") };
        return base_status("h2", if explicit { "manual" } else { "missing" }, message, true)
    }
    match crate::h2_db::runtime_status(path) {
        Ok(probe) => { let mut status = base_status("h2", "ready", format!("{source} 的全部 ZIP CRC、Driver.class/版本与 Java/JDBC 运行时验证通过：{probe}。数据库连接仍需测试。"), false); status.library = Some(path.to_string_lossy().into_owned()); status },
        Err(error) => { let mut status = base_status("h2", "manual", format!("{source} 完整，但 Java/JDBC 运行时不可用：{error:#}。请配置 NAVKIT_JAVA、JAVA_HOME 或 PATH 中的 Java 11+，包含 jdk.compiler/source launcher。"), false); status.library = Some(path.to_string_lossy().into_owned()); status },
    }
}
/// 按版本展开的 H2 状态（推荐版本在最前）；NAVKIT_H2_JAR 环境变量覆盖时无版本维度。
fn h2_statuses() -> Vec<DriverStatus> {
    match std::env::var_os("NAVKIT_H2_JAR") {
        Some(path) => vec![h2_status_for(&PathBuf::from(path), true, H2_RECOMMENDED)],
        None => H2_VERSIONS.iter().map(|version| {
            let versioned = managed_h2_jar_for(version);
            let path = if versioned.exists() { versioned }
                else if *version == H2_RECOMMENDED && managed_h2_jar().exists() { managed_h2_jar() }
                else { versioned };
            let mut status = h2_status_for(&path, false, version);
            status.version = (*version).into();
            status
        }).collect(),
    }
}
#[derive(Serialize, Deserialize)]
struct Manifest { version: u32, kind: String, architecture: String, library: String, files: Vec<ManifestFile> }
#[derive(Serialize, Deserialize)]
struct ManifestFile { name: String, size: u64, sha256: String }
fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new(); let mut buffer = [0u8; 65536];
    loop { let n = file.read(&mut buffer)?; if n == 0 { break } digest.update(&buffer[..n]); }
    Ok(format!("{:x}", digest.finalize()))
}
fn safe_relative(name: &str) -> bool {
    !name.is_empty() && !name.contains('\\') && !name.contains(':') && !Path::new(name).is_absolute()
        && name.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && Path::new(name).components().all(|part| matches!(part, std::path::Component::Normal(_)))
}
fn verify_manifest_files(directory: &Path, kind: &str) -> Result<PathBuf> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?).context("托管驱动清单损坏")?;
    if manifest.version != 1 || manifest.kind != kind || manifest.architecture != std::env::consts::ARCH
        || !safe_relative(&manifest.library) || manifest.files.is_empty() || manifest.files.len() > if kind == "oracle" { 128 } else { 64 } {
        bail!("托管驱动清单或架构无效")
    }
    if !manifest.files.iter().any(|entry| entry.name == manifest.library) { bail!("清单没有包含驱动主库") }
    for entry in &manifest.files {
        if !safe_relative(&entry.name) { bail!("托管驱动清单包含不安全路径") }
        let path = directory.join(&entry.name);
        if !path.is_file() || fs::metadata(&path)?.len() != entry.size || digest_file(&path)? != entry.sha256 {
            bail!("驱动库不完整或已损坏：{}", entry.name)
        }
    }
    if kind == "oracle" {
        let specs = oracle_specs().ok_or_else(|| anyhow!("当前架构没有已核验的 Oracle 包"))?;
        let mut expanded = 0u64; let mut entries = 0usize; let mut expected_files = 2usize;
        for ((label, name), spec) in [("basic", "basic.zip"), ("odbc", "odbc.zip")].iter().zip(specs) {
            let path = directory.join("archives").join(name);
            if fs::metadata(&path)?.len() != spec.size || digest_file(&path)? != spec.sha256 { bail!("Oracle 官方原始资产损坏") }
            let mut zip = zip::ZipArchive::new(fs::File::open(path)?)?;
            entries = entries.checked_add(zip.len()).ok_or_else(|| anyhow!("Oracle 条目数量溢出"))?;
            if entries > 128 { bail!("Oracle 双资产条目超过限制") }
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index)?;
                if entry.is_dir() { continue }
                expanded = expanded.checked_add(entry.size()).ok_or_else(|| anyhow!("Oracle 解压大小溢出"))?;
                if expanded > ORACLE_EXPANDED_LIMIT { bail!("Oracle 固定双资产展开超过限制") }
                let name = oracle_entry_path(entry.name(), label)?;
                let item = manifest.files.iter().find(|item| item.name == name).ok_or_else(|| anyhow!("Oracle 清单遗漏官方组件：{name}"))?;
                if item.size != entry.size() { bail!("Oracle 清单文件大小与官方资产不符：{name}") }
                let mut digest = Sha256::new(); let mut bytes = [0u8; 65536]; let mut size = 0u64;
                loop { let n = entry.read(&mut bytes)?; if n == 0 { break } size = size.saturating_add(n as u64); if size > item.size { bail!("Oracle ZIP 实际大小不符") } digest.update(&bytes[..n]); }
                if size != item.size || format!("{:x}", digest.finalize()) != item.sha256 { bail!("Oracle 展开文件不是固定官方资产内容：{name}") }
                expected_files += 1;
            }
        }
        if manifest.files.len() != expected_files { bail!("Oracle 清单重复或附加未知文件") }
        if manifest.library != "client/sqora32.dll" { bail!("Oracle 主库清单无效") }
        for name in ["client/oci.dll", "client/oraociei.dll", "client/BASIC_LICENSE", "client/ODBC_LICENSE", "client/BASIC_README", "client/ODBC_README"] {
            if !manifest.files.iter().any(|entry| entry.name == name) { bail!("Oracle Basic / ODBC 组件或许可不完整：{name}") }
        }
        // The pinned official Basic asset deliberately includes three 11-byte provider placeholders.
        // Their original ZIP SHA256/CRC and exact extracted contents were verified above; never load them.
        for entry in manifest.files.iter().filter(|entry| entry.name.to_ascii_lowercase().ends_with(".dll")) {
            let path = directory.join(&entry.name);
            if ["client/extks.dll", "client/legacy.dll", "client/pkcs11.dll"].contains(&entry.name.as_str()) {
                if !oracle_provider_placeholder(&entry.name, &fs::read(&path)?) { bail!("Oracle 固定官方占位组件已被修改：{}", entry.name) }
                continue;
            }
            super::validate_pe(&path, 0x8664).with_context(|| format!("Oracle 本机组件校验失败：{}", entry.name))?;
        }
    }
    Ok(directory.join(&manifest.library))
}
fn oracle_provider_placeholder(name: &str, bytes: &[u8]) -> bool {
    ["client/extks.dll", "client/legacy.dll", "client/pkcs11.dll"].contains(&name) && bytes == b"Dummy File!"
}
fn verify_manifest(directory: &Path, kind: &str) -> Result<PathBuf> {
    let library = verify_manifest_files(directory, kind)?;
    validate_driver_file(&library.to_string_lossy())?;
    Ok(library)
}
pub fn managed_library(kind: &str) -> Result<Option<PathBuf>> {
    if kind != "duckdb" && kind != "oracle" { return Ok(None) }
    let directory = root(kind);
    if !directory.exists() { return Ok(None) }
    verify_manifest(&directory, kind).map(Some)
}
pub fn managed_connection(req: &crate::models::ConnectionRequest) -> Result<Option<String>> {
    let kind = req.kind.id();
    if kind == "oracle" {
        // User-supplied raw ODBC connection strings continue through the existing path.
        if !req.params.trim().is_empty() && !req.params.trim().starts_with('{') { return Ok(None) }
        let Some(library) = managed_library(kind)? else { return Ok(None) };
        return oracle_connection(req, &library).map(Some)
    }
    if kind != "duckdb" || !req.params.trim().is_empty() { return Ok(None) }
    let Some(library) = managed_library("duckdb")? else { return Ok(None) };
    let database = if !req.file.trim().is_empty() { req.file.trim() } else if !req.database.trim().is_empty() { req.database.trim() } else { ":memory:" };
    Ok(Some(format!("DRIVER={};Database={}", escape_value(&library.to_string_lossy()), escape_value(database))))
}
fn oracle_connection(req: &crate::models::ConnectionRequest, library: &Path) -> Result<String> {
    let params: serde_json::Value = if req.params.trim().is_empty() { serde_json::json!({}) } else { serde_json::from_str(&req.params).context("Oracle 参数需要 JSON 对象或完整 ODBC 连接串")? };
    let params = params.as_object().ok_or_else(|| anyhow!("Oracle 参数需要 JSON 对象"))?;
    let text = |name: &str| -> Result<Option<&str>> {
        params.get(name).map(|value| value.as_str().ok_or_else(|| anyhow!("Oracle 参数 {name} 必须为字符串"))).transpose()
    };
    let host = if req.host.trim().is_empty() { "127.0.0.1" } else { req.host.trim() };
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.into() };
    let port = if req.port == 0 { 1521 } else { req.port };
    let mode = text("oracle_connect_as")?.unwrap_or("service");
    if mode != "service" && mode != "sid" { bail!("oracle_connect_as 只能是 service 或 sid") }
    let sid = text("sid")?;
    let dbq = if mode == "sid" || sid.is_some() {
        let sid = sid.filter(|sid| !sid.trim().is_empty()).ok_or_else(|| anyhow!("SID 连接需要显式填写 params.sid，不会猜测数据库名为 SID"))?;
        if [host.as_str(), sid].iter().any(|part| part.contains(['(', ')', '\0'])) { bail!("Oracle SID/主机包含不安全的描述符字符") }
        format!("(DESCRIPTION=(ADDRESS=(PROTOCOL=TCP)(HOST={host})(PORT={port}))(CONNECT_DATA=(SID={sid})))")
    } else {
        let service = text("service")?.unwrap_or(req.database.trim());
        if service.is_empty() { bail!("请填写 Oracle service name；SID 必须在 params.sid 中明确指定") }
        format!("{host}:{port}/{service}")
    };
    if [dbq.as_str(), req.user.as_str(), req.password.as_str()].iter().any(|value| value.contains('\0')) { bail!("Oracle 连接参数不得包含 NUL") }
    Ok(format!("DRIVER={};DBQ={};UID={};PWD={}", escape_value(&library.to_string_lossy()), escape_value(&dbq), escape_value(&req.user), escape_value(&req.password)))
}
fn base_status(kind: &str, status: &str, message: impl Into<String>, can_download: bool) -> DriverStatus {
    DriverStatus { kind: kind.into(), version: String::new(), status: status.into(), architecture: std::env::consts::ARCH.into(),
        message: message.into(), can_download, library: None, download_path: None }
}
fn manual_reason(kind: &str) -> &'static str {
    match kind {
        "oracle" => "当前平台/架构尚未核验 Oracle 完整 Basic + ODBC 双资产。请手动从 Oracle 官方获取同架构完整客户端与运行库；不会自动接受需要点击 Accept 的旧版许可或伪造下载。",
        "db2" | "informix" => "IBM 驱动需从官方取得匹配的完整客户端，可能要求账号、许可证或授权；当前没有经核验的可自动分发下载包。",
        "ignite" => "Ignite thin JDBC 需要 ignite-core 与 javax.cache-api 两个 JAR 组成完整类路径；当前托管/自定义 JDBC 仅支持单个 JAR，不会伪造单包下载。",
        "bigquery" | "spanner" => "官方 JDBC 驱动的 Maven 制品是缺少依赖闭包的壳包（经真实加载验证缺类）；请从官方渠道获取完整驱动发行包并使用自定义 JDBC 配置。",
        "dremio" => "官方 JDBC 仅通过 Dremio 官网分发（需接受许可），Maven Central 没有官方制品；请从官方获取并使用自定义 JDBC 配置。",
        "inceptor" | "argodb" => "星环 Inceptor/ArgoDB 的自有认证扩展未经核验；官方 Hive standalone JDBC 不作为其专有 SDK 的替代，请安装星环/Hive ODBC 驱动或使用自定义 JDBC。",
        "gbase8c" => "GBase 8c 需要手动获取厂商驱动，不能复用 GBase 8a 驱动。已知第三方固定提交 71ce03490dad3c9c3eaaa3bd62863b9a6fcdbb9a 的 agents/drivers/gbase8c/libs/gbase8c-jdbc-3.0.0B02.jar 仅是候选来源；大小、SHA-256、驱动类尚未核验，也未发布到自有镜像 GaoHu1997/Navkit-drivers，不作为运行时下载来源。请显式配置厂商 ODBC 或自定义 JDBC、实际端口和 TLS 参数；不会伪造下载或代接受许可。",
        "gbase" | "yashandb" | "sundb" | "oscar" | "xugu" => "此厂商驱动通常随授权客户端/数据库安装介质提供；请向厂商获取完整同架构 ODBC 客户端。当前没有经核验的公开直链，不会伪造下载或自动接受许可。",
        "access" => "Access ODBC 需要与应用同架构的 Microsoft Access/ACE 完整组件；安装受 Office 版本与许可影响，当前不静默运行安装器。",
        "odbc" | "jdbcx" => "通用连接没有唯一可下载驱动。请在驱动管理中选择已安装的同架构驱动、完整原生库或 DSN。",
        _ => "需要厂商的完整同架构 ODBC 客户端；可能要求登录、许可或第三方授权。当前没有经核验的公开免安装直链，请手动取得可信驱动，不会打开网页或伪造下载成功。",
    }
}
fn driver_path(value: &str) -> Option<PathBuf> {
    if value.trim().is_empty() { return None }
    let mut expanded = value.trim().trim_matches('"').to_string();
    for (key, value) in std::env::vars() { expanded = expanded.replace(&format!("%{key}%"), &value); }
    let path = PathBuf::from(&expanded);
    if path.is_absolute() { return Some(path) }
    #[cfg(target_os = "windows")]
    if let Some(windir) = std::env::var_os("WINDIR") { return Some(PathBuf::from(windir).join("System32").join(path)) }
    None
}
pub(super) fn validate_installed_library(name: &str) -> Result<()> {
    let inventory = super::list_odbc_inventory().map_err(|error| anyhow!(error))?;
    let driver = inventory.drivers.iter().find(|driver| driver.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("当前应用架构中没有安装该 ODBC 驱动：{name}"))?;
    let path = driver_path(&driver.library).ok_or_else(|| anyhow!("已安装 ODBC 驱动的主库路径无效：{name}"))?;
    validate_driver_file(&path.to_string_lossy())
}
fn statuses_from_inventory(kind: &str, inventory: Option<&super::OdbcInventory>, inventory_error: &str) -> Result<Vec<DriverStatus>> {
    let meta = catalog::meta(kind).ok_or_else(|| anyhow!("未知数据源：{kind}"))?;
    if meta.planned { return Ok(vec![base_status(kind, "unsupported", meta.note, false)]) }
    if kind == "h2" { return Ok(h2_statuses()) }
    if meta.protocol != Protocol::Odbc { return Ok(vec![base_status(kind, "builtin", "内置协议客户端，无需下载本地驱动。", false)]) }
    if kind == "oracle" && root(kind).exists() {
        let status = oracle_managed_status(&root(kind));
        // 已核验的托管 ODBC 客户端继续作为 Oracle 首选；损坏/不完整目录不再卡死，
        // 继续展示托管 thin JDBC（8.8MB）选项。路由决策见 managed_jdbc::resolve_default。
        if status.status == "ready" { return Ok(vec![status]) }
    }
    let public_download = kind == "duckdb" && duckdb_spec().is_some() || kind == "oracle" && oracle_specs().is_some();
    let mut failures = Vec::new();
    if kind == "duckdb" {
        match managed_library(kind) {
            Ok(Some(library)) => { let mut status = base_status(kind, "ready", "托管驱动清单、SHA-256、当前架构与依赖加载均已通过；数据库连接仍需实际验证。", false);
                status.library = Some(library.to_string_lossy().into_owned()); return Ok(vec![status]); },
            Err(error) => failures.push(format!("托管驱动校验失败：{error:#}")),
            Ok(None) => {},
        }
    }
    if let Some(inventory) = inventory {
        // Empty driver names cannot prove any arbitrary driver's compatibility with this database.
        for driver in inventory.drivers.iter().filter(|driver| !meta.driver.is_empty() && driver.name.eq_ignore_ascii_case(meta.driver)) {
            if let Some(path) = driver_path(&driver.library) {
                match validate_driver_file(&path.to_string_lossy()) {
                    Ok(()) => { let mut status = base_status(kind, "ready", "已安装驱动库的当前架构、文件结构与依赖加载通过；数据库连接仍需实际验证。", false);
                        status.library = Some(path.to_string_lossy().into_owned()); return Ok(vec![status]); },
                    Err(error) => failures.push(format!("{}：{error:#}", driver.name)),
                }
            } else { failures.push(format!("{} 的 Driver 库路径缺失", driver.name)); }
        }
    } else if !inventory_error.is_empty() { failures.push(format!("无法检查当前架构 ODBC 清单：{inventory_error}")); }
    if managed_jdbc::supports(kind) {
        let mut jdbc = managed_jdbc::statuses(kind);
        if !failures.is_empty() { for status in &mut jdbc { status.message = format!("{}\n{}", failures.join("；"), status.message); } }
        return Ok(jdbc);
    }
    if let Some(mut artifact) = artifacts::status(kind) {
        if !failures.is_empty() { artifact.message = format!("{}\n{}", failures.join("；"), artifact.message); }
        return Ok(vec![artifact]);
    }
    let explanation = if kind == "oracle" && public_download { "缺少完整 Oracle ODBC 驱动；可匿名直接下载官方完整 Basic + ODBC 23.26 双包（非 BasicLite），原样保留现代 Free Distribution/Hosting/Use 许可。需要 VC++ 2015–2022 x64 运行库；不会接受交互许可或执行安装器。也可直接下载官方 thin JDBC（ojdbc11，8.8MB）。" }
        else if public_download { "缺少完整 DuckDB ODBC 驱动；可直接下载官方已校验免安装包。" } else { manual_reason(kind) };
    let status = base_status(kind, if public_download { "missing" } else { "manual" },
        if failures.is_empty() { explanation.into() } else { format!("{}\n{}", failures.join("；"), explanation) }, public_download);
    Ok(vec![status])
}
#[tauri::command]
pub async fn list_driver_statuses() -> Result<Vec<DriverStatus>, String> {
    tokio::task::spawn_blocking(|| {
        // 指纹命中直接复用：弹窗每次打开都会调用，不能重复哈希/起探针。
        let fingerprint = status_fingerprint();
        if let Some(hit) = cached_driver_statuses(fingerprint) { return Ok(hit); }
        let inventory = super::list_odbc_inventory();
        let error = inventory.as_ref().err().cloned().unwrap_or_default();
        let statuses: Vec<DriverStatus> = catalog::CATALOG.iter()
            .map(|meta| statuses_from_inventory(meta.id, inventory.as_ref().ok(), &error))
            .collect::<Result<Vec<Vec<DriverStatus>>>>()
            .map(|groups: Vec<Vec<DriverStatus>>| groups.into_iter().flatten().collect())
            .map_err(|error| error.to_string())?;
        store_driver_statuses(fingerprint, &statuses);
        Ok(statuses)
    }).await.map_err(|error| error.to_string())?
}

/// RAII removes only the generated staging directory on errors/cancellation.
struct Staging(PathBuf);
impl Drop for Staging { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
fn trusted_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && matches!(url.host_str(), Some("github.com" | "api.github.com" | "raw.githubusercontent.com" | "release-assets.githubusercontent.com" | "objects.githubusercontent.com" | "repo.maven.apache.org" | "download.oracle.com" | "sfc-repo.snowflakecomputing.com" | "download.dremio.com"))
}
pub(crate) fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent("NavKit-DriverManager/1.0").no_gzip()
        .connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !trusted_url(attempt.url()) { attempt.error("驱动下载重定向到非可信地址") } else { attempt.follow() }
        })).build()?)
}
async fn download_file(spec: &DownloadSpec, path: &Path) -> Result<()> {
    download_file_limited(spec, path, MAX_DOWNLOAD).await
}
pub(crate) async fn download_file_limited(spec: &DownloadSpec, path: &Path, limit: u64) -> Result<()> {
    transfer::download(spec, path, limit).await
}
fn extract_verified_zip(archive: &Path, destination: &Path) -> Result<Vec<ManifestFile>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).context("下载内容不是完整有效的 ZIP 驱动包")?;
    if zip.len() == 0 || zip.len() > 64 { bail!("驱动包文件数量异常") }
    let mut files = Vec::new(); let mut expanded = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.enclosed_name().is_none() || !safe_relative(if entry.is_dir() { entry.name().trim_end_matches('/') } else { entry.name() }) { bail!("驱动包包含不安全路径") }
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) { bail!("驱动包不得包含符号链接") }
        if entry.is_dir() { continue }
        expanded = expanded.saturating_add(entry.size());
        if expanded > MAX_EXPANDED { bail!("驱动解压大小超过限制") }
        let name = entry.name().to_string();
        if name == "manifest.json" || files.iter().any(|file: &ManifestFile| file.name == name) { bail!("驱动包包含保留名称或重复文件") }
        let path = destination.join(&name);
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        let mut output = fs::OpenOptions::new().create_new(true).write(true).open(&path)?;
        // Read to EOF to validate ZIP CRC, then enforce advertised size.
        let copied = std::io::copy(&mut (&mut entry).take(MAX_EXPANDED + 1), &mut output)?;
        output.flush()?;
        if copied != entry.size() { bail!("驱动 ZIP 文件被截断") }
        files.push(ManifestFile { name, size: copied, sha256: digest_file(&path)? });
    }
    Ok(files)
}
fn prepare_package(staging: &Path) -> Result<()> {
    let directory = staging.join("package"); fs::create_dir(&directory)?;
    let files = extract_verified_zip(&staging.join("download.zip.partial"), &directory)?;
    let library = files.iter().find(|file| Path::new(&file.name).file_name().is_some_and(|name| name.eq_ignore_ascii_case("duckdb_odbc.dll")))
        .ok_or_else(|| anyhow!("官方包中没有 DuckDB ODBC 主库"))?.name.clone();
    let manifest = Manifest { version: 1, kind: "duckdb".into(), architecture: std::env::consts::ARCH.into(), library, files };
    fs::write(directory.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    verify_manifest(&directory, "duckdb")?;
    Ok(())
}
fn publish_package(staging: &Path) -> Result<()> { publish_package_for(staging, "duckdb") }
fn publish_package_for(staging: &Path, kind: &str) -> Result<()> {
    publication::replace(&staging.join("package"), &root(kind))?;
    Ok(())
}
const ORACLE_EXPANDED_LIMIT: u64 = 416 * 1024 * 1024;
fn oracle_entry_path(name: &str, label: &str) -> Result<String> {
    if !safe_relative(name) || !matches!(label, "basic" | "odbc") { bail!("Oracle 包含不安全路径") }
    Ok(if let Some(relative) = name.strip_prefix("instantclient_23_26/") { format!("client/{relative}") }
        else { format!("sources/{label}/{name}") })
}
fn extract_oracle_asset(archive: &Path, directory: &Path, label: &str, expanded: &mut u64, entries: &mut usize) -> Result<Vec<ManifestFile>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).context("Oracle 资产不是有效 ZIP")?;
    *entries = entries.checked_add(zip.len()).ok_or_else(|| anyhow!("Oracle ZIP 条目数量溢出"))?;
    if zip.is_empty() || *entries > 128 { bail!("Oracle 双资产 ZIP 条目总数超过 128") }
    let mut files = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.enclosed_name().is_none() || !safe_relative(if entry.is_dir() { entry.name().trim_end_matches('/') } else { entry.name() })
            || entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) { bail!("Oracle ZIP 包含不安全路径或链接") }
        if entry.is_dir() { continue }
        *expanded = expanded.checked_add(entry.size()).ok_or_else(|| anyhow!("Oracle ZIP 解压大小溢出"))?;
        if *expanded > ORACLE_EXPANDED_LIMIT { bail!("Oracle 固定双资产累计解压超过 416MiB") }
        let name = oracle_entry_path(entry.name(), label)?;
        let path = directory.join(&name);
        if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
        // create_new rejects cross-asset overwrites, including case-insensitive Windows collisions.
        let mut output = fs::OpenOptions::new().create_new(true).write(true).open(&path)?;
        let advertised = entry.size();
        let copied = std::io::copy(&mut (&mut entry).take(advertised.saturating_add(1)), &mut output)?;
        output.flush()?;
        if copied != advertised { bail!("Oracle ZIP 文件截断或实际解压大小异常") }
        files.push(ManifestFile { name, size: copied, sha256: digest_file(&path)? });
    }
    Ok(files)
}
fn prepare_oracle_package(staging: &Path) -> Result<()> {
    let specs = oracle_specs().ok_or_else(|| anyhow!("当前平台没有已核验的 Oracle 双资产"))?;
    if specs.iter().map(|spec| spec.size).sum::<u64>() > ORACLE_TOTAL_DOWNLOAD { bail!("Oracle 双资产总下载大小超过限制") }
    let directory = staging.join("package"); fs::create_dir(&directory)?;
    fs::create_dir(directory.join("archives"))?;
    let mut files = Vec::new(); let mut expanded = 0u64; let mut entries = 0usize;
    for (label, spec) in ["basic", "odbc"].iter().zip(specs) {
        let source = staging.join(format!("{label}.zip.partial"));
        if spec.size > ORACLE_ASSET_LIMIT || fs::metadata(&source)?.len() != spec.size || digest_file(&source)? != spec.sha256 { bail!("Oracle 官方原资产大小/SHA-256 不符") }
        files.extend(extract_oracle_asset(&source, &directory, label, &mut expanded, &mut entries)?);
        let name = format!("archives/{label}.zip"); fs::rename(&source, directory.join(&name))?;
        files.push(ManifestFile { name, size: spec.size, sha256: spec.sha256.into() });
    }
    let manifest = Manifest { version: 1, kind: "oracle".into(), architecture: std::env::consts::ARCH.into(), library: "client/sqora32.dll".into(), files };
    fs::write(directory.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    verify_manifest_files(&directory, "oracle")?;
    Ok(())
}
fn oracle_managed_status(directory: &Path) -> DriverStatus {
    let mut status = base_status("oracle", "manual", "", true);
    status.download_path = Some(directory.to_string_lossy().into_owned());
    match verify_manifest(directory, "oracle") {
        Ok(library) => {
            status.status = "ready".into(); status.can_download = false;
            status.library = Some(library.to_string_lossy().into_owned());
            status.message = "Oracle 官方完整 Basic + ODBC、原始 ZIP、全部许可与标识已保留；官方 SHA-256、全 ZIP CRC、当前架构 DLL、依赖加载和四个 ODBC 导出检查通过。未运行安装器或接受交互许可。默认按 host:port/service 连接，SID 须 params.sid 明确指定；仍需测试实际数据库连接。".into();
        },
        Err(error) => status.message = format!("Oracle 托管组件未通过完整性/依赖检查：{error:#}。完整下载目录：{}。缺 VC++ 2015–2022 x64 运行库时请从 Microsoft 正式安装后重新检测；不自动执行安装器，不标记驱动可用。", directory.display()),
    }
    status
}
async fn download_oracle() -> Result<DriverStatus, String> {
    let specs = oracle_specs().ok_or("当前平台/架构未核验 Oracle 双资产直链（当前仅 Windows x64）")?;
    if specs.iter().map(|spec| spec.size).sum::<u64>() > ORACLE_TOTAL_DOWNLOAD { return Err("Oracle 双资产总下载大小超过限制".into()) }
    let _permit = DOWNLOAD_GATE.get_or_init(|| tokio::sync::Semaphore::new(1)).try_acquire().map_err(|_| "已有驱动正在下载，请等待完成后重试")?;
    let parent = root("oracle").parent().ok_or("Oracle 驱动目录无效")?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let staging = Staging(parent.join(format!(".download-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&staging.0).map_err(|error| error.to_string())?;
    tokio::time::timeout(Duration::from_secs(180), async {
        for (label, spec) in ["basic", "odbc"].iter().zip(&specs) {
            download_file_limited(spec, &staging.0.join(format!("{label}.zip.partial")), ORACLE_ASSET_LIMIT).await?;
        }
        Ok::<(), anyhow::Error>(())
    }).await.map_err(|_| "Oracle 双资产下载超时；partial 已清理")?.map_err(|error| format!("Oracle 下载失败：{error:#}"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = _permit;
        prepare_oracle_package(&staging.0).and_then(|_| publish_package_for(&staging.0, "oracle")).map_err(|error| format!("Oracle 双资产校验/发布失败：{error:#}"))?;
        Ok(oracle_managed_status(&root("oracle")))
    }).await.map_err(|error| error.to_string())?
}

async fn download_h2(version: Option<&str>) -> Result<DriverStatus, String> {
    let version = version.unwrap_or(H2_RECOMMENDED).to_string();
    let spec = h2_spec_for(&version).ok_or(format!("没有已核验的 H2 官方版本 {version}；可用版本：{}", H2_VERSIONS.join("、")))?;
    let _permit = DOWNLOAD_GATE.get_or_init(|| tokio::sync::Semaphore::new(1)).try_acquire().map_err(|_| "已有驱动正在下载，请等待完成后重试")?;
    let destination = managed_h2_jar_for(&version);
    let parent = destination.parent().ok_or("H2 驱动目录无效")?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let staging = Staging(parent.join(format!(".download-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&staging.0).map_err(|error| error.to_string())?;
    let temporary = staging.0.join("h2.jar.partial");
    tokio::time::timeout(Duration::from_secs(180), download_file(&spec, &temporary)).await.map_err(|_| "H2 下载超时；partial 已清理")?
        .map_err(|error| format!("H2 下载失败：{error:#}"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = _permit;
        verify_h2_jar_for(&temporary, &version).map_err(|error| format!("H2 JAR 校验失败：{error:#}"))?;
        let retained_backup = publication::replace(&temporary, &destination)
            .map_err(|error| format!("H2 发布失败：{error:#}"))?;
        let mut status = h2_status_for(&destination, false, &version);
        status.version = version.clone();
        status.message = format!("H2 {version} 官方 JAR 已直接下载，大小、SHA-256、ZIP CRC 与 Driver.class 验证通过。{}", status.message);
        if let Some(path) = retained_backup { status.message.push_str(&format!(" 原包备份暂被占用，保留于 {}。", path.display())); }
        status.download_path = Some(destination.to_string_lossy().into_owned());
        Ok(status)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn download_database_driver(kind: String, version: Option<String>) -> Result<DriverStatus, String> {
    let meta = catalog::meta(&kind).ok_or("未知数据源")?;
    if meta.planned { return Err("此数据源适配器尚未实现，下载驱动不能使其可用".into()) }
    if kind == "h2" { return download_h2(version.as_deref()).await }
    // 托管 JDBC 优先：官方固定包比 ODBC 安装器/thin 客户端更轻（如 Firebird Jaybird、
    // Oracle thin JDBC），状态链（statuses_from_inventory）也按同一优先级展示。
    if managed_jdbc::supports(&kind) { return managed_jdbc::download(&kind, version.as_deref()).await }
    if version.is_some() { return Err("该数据源的托管驱动没有多版本目录，不能按版本下载".into()) }
    if kind == "oracle" { return download_oracle().await }
    if artifacts::supports(&kind) { return artifacts::download(&kind).await }
    if kind != "duckdb" { return Err(manual_reason(&kind).into()) }
    let spec = duckdb_spec().ok_or("当前平台/架构没有已核验的 DuckDB 免安装驱动包")?;
    let _permit = DOWNLOAD_GATE.get_or_init(|| tokio::sync::Semaphore::new(1)).try_acquire()
        .map_err(|_| "已有驱动正在下载，请等待完成后重试")?;
    let parent = root(&kind).parent().ok_or("驱动目录无效")?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let staging = Staging(parent.join(format!(".download-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&staging.0).map_err(|error| error.to_string())?;
    let result = tokio::time::timeout(Duration::from_secs(180), download_file(&spec, &staging.0.join("download.zip.partial")))
        .await.map_err(|_| "驱动下载超时；未完成文件已清理".to_string())?;
    result.map_err(|error| format!("驱动下载失败：{error:#}"))?;
    // Move the cleanup guard into the worker: cancelling IPC must not delete files the worker is using.
    tokio::task::spawn_blocking(move || -> Result<DriverStatus, String> {
        let _permit = _permit;
        prepare_package(&staging.0).and_then(|_| publish_package(&staging.0)).map_err(|error| format!("驱动包校验/安装失败：{error:#}"))?;
        let mut status = base_status("duckdb", "ready", "官方驱动已直接下载；SHA-256、ZIP CRC、同架构主库和依赖加载检查通过。未运行安装器；请测试数据库连接。", false);
        status.library = managed_library("duckdb").map_err(|error| error.to_string())?.map(|path| path.to_string_lossy().into_owned());
        status.download_path = Some(root("duckdb").to_string_lossy().into_owned());
        Ok(status)
    }).await.map_err(|error| error.to_string())?
}

/// 删除一个已下载的托管驱动版本（h2 / 托管 JDBC 产品）；被连接引用的版本拒绝删除。
#[tauri::command]
pub async fn delete_managed_driver(
    state: tauri::State<'_, crate::commands::AppState>,
    kind: String,
    version: String,
) -> Result<(), String> {
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    if !crate::driver_manager::valid_version_token(&version) {
        return Err("驱动版本号无效".into());
    }
    for connection in state.store.connections() {
        if let Ok((Some(selection), _)) = crate::driver_manager::split_version_selection(&connection.params) {
            if selection.kind == kind && selection.version == version {
                return Err(format!("驱动版本被连接「{}」使用，请先更换该连接的驱动版本或删除连接", connection.name));
            }
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    if kind == "h2" {
        if !H2_VERSIONS.contains(&version.as_str()) { return Err(format!("没有已核验的 H2 版本 {version}")); }
        dirs.push(managed_h2_jar_for(&version));
        if version == H2_RECOMMENDED { files.push(managed_h2_jar()); }
    } else {
        let Some(vendor) = managed_jdbc::vendor_version(&kind, &version) else {
            return Err(format!("数据源 {kind} 没有已核验的版本 {version}"));
        };
        dirs.push(managed_jdbc::package_dir_for(&vendor));
        // 旧布局（无版本段目录）如果正是该版本的存储，也一并删除。
        let legacy = managed_jdbc::package_dir(&kind);
        if legacy.exists() && managed_jdbc::resolve_package_dir(&vendor) == legacy { dirs.push(legacy); }
    }
    for path in &dirs {
        if path.exists() { fs::remove_dir_all(path).map_err(|error| format!("删除驱动目录失败：{error}"))?; }
    }
    for path in &files {
        if path.exists() { fs::remove_file(path).map_err(|error| format!("删除驱动文件失败：{error}"))?; }
    }
    storage::log_line("driver-manager", &format!("delete managed driver kind={kind} version={version}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status_from_inventory(kind: &str, inventory: Option<&super::super::OdbcInventory>, inventory_error: &str) -> Result<DriverStatus> {
        statuses_from_inventory(kind, inventory, inventory_error)?.into_iter().next().ok_or_else(|| anyhow!("数据源缺少推荐驱动状态"))
    }
    fn temporary() -> Staging { let path = std::env::temp_dir().join(format!("navkit-download-test-{}", uuid::Uuid::new_v4())); fs::create_dir(&path).unwrap(); Staging(path) }
    #[test] fn status_fingerprint_tracks_managed_driver_files() {
        // Empty and absent directories are different cache inputs. Use a known
        // empty, private root so probe cleanup restores the exact initial state
        // without writing into the portable application's real driver directory.
        let staging = temporary();
        let fingerprint = || status_fingerprint_at(&staging.0);
        let first = fingerprint();
        assert_eq!(first, fingerprint(), "无变化时指纹必须稳定");
        let dir = staging.0.join("fingerprint-probe-test");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("probe.bin"), b"hello").unwrap();
        let with_file = fingerprint();
        assert_ne!(with_file, first, "新增驱动文件必须改变指纹");
        fs::write(dir.join("probe.bin"), b"hello-world").unwrap();
        assert_ne!(fingerprint(), with_file, "文件内容变化必须改变指纹");
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(fingerprint(), first, "删除后指纹必须还原");
    }
    #[test] fn status_cache_returns_same_list_for_same_fingerprint() {
        let fingerprint = status_fingerprint();
        store_driver_statuses(fingerprint, &[base_status("cache-probe", "missing", "cached", true)]);
        let hit = cached_driver_statuses(fingerprint).expect("同指纹必须命中缓存");
        assert_eq!(hit.len(), 1); assert_eq!(hit[0].kind, "cache-probe");
        assert!(cached_driver_statuses(fingerprint.wrapping_add(1)).is_none(), "指纹不同不得命中");
        store_driver_statuses(u64::MAX, &[]);
    }
    #[test] fn reject_html_and_truncated_archives() {
        let staging = temporary(); let archive = staging.0.join("invalid.zip");
        for bytes in [b"<!doctype html><html>login required</html>".as_slice(), b"PK\x03\x04short".as_slice()] {
            fs::write(&archive, bytes).unwrap(); assert!(extract_verified_zip(&archive, &staging.0).is_err());
        }
    }
    #[test] fn reject_unsafe_paths() {
        for name in ["../escape.dll", "/absolute.dll", "C:/driver.dll", "a\\..\\escape.dll", "a/./b.dll"] { assert!(!safe_relative(name), "{name}"); }
        assert!(safe_relative("lib/duckdb_odbc.dll"));
    }
    #[test] fn oracle_pinned_placeholders_are_not_generic_invalid_dll_exemptions() {
        for name in ["client/extks.dll", "client/legacy.dll", "client/pkcs11.dll"] {
            assert!(oracle_provider_placeholder(name, b"Dummy File!"));
            assert!(!oracle_provider_placeholder(name, b"Dummy File?"));
            assert!(!oracle_provider_placeholder(name, b""));
            assert!(!oracle_provider_placeholder(name, b"MZ"));
        }
        for name in ["client/oci.dll", "client/sqora32.dll", "client/other.dll", "extks.dll", "../client/extks.dll"] {
            assert!(!oracle_provider_placeholder(name, b"Dummy File!"));
        }
    }
    #[test] fn builtin_and_manual_are_honest() {
        assert_eq!(status_from_inventory("mysql", None, "").unwrap().status, "builtin");
        // 通用 ODBC / JDBCX 已从数据源目录移除：不再作为可管理/可下载数据源出现（旧连接仍按 Odbc 兜底）。
        assert!(crate::catalog::meta("odbc").is_none());
        assert!(crate::catalog::meta("jdbcx").is_none());
        assert!(status_from_inventory("odbc", None, "").is_err());
        assert!(status_from_inventory("jdbcx", None, "").is_err());
        let downloadable = status_from_inventory("dameng", None, "").unwrap();
        assert_ne!(downloadable.status, "builtin");
        assert!(managed_jdbc::supports("dameng"));
        if downloadable.status == "missing" { assert!(downloadable.can_download); }
        let inventory = super::super::OdbcInventory { drivers: vec![super::super::InstalledOdbcDriver { name: "DM8 ODBC DRIVER".into(), library: "Z:\\does-not-exist.dll".into() }], dsns: vec![], architecture: std::env::consts::ARCH.into() };
        assert_ne!(status_from_inventory("dameng", Some(&inventory), "").unwrap().status, "ready");
    }
    #[test] fn gbase8c_stays_manual_and_never_borrows_8a_driver() {
        let status = status_from_inventory("gbase8c", None, "").unwrap();
        assert_eq!(status.status, "manual");
        assert!(!status.can_download);
        assert!(status.library.is_none());
        assert!(status.message.contains("GBase 8a"));
        assert!(status.message.contains("gbase8c-jdbc-3.0.0B02.jar"));
        assert!(status.message.contains("未核验"));
        let inventory = super::super::OdbcInventory { drivers: vec![super::super::InstalledOdbcDriver { name: "GBase 8a ODBC Driver".into(), library: "Z:\\missing-8a.dll".into() }], dsns: vec![], architecture: std::env::consts::ARCH.into() };
        let status = status_from_inventory("gbase8c", Some(&inventory), "").unwrap();
        assert_eq!(status.status, "manual");
        assert!(status.library.is_none());
        assert!(!managed_jdbc::supports("gbase8c"));
    }
    #[test] fn clean_staging_on_drop() { let staging = temporary(); let path = staging.0.clone(); fs::write(path.join("download.zip.partial"), b"partial").unwrap(); drop(staging); assert!(!path.exists()); }
    #[test] fn reject_changed_or_missing_manifest_files() {
        let staging = temporary(); let manifest = Manifest { version: 1, kind: "duckdb".into(), architecture: std::env::consts::ARCH.into(), library: "driver.dll".into(), files: vec![ManifestFile { name: "driver.dll".into(), size: 7, sha256: "00".repeat(32) }] };
        fs::write(staging.0.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(verify_manifest(&staging.0, "duckdb").is_err()); fs::write(staging.0.join("driver.dll"), b"changed").unwrap(); assert!(verify_manifest(&staging.0, "duckdb").is_err());
    }
    #[test] fn explicit_h2_override_cannot_fall_back_to_default() {
        let staging = temporary(); let path = staging.0.join("missing-override.jar");
        let status = h2_status_for(&path, true, H2_RECOMMENDED);
        assert_eq!(status.status, "manual"); assert!(status.can_download); assert!(status.library.is_none());
        assert!(status.message.contains("NAVKIT_H2_JAR")); assert!(status.message.contains("不会覆盖环境变量"));
        fs::write(&path, b"<!doctype html>login").unwrap(); assert!(verify_h2_structure(&path).is_err());
        assert_ne!(h2_status_for(&path, true, H2_RECOMMENDED).status, "ready");
    }
    #[test] fn oracle_service_sid_and_secret_escaping() {
        let mut req: crate::models::ConnectionRequest = serde_json::from_value(serde_json::json!({"kind":"oracle", "host":"db.example", "port":1521, "user":"u", "password":"p};UID=bad", "database":"svc.example", "params":""})).unwrap();
        let library = Path::new("C:/driver/sqora32.dll");
        let service = oracle_connection(&req, library).unwrap();
        assert!(service.contains(";DBQ=db.example:1521/svc.example;UID=u;")); assert!(service.ends_with("PWD={p}};UID=bad}"));
        req.params = r#"{"oracle_connect_as":"sid"}"#.into(); assert!(oracle_connection(&req, library).is_err());
        req.params = r#"{"sid":"ORCL"}"#.into(); let sid = oracle_connection(&req, library).unwrap();
        assert!(sid.contains("(SID=ORCL)")); assert!(!sid.contains("/svc.example"));
        req.params = r#"{"sid":"ORCL)(HOST=evil"}"#.into(); assert!(oracle_connection(&req, library).is_err());
        req.params = r#"{"oracle_connect_as":"guess"}"#.into(); assert!(oracle_connection(&req, library).is_err());
    }
    #[test] fn oracle_zip_limits_crc_and_cross_asset_collisions() {
        let staging = temporary(); let archive = staging.0.join("asset.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        writer.start_file("instantclient_23_26/fixture.dll", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        writer.write_all(b"hello").unwrap(); writer.finish().unwrap();
        let directory = staging.0.join("expanded"); fs::create_dir(&directory).unwrap();
        assert!(extract_oracle_asset(&archive, &directory, "basic", &mut (ORACLE_EXPANDED_LIMIT - 1), &mut 0).is_err());
        assert!(extract_oracle_asset(&archive, &directory, "basic", &mut 0, &mut 128).is_err());
        extract_oracle_asset(&archive, &directory, "basic", &mut 0, &mut 0).unwrap();
        assert!(extract_oracle_asset(&archive, &directory, "odbc", &mut 0, &mut 0).is_err());
        assert!(oracle_entry_path("../escape.dll", "basic").is_err());
        let mut bytes = fs::read(&archive).unwrap(); let offset = bytes.windows(5).position(|part| part == b"hello").unwrap(); bytes[offset] = b'x';
        fs::write(&archive, bytes).unwrap(); let broken = staging.0.join("broken"); fs::create_dir(&broken).unwrap();
        assert!(extract_oracle_asset(&archive, &broken, "basic", &mut 0, &mut 0).is_err());
    }
    #[test] fn incomplete_oracle_is_never_ready() {
        let staging = temporary(); let status = oracle_managed_status(&staging.0);
        assert_eq!(status.status, "manual"); assert!(status.library.is_none());
        assert!(ORACLE_EXPANDED_LIMIT > 403892957); assert_eq!(MAX_EXPANDED, 384 * 1024 * 1024);
        if let Some(specs) = oracle_specs() { assert!(specs[0].size > MAX_DOWNLOAD); assert!(specs.iter().all(|spec| spec.size <= ORACLE_ASSET_LIMIT)); assert!(specs.iter().map(|spec| spec.size).sum::<u64>() <= ORACLE_TOTAL_DOWNLOAD); }
    }
    #[tokio::test]
    #[ignore = "validates pinned official Oracle Basic+ODBC assets; optionally reuses NAVKIT_ORACLE_BASIC_ZIP/NAVKIT_ORACLE_ODBC_ZIP"]
    async fn real_official_oracle_download() {
        let staging = temporary(); let specs = oracle_specs().expect("verified Windows x64");
        for ((label, variable), spec) in [("basic", "NAVKIT_ORACLE_BASIC_ZIP"), ("odbc", "NAVKIT_ORACLE_ODBC_ZIP")].iter().zip(&specs) {
            let path = staging.0.join(format!("{label}.zip.partial"));
            if let Some(source) = std::env::var_os(variable) { fs::copy(source, &path).unwrap(); }
            else { download_file_limited(spec, &path, ORACLE_ASSET_LIMIT).await.unwrap(); }
        }
        prepare_oracle_package(&staging.0).unwrap();
        let directory = staging.0.join("package"); let library = verify_manifest_files(&directory, "oracle").unwrap();
        let status = oracle_managed_status(&directory);
        println!("REAL ORACLE ASSETS OK: Basic144192755 + ODBC1065630; pinned official SHA256, cumulative<=416MiB/128entries, every ZIP CRC, x64 DLLs, all licenses/notices and original ZIPs retained; runtime status={} message={}", status.status, status.message);
        match validate_driver_file(&library.to_string_lossy()) {
            Ok(()) => assert_eq!(status.status, "ready"),
            Err(_) => { assert_eq!(status.status, "manual"); assert!(status.library.is_none()); },
        }
    }
    #[tokio::test]
    #[ignore = "downloads pinned official H2 JAR into a temporary directory"]
    async fn real_official_h2_download() {
        let staging = temporary(); let path = staging.0.join("h2.jar.partial");
        download_file(&h2_spec(), &path).await.unwrap(); verify_h2_jar(&path).unwrap();
        println!("REAL H2 DOWNLOAD OK: official size/SHA256, every ZIP CRC, Driver.class and H2 2.x manifest");
    }
    #[tokio::test]
    #[ignore = "downloads pinned official DuckDB asset into a temporary directory"]
    async fn real_official_download() {
        let staging = temporary(); let spec = duckdb_spec().expect("verified Windows architecture");
        download_file(&spec, &staging.0.join("download.zip.partial")).await.unwrap();
        prepare_package(&staging.0).unwrap();
        let library = verify_manifest(&staging.0.join("package"), "duckdb").unwrap();
        println!("REAL DOWNLOAD OK: {} bytes, official SHA256={}, ZIP CRC, complete {:?}, matching architecture, ODBC exports and dependency loading", spec.size, spec.sha256, library);
    }
}
