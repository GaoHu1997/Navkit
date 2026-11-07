//! Portable custom driver definitions: native ODBC or JDBC (.jar + Java 11+ sidecar).
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::AppState;
use crate::models::{ConnectionRequest, DbKind, TestResult};
use crate::{odbc_db, storage};

pub const PARAMS_PREFIX: &str = "@navkit-driver:";
#[path = "driver_download.rs"]
pub mod downloads;
static STORE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DriverSource { Installed, File, Dsn }

/// 驱动协议：ODBC（原生库/DSN）或 JDBC（本地 .jar + Java sidecar）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DriverProtocol {
    #[default]
    Odbc,
    Jdbc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverDefinition {
    pub id: String,
    pub name: String,
    /// 旧配置没有该字段时按 ODBC 处理，保持向后兼容。
    #[serde(default)]
    pub protocol: DriverProtocol,
    pub source: DriverSource,
    /// ODBC: 已安装驱动名 / 原生库绝对路径 / DSN 名。JDBC: 驱动 .jar 绝对路径。
    pub target: String,
    /// ODBC 连接串模板或 JDBC URL 模板，占位符 {{host}} {{port}} {{database}} 等。
    /// ODBC 下 {{driver}}/{{dsn}} 解析为 target 且替换值做 ODBC 转义。
    pub template: String,
    /// JDBC 可选驱动类名；留空则由 DriverManager 按 SPI 自动发现。
    #[serde(default)]
    pub driver_class: String,
    #[serde(default = "default_test_sql")]
    pub test_sql: String,
}
fn default_test_sql() -> String { "SELECT 1".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverConnectionParams {
    pub version: u32,
    pub driver_id: String,
    #[serde(default)]
    pub template: String,
}

#[derive(Debug, Serialize)]
pub struct InstalledOdbcDriver { pub name: String, pub library: String }
#[derive(Debug, Serialize)]
pub struct OdbcDsn { pub name: String, pub driver: String }
#[derive(Debug, Serialize)]
pub struct OdbcInventory {
    pub drivers: Vec<InstalledOdbcDriver>,
    pub dsns: Vec<OdbcDsn>,
    pub architecture: String,
}

/// Brace quoting protects semicolons, braces and credentials against attribute injection.
pub fn escape_value(value: &str) -> String {
    // Ordinary values can stay unquoted; some ODBC drivers do not strip braces correctly.
    if !value.is_empty() && value.trim() == value && !value.chars().any(|c| matches!(c, ';' | '{' | '}' | '\0')) {
        value.into()
    } else {
        format!("{{{}}}", value.replace('}', "}}"))
    }
}

pub fn parse_params(params: &str) -> Result<Option<DriverConnectionParams>> {
    let Some(json) = params.trim().strip_prefix(PARAMS_PREFIX) else { return Ok(None) };
    let value: DriverConnectionParams = serde_json::from_str(json).context("自定义驱动连接配置损坏")?;
    if value.version != 1 { bail!("不支持的自定义驱动配置版本") }
    if value.driver_id.is_empty() { bail!("未选择自定义驱动") }
    Ok(Some(value))
}

/// 托管驱动版本选择：连接参数前缀 `@navkit-driver-version:{json}`，
/// 与自定义驱动 PARAMS_PREFIX 互斥。extra 携带其余连接参数原样传给底层解析。
pub const VERSION_PARAMS_PREFIX: &str = "@navkit-driver-version:";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverVersionSelection {
    pub v: u32,
    /// 数据源类型（目录 id），用于把版本选择绑定到数据源，避免复制连接时错绑。
    pub kind: String,
    /// 托管驱动版本（如 "2.3.232"、"476"）。
    pub version: String,
    /// 其余连接参数（高级页文本），剥离版本选择后原样解析。
    #[serde(default)]
    pub extra: String,
}
pub(crate) fn valid_version_token(version: &str) -> bool {
    // 版本号会拼进驱动存储路径：除字符白名单外，必须排除目录穿越名 "." / ".."。
    !version.is_empty() && version.len() <= 64
        && version != "." && version != ".."
        && version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
/// 解析并剥离版本选择前缀；没有前缀时原样返回参数。
pub fn split_version_selection(params: &str) -> Result<(Option<DriverVersionSelection>, String)> {
    let Some(json) = params.trim().strip_prefix(VERSION_PARAMS_PREFIX) else { return Ok((None, params.to_string())) };
    let value: DriverVersionSelection = serde_json::from_str(json).context("托管驱动版本选择配置损坏")?;
    if value.v != 1 { bail!("不支持的托管驱动版本选择配置版本") }
    if value.kind.is_empty() || value.kind.len() > 64 || !value.kind.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') {
        bail!("托管驱动版本选择的数据源类型无效");
    }
    if !valid_version_token(&value.version) { bail!("托管驱动版本号无效") }
    if value.extra.contains('\0') || value.extra.len() > 8192 { bail!("托管驱动版本选择的附加参数无效") }
    let extra = value.extra.clone();
    Ok((Some(value), extra))
}

/// Reject literal credentials: definitions and params are portable JSON, not encrypted storage.
pub fn validate_template(template: &str) -> Result<()> {
    if template.trim().is_empty() { bail!("连接串模板不能为空") }
    if template.contains('\0') { bail!("连接串不得包含 NUL") }
    let allowed = ["driver", "dsn", "host", "port", "database", "file", "user", "password"];
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| anyhow!("未闭合的模板占位符"))?;
        if !allowed.contains(&&after[..end]) { bail!("未知占位符：{}", &after[..end]) }
        rest = &after[end + 2..];
    }
    // Credentials must be supplied via encrypted connection password, never a literal PWD.
    for part in template.split(';') {
        if let Some((key, value)) = part.split_once('=') {
            if ["pwd", "password"].contains(&key.trim().to_ascii_lowercase().as_str())
                && !["", "{{password}}"].contains(&value.trim()) {
                bail!("请将密码放在密码字段，模板使用 PWD={{{{password}}}}")
            }
        }
    }
    Ok(())
}

/// JDBC URL 模板：占位符直接替换（不做 ODBC 花括号转义）。
pub fn validate_url_template(template: &str) -> Result<()> {
    if template.trim().is_empty() { bail!("JDBC URL 模板不能为空") }
    if template.contains('\0') { bail!("JDBC URL 不得包含 NUL") }
    if !template.trim_start().starts_with("jdbc:") { bail!("JDBC URL 模板必须以 jdbc: 开头") }
    let allowed = ["driver", "host", "port", "database", "file", "user", "password"];
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| anyhow!("未闭合的模板占位符"))?;
        if !allowed.contains(&&after[..end]) { bail!("未知占位符：{}", &after[..end]) }
        rest = &after[end + 2..];
    }
    Ok(())
}

/// 渲染 JDBC URL：单遍替换，插入值原样输出（URL 语法不接受 ODBC 花括号转义）。
pub fn render_url(driver: &DriverDefinition, template: &str, req: &ConnectionRequest) -> Result<String> {
    validate_url_template(template)?;
    if [&driver.target, &req.host, &req.database, &req.file, &req.user, &req.password]
        .iter().any(|value| value.contains('\0')) { bail!("JDBC 字段不得包含 NUL") }
    let port = req.port.to_string();
    let values = [
        ("driver", driver.target.as_str()), ("host", req.host.as_str()),
        ("port", port.as_str()), ("database", req.database.as_str()),
        ("file", req.file.as_str()), ("user", req.user.as_str()),
        ("password", req.password.as_str()),
    ];
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| anyhow!("未闭合的模板占位符"))?;
        let value = values.iter().find(|(name, _)| *name == &after[..end]).unwrap().1;
        output.push_str(value);
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

/// JDBC 驱动只接受存在的绝对 .jar；ZIP 头校验拦掉伪装文件（Java 运行时在连接时校验）。
pub fn validate_jdbc_jar(path: &str) -> Result<()> {
    let path = Path::new(path);
    if !path.is_absolute() { bail!("JDBC 驱动 JAR 必须使用绝对路径") }
    if !path.is_file() { bail!("JDBC 驱动 JAR 不存在") }
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jar")) { bail!("JDBC 驱动必须是 .jar 文件") }
    let mut file = File::open(path)?;
    let mut head = [0u8; 4];
    file.read_exact(&mut head).context("无法读取 JDBC 驱动 JAR")?;
    if &head != b"PK\x03\x04" { bail!("不是有效的 JAR（ZIP）文件") }
    Ok(())
}

pub fn validate_driver_file(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jar")) {
        bail!("JDBC/JAR 不能作为原生 ODBC 驱动加载")
    }
    if !path.is_absolute() { bail!("驱动文件必须使用绝对路径") }
    if !path.is_file() { bail!("驱动文件不存在") }
    #[cfg(target_os = "windows")]
    {
        if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("dll")) {
            bail!("ODBC 驱动必须是原生 .dll")
        }
        validate_pe(path, current_pe_machine())?;
        validate_odbc_load(path)?;
    }
    #[cfg(not(target_os = "windows"))]
    { bail!("当前平台尚未实现原生驱动的完整依赖与架构校验，不能标记可用") }
    #[allow(unreachable_code)]
    Ok(())
}

fn current_pe_machine() -> u16 {
    if cfg!(target_arch = "aarch64") { 0xaa64 } else if cfg!(target_arch = "x86_64") { 0x8664 } else { 0x14c }
}

/// Validate the complete PE section ranges before any DLL is loaded. Rejects truncated/HTML files.
pub(crate) fn validate_pe(path: &Path, expected_machine: u16) -> Result<()> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos).context("无法读取驱动 PE 文件头")?;
    if &dos[..2] != b"MZ" { bail!("不是有效的 Windows 原生驱动（可能是 HTML 或损坏文件）") }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if offset < 64 || offset.saturating_add(24) > length { bail!("驱动 PE 文件头已损坏") }
    file.seek(SeekFrom::Start(offset))?;
    let mut coff = [0u8; 24]; file.read_exact(&mut coff)?;
    if &coff[..4] != b"PE\0\0" { bail!("不是有效的 PE 驱动文件") }
    let machine = u16::from_le_bytes([coff[4], coff[5]]);
    if machine != expected_machine { bail!("驱动架构与应用不匹配（需要 {}）", std::env::consts::ARCH) }
    let sections = u16::from_le_bytes([coff[6], coff[7]]) as u64;
    let optional_size = u16::from_le_bytes([coff[20], coff[21]]) as u64;
    if u16::from_le_bytes([coff[22], coff[23]]) & 0x2000 == 0 { bail!("PE 文件不是 DLL 驱动") }
    if sections == 0 || sections > 96 || optional_size < 96
        || offset.saturating_add(24 + optional_size + sections * 40) > length { bail!("驱动 PE 节表不完整") }
    let mut magic = [0u8; 2]; file.read_exact(&mut magic)?;
    let magic = u16::from_le_bytes(magic);
    if magic != if machine == 0x14c { 0x10b } else { 0x20b } { bail!("驱动 PE 位宽不匹配") }
    file.seek(SeekFrom::Start(offset + 24 + optional_size))?;
    for _ in 0..sections {
        let mut section = [0u8; 40]; file.read_exact(&mut section)?;
        let size = u32::from_le_bytes(section[16..20].try_into().unwrap()) as u64;
        let start = u32::from_le_bytes(section[20..24].try_into().unwrap()) as u64;
        if size > 0 && (start == 0 || start.saturating_add(size) > length) { bail!("驱动 PE 数据节被截断") }
    }
    Ok(())
}

// Unicode ODBC drivers may omit ANSI entry points. The Windows Driver Manager
// recognizes them by SQLConnectW; W fallbacks alone are not sufficient.
#[cfg(any(target_os = "windows", test))]
fn missing_odbc_exports(mut has: impl FnMut(&[u8]) -> bool) -> Option<&'static str> {
    if !has(b"SQLAllocHandle\0") { return Some("SQLAllocHandle") }
    if !has(b"SQLFreeHandle\0") { return Some("SQLFreeHandle") }
    let unicode = has(b"SQLConnectW\0");
    if !has(b"SQLDriverConnect\0") && !(unicode && has(b"SQLDriverConnectW\0")) {
        return Some("SQLDriverConnect / SQLConnectW + SQLDriverConnectW");
    }
    if !has(b"SQLGetInfo\0") && !(unicode && has(b"SQLGetInfoW\0")) {
        return Some("SQLGetInfo / SQLConnectW + SQLGetInfoW");
    }
    None
}

#[cfg(test)]
#[path = "driver_odbc_exports_tests.rs"]
mod odbc_exports_tests;

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
    fn FreeLibrary(module: isize) -> i32;
    fn GetProcAddress(module: isize, name: *const u8) -> *const std::ffi::c_void;
    fn GetLastError() -> u32;
}
#[cfg(target_os = "windows")]
fn validate_odbc_load(path: &Path) -> Result<()> {
    let absolute = fs::canonicalize(path)?;
    let name = wide(&absolute.to_string_lossy());
    // Only the DLL's directory and safe default directories; never the current working directory.
    let module = unsafe { LoadLibraryExW(name.as_ptr(), 0, 0x00001100) };
    if module == 0 { bail!("驱动或其依赖运行库无法加载（Windows 错误 {}）；请安装同架构厂商客户端/运行库", unsafe { GetLastError() }) }
    let missing = missing_odbc_exports(|symbol| unsafe { !GetProcAddress(module, symbol.as_ptr()).is_null() });
    unsafe { FreeLibrary(module); }
    if let Some(symbol) = missing { bail!("原生库缺少完整 ODBC 导出：{}", symbol) }
    Ok(())
}

pub fn validate_definition(driver: &DriverDefinition) -> Result<()> {
    if driver.id.is_empty() || !driver.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        bail!("驱动 ID 只能包含字母、数字、下划线或连字符")
    }
    if driver.name.trim().is_empty() || driver.target.trim().is_empty() { bail!("驱动名称和目标不能为空") }
    if driver.target.contains('\0') { bail!("驱动目标不得包含 NUL") }
    match driver.protocol {
        DriverProtocol::Odbc => validate_template(&driver.template)?,
        DriverProtocol::Jdbc => validate_url_template(&driver.template)?,
    }
    if driver.test_sql.trim().is_empty() { bail!("验证 SQL 不能为空") }
    match driver.protocol {
        DriverProtocol::Odbc => match driver.source {
            DriverSource::File => validate_driver_file(&driver.target)?,
            DriverSource::Installed => downloads::validate_installed_library(&driver.target)?,
            DriverSource::Dsn => {
                let sources = odbc_db::environment()?.data_sources()?;
                let dsn = sources.iter().find(|dsn| dsn.server_name.eq_ignore_ascii_case(&driver.target))
                    .ok_or_else(|| anyhow!("当前应用架构中没有注册该 DSN：{}", driver.target))?;
                downloads::validate_installed_library(&dsn.driver)?;
            }
        },
        DriverProtocol::Jdbc => validate_jdbc_jar(&driver.target)?,
    }
    Ok(())
}

pub fn render_template(driver: &DriverDefinition, template: &str, req: &ConnectionRequest) -> Result<String> {
    validate_template(template)?;
    if [&driver.target, &req.host, &req.database, &req.file, &req.user, &req.password]
        .iter().any(|value| value.contains('\0')) { bail!("ODBC 字段不得包含 NUL") }
    let port = req.port.to_string();
    let values = [
        ("driver", driver.target.as_str()), ("dsn", driver.target.as_str()),
        ("host", req.host.as_str()), ("port", port.as_str()),
        ("database", req.database.as_str()), ("file", req.file.as_str()),
        ("user", req.user.as_str()), ("password", req.password.as_str()),
    ];
    // Single pass: inserted data containing {{...}} must never be evaluated as template code.
    let mut output = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or_else(|| anyhow!("未闭合的模板占位符"))?;
        let value = values.iter().find(|(name, _)| *name == &after[..end]).unwrap().1;
        output.push_str(&escape_value(value));
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    Ok(output)
}

fn read_definitions(path: &Path) -> Result<Vec<DriverDefinition>> {
    if !path.exists() { return Ok(Vec::new()) }
    serde_json::from_slice(&fs::read(path)?).context("驱动配置损坏，请检查 data/drivers.json")
}
fn write_definitions(path: &Path, definitions: &[DriverDefinition]) -> Result<()> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(definitions)?)?;
    fs::rename(&tmp, path).context("保存驱动配置失败")
}
/// 解析后的驱动连接目标：ODBC 用连接串，JDBC 用 jar + URL + 可选驱动类。
pub struct DriverResolution {
    /// ODBC: 渲染后的连接串；JDBC: 驱动 .jar 绝对路径。
    pub target: String,
    /// JDBC URL（ODBC 时为空）。
    pub url: String,
    pub driver_class: String,
}

pub fn resolve_driver(req: &ConnectionRequest) -> Result<Option<DriverResolution>> {
    let Some(params) = parse_params(&req.params)? else { return Ok(None) };
    let defs = list_custom_drivers().map_err(|e| anyhow!(e))?;
    let driver = defs.iter().find(|d| d.id == params.driver_id).ok_or_else(|| anyhow!("自定义驱动已删除：{}", params.driver_id))?;
    validate_definition(driver)?;
    let template = if params.template.trim().is_empty() { &driver.template } else { &params.template };
    match driver.protocol {
        DriverProtocol::Odbc => Ok(Some(DriverResolution {
            target: render_template(driver, template, req)?,
            url: String::new(),
            driver_class: String::new(),
        })),
        DriverProtocol::Jdbc => Ok(Some(DriverResolution {
            target: driver.target.clone(),
            url: render_url(driver, template, req)?,
            driver_class: driver.driver_class.clone(),
        })),
    }
}

/// 该连接请求是否指向一个 JDBC 自定义驱动（用于 db::open 分流）。
pub fn is_jdbc_driver(req: &ConnectionRequest) -> Result<bool> {
    let Some(params) = parse_params(&req.params)? else { return Ok(false) };
    let defs = list_custom_drivers().map_err(|e| anyhow!(e))?;
    Ok(defs.iter().any(|d| d.id == params.driver_id && d.protocol == DriverProtocol::Jdbc))
}

/// Single JDBC routing decision: explicit version selection first, then custom bindings;
/// otherwise only an exact, verified managed product is selected. Usable installed ODBC
/// remains an ODBC route.
pub fn resolve_jdbc_driver(req: &ConnectionRequest) -> Result<Option<DriverResolution>> {
    let (selection, rest) = split_version_selection(&req.params)?;
    if let Some(selection) = selection {
        if selection.kind != req.kind.as_str() {
            bail!("托管驱动版本选择属于数据源 {}，与当前连接类型 {} 不一致", selection.kind, req.kind.as_str());
        }
        let mut scoped = req.clone();
        scoped.params = rest;
        return downloads::managed_jdbc::resolve_versioned(&scoped, &selection.version);
    }
    if parse_params(&req.params)?.is_some() {
        return if is_jdbc_driver(req)? { resolve_driver(req) } else { Ok(None) };
    }
    downloads::managed_jdbc::resolve_default(req)
}

pub fn resolve_connection(req: &ConnectionRequest) -> Result<Option<String>> {
    let (selection, rest) = split_version_selection(&req.params)?;
    if selection.is_some() {
        // 托管 ODBC 包（Oracle/Snowflake/DuckDB/Firebird）按平台单版本分发，没有版本选择；
        // 带 JDBC 版本前缀的请求不该走到 ODBC 连接串通道。
        bail!("该数据源的托管驱动没有可选择的版本；请移除版本选择或改用 JDBC 通道");
    }
    let mut scoped = req.clone();
    scoped.params = rest;
    let req = &scoped;
    let Some(params) = parse_params(&req.params)? else { return downloads::managed_connection(req) };
    let defs = list_custom_drivers().map_err(|e| anyhow!(e))?;
    let driver = defs.iter().find(|d| d.id == params.driver_id).ok_or_else(|| anyhow!("自定义驱动已删除：{}", params.driver_id))?;
    validate_definition(driver)?;
    if driver.protocol == DriverProtocol::Jdbc { bail!("驱动「{}」是 JDBC 驱动，请通过 JDBC 通道连接", driver.name) }
    let template = if params.template.trim().is_empty() { &driver.template } else { &params.template };
    Ok(Some(render_template(driver, template, req)?))
}

#[tauri::command]
pub fn list_custom_drivers() -> Result<Vec<DriverDefinition>, String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "驱动配置锁不可用".to_string())?;
    read_definitions(&storage::data_dir().join("drivers.json")).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn save_custom_driver(state: State<'_, AppState>, driver: DriverDefinition) -> Result<Vec<DriverDefinition>, String> {
    // Always enter the activity gate before the driver-store lock: sync commit uses the same order.
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    persist_custom_driver(driver)
}

fn persist_custom_driver(driver: DriverDefinition) -> Result<Vec<DriverDefinition>, String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "驱动配置锁不可用".to_string())?;
    let result = (|| -> Result<_> {
        validate_definition(&driver)?;
        let path = storage::data_dir().join("drivers.json");
        let mut defs = read_definitions(&path)?;
        if defs.iter().any(|d| d.id != driver.id && d.name.eq_ignore_ascii_case(&driver.name)) { bail!("驱动名称已经存在") }
        match defs.iter_mut().find(|d| d.id == driver.id) { Some(slot) => *slot = driver, None => defs.push(driver) }
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        write_definitions(&path, &defs)?;
        Ok(defs)
    })();
    result.map_err(|e| e.to_string())
}
#[cfg(test)]
pub(crate) fn save_custom_driver_for_test(driver: DriverDefinition) -> Result<Vec<DriverDefinition>, String> {
    persist_custom_driver(driver)
}

#[tauri::command]
pub fn delete_custom_driver(state: State<'_, AppState>, id: String) -> Result<Vec<DriverDefinition>, String> {
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    let _guard = STORE_LOCK.lock().map_err(|_| "驱动配置锁不可用".to_string())?;
    let result = (|| -> Result<_> {
        for connection in state.store.connections() {
            if parse_params(&connection.params)?.is_some_and(|p| p.driver_id == id) {
                bail!("驱动被连接「{}」使用，请先更换或删除该连接", connection.name)
            }
        }
        let path = storage::data_dir().join("drivers.json");
        let mut defs = read_definitions(&path)?;
        defs.retain(|d| d.id != id);
        write_definitions(&path, &defs)?;
        Ok(defs)
    })();
    result.map_err(|e| e.to_string())
}
#[tauri::command]
pub fn list_odbc_inventory() -> Result<OdbcInventory, String> {
    let result = (|| -> Result<_> {
        let env = odbc_db::environment()?;
        let drivers = env.drivers()?.into_iter().map(|d| InstalledOdbcDriver {
            name: d.description,
            library: d.attributes.get("Driver").or_else(|| d.attributes.get("DRIVER")).cloned().unwrap_or_default(),
        }).collect();
        let dsns = env.data_sources()?.into_iter().map(|d| OdbcDsn { name: d.server_name, driver: d.driver }).collect();
        Ok(OdbcInventory { drivers, dsns, architecture: std::env::consts::ARCH.into() })
    })();
    result.map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn test_custom_driver(driver: DriverDefinition, req: ConnectionRequest) -> Result<TestResult, String> {
    let password = req.password.clone();
    let outcome: Result<TestResult> = if driver.protocol == DriverProtocol::Jdbc {
        test_jdbc_driver(&driver, &req).await
    } else {
        let result = tokio::task::spawn_blocking(move || -> Result<_> {
            validate_definition(&driver)?;
            let connection_string = render_template(&driver, &driver.template, &req)?;
            let conn = odbc_db::OdbcDb::connect_string(DbKind::of("odbc"), &req, &connection_string)?;
            let sql = driver.test_sql.clone();
            Ok((conn, sql))
        }).await.map_err(|e| e.to_string())?;
        match result {
            Ok((conn, sql)) => conn.query_rows(&sql).await.map(|r| TestResult {
                ok: true, message: "真实 ODBC 连接和验证 SQL 均成功".into(),
                version: summarize_rows(&r.rows), charset: String::new(),
            }),
            Err(e) => Err(e),
        }
    };
    Ok(match outcome { Ok(r) => r, Err(e) => TestResult {
        ok: false, message: redact_error(&format!("{e:#}"), &password), version: String::new(), charset: String::new(),
    } })
}

async fn test_jdbc_driver(driver: &DriverDefinition, req: &ConnectionRequest) -> Result<TestResult> {
    validate_definition(driver)?;
    let url = render_url(driver, &driver.template, req)?;
    let db = crate::jdbc_db::JdbcDb::connect(req, &driver.target, &url, &driver.driver_class).await?;
    let r = db.query_rows(&driver.test_sql).await?;
    Ok(TestResult {
        ok: true,
        message: "真实 JDBC 连接和验证 SQL 均成功".into(),
        version: summarize_rows(&r.rows),
        charset: String::new(),
    })
}

fn summarize_rows(rows: &[Vec<Option<String>>]) -> String {
    rows.first().map(|row| row.iter().map(|v| v.as_deref().unwrap_or("NULL")).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// Windows' driver manager treats DRIVER= as an installed name, not a DLL path.
/// A uniquely named temporary user DSN points directly at the native DLL. It is removed
/// immediately after SQLDriverConnect, and also on error; no machine driver installation.
#[cfg(target_os = "windows")]
pub(crate) struct FileDsnRegistration { key: Vec<u16> }
#[cfg(target_os = "windows")]
impl Drop for FileDsnRegistration {
    fn drop(&mut self) {
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, self.key.as_ptr()); }
    }
}
#[cfg(target_os = "windows")]
const HKEY_CURRENT_USER: isize = 0x80000001u32 as i32 as isize;
#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(key: isize, subkey: *const u16, reserved: u32, class: *mut u16, options: u32, access: u32, security: *const std::ffi::c_void, result: *mut isize, disposition: *mut u32) -> i32;
    fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, length: u32) -> i32;
    fn RegCloseKey(key: isize) -> i32;
    fn RegDeleteTreeW(key: isize, subkey: *const u16) -> i32;
}
#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

#[cfg(target_os = "windows")]
pub(crate) fn prepare_file_dsn(connection_string: &str) -> Result<(String, Option<FileDsnRegistration>)> {
    // Locate a DRIVER attribute, respecting ODBC brace escaping and semicolons in values.
    let bytes = connection_string.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        while offset < bytes.len() && (bytes[offset] == b';' || bytes[offset].is_ascii_whitespace()) { offset += 1; }
        let start = offset;
        let Some(eq) = connection_string[offset..].find('=') else { break };
        let eq = offset + eq;
        let key = connection_string[start..eq].trim();
        offset = eq + 1;
        let mut value = String::new();
        if bytes.get(offset) == Some(&b'{') {
            offset += 1; let value_start = offset;
            while offset < bytes.len() {
                if bytes[offset] == b'}' {
                    if bytes.get(offset + 1) == Some(&b'}') { offset += 2; continue; }
                    value = connection_string[value_start..offset].replace("}}", "}");
                    offset += 1; break;
                }
                offset += 1;
            }
        } else {
            let value_start = offset;
            while offset < bytes.len() && bytes[offset] != b';' { offset += 1; }
            value = connection_string[value_start..offset].trim().into();
        }
        if key.eq_ignore_ascii_case("DRIVER") && Path::new(&value).is_absolute() {
            validate_driver_file(&value)?;
            // Windows limits DSN names to 32 characters; keep 96 random bits under that limit.
            let dsn = format!("NavKit_{:.24}", uuid::Uuid::new_v4().simple().to_string());
            let key_path = wide(&format!("Software\\ODBC\\ODBC.INI\\{dsn}"));
            let driver_path = wide(&fs::canonicalize(&value)?.to_string_lossy().trim_start_matches("\\\\?\\"));
            let mut key_handle = 0isize;
            let result = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, std::ptr::null_mut(), 0, 0x0002, std::ptr::null(), &mut key_handle, std::ptr::null_mut()) };
            if result != 0 { bail!("无法创建临时用户 ODBC DSN（Windows 错误 {result}）") }
            let registration = FileDsnRegistration { key: key_path };
            let result = unsafe { RegSetValueExW(key_handle, wide("Driver").as_ptr(), 0, 1, driver_path.as_ptr().cast(), (driver_path.len() * 2) as u32) };
            unsafe { RegCloseKey(key_handle); }
            if result != 0 { bail!("无法设置临时 ODBC 驱动路径（Windows 错误 {result}）") }
            let transformed = format!("{}DSN={};{}", &connection_string[..start], dsn, &connection_string[offset..].trim_start_matches(';'));
            return Ok((transformed, Some(registration)));
        }
    }
    Ok((connection_string.into(), None))
}

/// Test edits without exposing the saved plaintext password to frontend.
#[tauri::command]
pub async fn test_connection_with_saved_password(
    state: State<'_, AppState>, mut req: ConnectionRequest,
    saved_connection_id: Option<String>, use_saved_password: bool,
) -> Result<TestResult, String> {
    let _activity_guard = state.activity.enter().map_err(|e| e.to_string())?;
    if use_saved_password {
        let id = saved_connection_id.ok_or("缺少已保存连接 ID")?;
        let cfg = state.store.connections().into_iter().find(|c| c.id == id).ok_or("连接不存在")?;
        req.password = state.store.decrypt_password(&cfg.password).map_err(|_| "密码解密失败，请重新输入密码".to_string())?;
    }
    let password = req.password.clone();
    let mut result = crate::commands::test_connection(req).await.map_err(|e| redact_error(&e.to_string(), &password))?;
    result.message = redact_error(&result.message, &password);
    Ok(result)
}
pub fn redact_error(message: &str, password: &str) -> String {
    if password.is_empty() { message.into() } else { message.replace(password, "••••").replace(&escape_value(password), "••••") }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req() -> ConnectionRequest { ConnectionRequest {
        kind: DbKind::of("odbc"), host: "host;PWD=evil".into(), port: 0, user: "user".into(),
        password: "s}e;cret{{host}}".into(), database: ":memory:".into(), file: String::new(), params: String::new(), timeout_secs: 10,
    } }
    fn driver() -> DriverDefinition { DriverDefinition {
        id: "test_driver".into(), name: "Test".into(), protocol: DriverProtocol::Odbc, source: DriverSource::Installed,
        target: "ODBC Driver 17 for SQL Server".into(), template: "DRIVER={{driver}};SERVER={{host}};PWD={{password}}".into(),
        driver_class: String::new(), test_sql: "SELECT 1".into(),
    } }
    #[test] fn escapes_odbc_and_does_not_expand_inserted_placeholders() {
        let output = render_template(&driver(), &driver().template, &req()).unwrap();
        assert_eq!(output, "DRIVER=ODBC Driver 17 for SQL Server;SERVER={host;PWD=evil};PWD={s}}e;cret{{host}}}}}");
    }
    #[test] fn rejects_literal_password_and_unknown_placeholders() {
        assert!(validate_template("DSN={{dsn}};PWD=secret").is_err());
        assert!(validate_template("DRIVER={{jdbc}}").is_err());
        assert!(validate_template("DRIVER={{driver").is_err());
        assert!(validate_template("DRIVER={{driver}};PWD={{password}}").is_ok());
    }
    #[test] fn params_roundtrip_and_version_gate() {
        let value = DriverConnectionParams { version: 1, driver_id: "test_driver".into(), template: String::new() };
        let encoded = format!("{PARAMS_PREFIX}{}", serde_json::to_string(&value).unwrap());
        assert_eq!(parse_params(&encoded).unwrap().unwrap().driver_id, value.driver_id);
        assert!(parse_params("@navkit-driver:{\"version\":2,\"driver_id\":\"x\"}").is_err());
        assert!(parse_params("DRIVER={SQL Server}").unwrap().is_none());
    }
    #[test] fn version_selection_roundtrip_and_validation() {
        let selection = DriverVersionSelection { v: 1, kind: "trino".into(), version: "476".into(), extra: "ssl=true".into() };
        let encoded = format!("{VERSION_PARAMS_PREFIX}{}", serde_json::to_string(&selection).unwrap());
        let (parsed, rest) = split_version_selection(&encoded).unwrap();
        let parsed = parsed.unwrap();
        assert_eq!(parsed.kind, "trino"); assert_eq!(parsed.version, "476"); assert_eq!(rest, "ssl=true");
        // 无前缀时原样返回
        let (none, rest) = split_version_selection("DRIVER={x};SERVER=h").unwrap();
        assert!(none.is_none()); assert_eq!(rest, "DRIVER={x};SERVER=h");
        // 与自定义驱动前缀互斥：自定义前缀不被误解析为版本选择
        assert!(split_version_selection("@navkit-driver:{\"version\":1,\"driver_id\":\"x\"}").unwrap().0.is_none());
        // 校验失败场景
        let bad = |json: &str| split_version_selection(&format!("{VERSION_PARAMS_PREFIX}{json}")).is_err();
        assert!(bad("{\"v\":2,\"kind\":\"trino\",\"version\":\"476\"}"));
        assert!(bad("{\"v\":1,\"kind\":\"trino\",\"version\":\"..\"}"));
        assert!(bad("{\"v\":1,\"kind\":\"Trino/extra\",\"version\":\"476\"}"));
        assert!(bad("{not json"));
        // 与自定义驱动参数互斥：版本前缀优先且不会被 parse_params 误读
        assert!(parse_params(&encoded).unwrap().is_none());
    }
    #[test] fn managed_versions_catalog_recommended_first() {
        // 推荐版本在最前；按版本号可查到；推荐版本存在于目录。
        let all = crate::driver_manager::downloads::managed_jdbc::versions("trino");
        assert!(all.len() >= 2);
        assert_eq!(all[0].version(), "435");
        assert!(crate::driver_manager::downloads::managed_jdbc::vendor_version("trino", "476").is_some());
        assert!(crate::driver_manager::downloads::managed_jdbc::vendor_version("trino", "9999").is_none());
        // 原生协议数据源没有托管版本目录（无需选择驱动版本）。
        assert!(crate::driver_manager::downloads::managed_jdbc::versions("mysql").is_empty());
    }
    #[test] fn portable_definition_roundtrip() {
        let path = std::env::temp_dir().join(format!("navkit-drivers-{}.json", uuid::Uuid::new_v4()));
        write_definitions(&path, &[driver()]).unwrap();
        assert_eq!(read_definitions(&path).unwrap()[0].id, "test_driver");
        write_definitions(&path, &[]).unwrap();
        assert!(read_definitions(&path).unwrap().is_empty());
        fs::remove_file(path).unwrap();
    }
    fn pe_fixture(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 1024]; bytes[..2].copy_from_slice(b"MZ"); bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0"); bytes[68..70].copy_from_slice(&machine.to_le_bytes());
        bytes[70..72].copy_from_slice(&1u16.to_le_bytes()); bytes[84..86].copy_from_slice(&240u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&0x2000u16.to_le_bytes());
        bytes[88..90].copy_from_slice(&(if machine == 0x14c { 0x10bu16 } else { 0x20bu16 }).to_le_bytes());
        bytes[344..348].copy_from_slice(&512u32.to_le_bytes()); bytes[348..352].copy_from_slice(&512u32.to_le_bytes()); bytes
    }
    #[test] fn reject_html_truncated_wrong_architecture_and_width() {
        let path = std::env::temp_dir().join(format!("navkit-pe-test-{}.dll", uuid::Uuid::new_v4()));
        let mut bytes = pe_fixture(0x8664); fs::write(&path, &bytes).unwrap();
        assert!(validate_pe(&path, 0x8664).is_ok()); assert!(validate_pe(&path, 0x14c).is_err());
        fs::write(&path, &bytes[..900]).unwrap(); assert!(validate_pe(&path, 0x8664).is_err());
        bytes[88..90].copy_from_slice(&0x10bu16.to_le_bytes()); fs::write(&path, &bytes).unwrap(); assert!(validate_pe(&path, 0x8664).is_err());
        fs::write(&path, b"<!doctype html>authorization required").unwrap(); assert!(validate_pe(&path, 0x8664).is_err());
        fs::remove_file(&path).unwrap();
    }
    #[test] fn reject_executable_disguised_as_driver_dll() {
        let path = std::env::temp_dir().join(format!("navkit-not-dll-test-{}.dll", uuid::Uuid::new_v4()));
        let mut bytes = pe_fixture(current_pe_machine()); bytes[86..88].fill(0); fs::write(&path, &bytes).unwrap();
        assert!(validate_pe(&path, current_pe_machine()).is_err()); fs::remove_file(&path).unwrap();
    }
    #[cfg(target_os = "windows")]
    #[test] fn reject_real_native_library_without_odbc_exports() {
        // An ordinary system DLL exercises export validation; kernel32 is a KnownDLL
        // and some Windows loaders reject explicit safe-search loading with ERROR_INVALID_ADDRESS.
        let path = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32").join("version.dll");
        let error = validate_driver_file(&path.to_string_lossy()).unwrap_err(); assert!(error.to_string().contains("ODBC 导出"), "{error:#}");
    }
    /// Opt-in test. Pass an absolute trusted native ODBC DLL path; no registry installation required.
    #[tokio::test]
    #[ignore = "requires NAVKIT_ODBC_DRIVER_FILE pointing to a trusted native ODBC DLL"]
    async fn real_local_driver_connection_and_query() {
        let path = std::env::var("NAVKIT_ODBC_DRIVER_FILE").expect("Set NAVKIT_ODBC_DRIVER_FILE");
        let mut driver = driver(); driver.source = DriverSource::File; driver.target = path;
        driver.id = format!("verified_{}", uuid::Uuid::new_v4().simple());
        driver.template = "DRIVER={{driver}};Database={{database}}".into();
        let result = test_custom_driver(driver.clone(), req()).await.unwrap();
        assert!(result.ok, "{}", result.message); assert_eq!(result.version, "1");
        let conn_string = render_template(&driver, &driver.template, &req()).unwrap();
        let conn = odbc_db::OdbcDb::connect_string(DbKind::of("odbc"), &req(), &conn_string).unwrap();
        conn.execute("CREATE TABLE driver_check (id INTEGER, name VARCHAR)").await.unwrap();
        conn.execute("INSERT INTO driver_check VALUES (9, '真实驱动')").await.unwrap();
        let result = conn.query_rows("SELECT id, name FROM driver_check").await.unwrap();
        assert_eq!(result.columns, vec!["id", "name"]);
        assert_eq!(result.rows, vec![vec![Some("9".into()), Some("真实驱动".into())]]);
        // Fail after the native DLL is loaded: no temporary DSN may survive SQLDriverConnect.
        let mut invalid = req();
        invalid.database = "Z:\\NavKit_Driver_Test_Missing_Parent\\missing.duckdb".into();
        let failed = test_custom_driver(driver.clone(), invalid).await.unwrap();
        assert!(!failed.ok, "nonexistent parent must fail without creating a database");
        // Exercise the same persisted ID -> params -> db::open path used by saved GUI connections.
        save_custom_driver_for_test(driver.clone()).unwrap();
        let mut saved_request = req();
        saved_request.params = format!("{PARAMS_PREFIX}{}", serde_json::to_string(&DriverConnectionParams {
            version: 1, driver_id: driver.id.clone(), template: String::new(),
        }).unwrap());
        let reopened = crate::db::open(&saved_request).await.unwrap();
        assert_eq!(reopened.generic_scalar("SELECT 42").await.unwrap(), Some("42".into()));
        // Remove only this test's fresh definition; never modify another user's entries.
        let _guard = STORE_LOCK.lock().unwrap();
        let path = storage::data_dir().join("drivers.json");
        let mut defs = read_definitions(&path).unwrap();
        defs.retain(|d| d.id != driver.id); write_definitions(&path, &defs).unwrap();
        println!("REAL ODBC OK: DLL load, SELECT 1, CREATE/INSERT/SELECT unicode, persisted driver ID -> db::open -> SELECT 42");
    }
}
