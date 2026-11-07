//! Pinned official packages which require user installation, never reported ready.
use super::*;

struct Artifact {
    kind: &'static str,
    filename: &'static str,
    version: &'static str,
    package: DownloadSpec,
}
fn artifact(kind: &str) -> Option<Artifact> {
    if !cfg!(all(target_os = "windows", target_arch = "x86_64")) { return None }
    match kind {
        // Official GitHub asset and the independent Snowflake repository publish
        // the same digest. Full CDN GET and read-only MSI x64 metadata verified.
        // https://sfc-repo.snowflakecomputing.com/odbc/win64/4.3.0/index.html
        "snowflake" => Some(Artifact {
            kind: "snowflake", filename: "snowflake-odbc-4.3.0.x86_64.msi", version: "4.3.0",
            package: DownloadSpec {
                url: "https://sfc-repo.snowflakecomputing.com/odbc/win64/4.3.0/snowflake-odbc-4.3.0.x86_64.msi",
                sha256: "30d0c5c095c52f5d00ddc0c3618c6fddd1fa04186268f1c09e4f6c27d213a150", size: 24686592,
            },
        }),
        _ => None,
    }
}
pub(super) fn supports(kind: &str) -> bool { artifact(kind).is_some() }
fn verify(path: &Path, spec: &Artifact) -> Result<()> {
    if fs::metadata(path)?.len() != spec.package.size || digest_file(path)? != spec.package.sha256 {
        bail!("完整官方安装包的大小或固定 SHA-256 不符")
    }
    let mut magic = [0u8; 8]; fs::File::open(path)?.read_exact(&mut magic)?;
    if magic != [0xd0,0xcf,0x11,0xe0,0xa1,0xb1,0x1a,0xe1] { bail!("下载内容不是有效 MSI 安装数据库") }
    verify_msi_x64(path)
}
#[cfg(not(target_os = "windows"))]
fn verify_msi_x64(_: &Path) -> Result<()> { bail!("此完整安装包仅核验 Windows x64") }
#[cfg(target_os = "windows")]
fn verify_msi_x64(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "msi")]
    extern "system" {
        fn MsiOpenDatabaseW(path: *const u16, persist: *const u16, handle: *mut u32) -> u32;
        fn MsiGetSummaryInformationW(database: u32, path: *const u16, update_count: u32, handle: *mut u32) -> u32;
        fn MsiSummaryInfoGetPropertyW(handle: u32, property: u32, data_type: *mut u32, int_value: *mut i32, time: *mut u32, buffer: *mut u16, length: *mut u32) -> u32;
        fn MsiCloseHandle(handle: u32) -> u32;
    }
    struct Handle(u32);
    impl Drop for Handle { fn drop(&mut self) { unsafe { MsiCloseHandle(self.0); } } }
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut database = 0;
    // MSIDBOPEN_READONLY == 0; never install, commit, extract or run custom actions.
    let code = unsafe { MsiOpenDatabaseW(name.as_ptr(), std::ptr::null(), &mut database) };
    if code != 0 { bail!("无法只读打开官方 MSI（Windows {code}）") }
    let database = Handle(database);
    let mut summary = 0;
    let code = unsafe { MsiGetSummaryInformationW(database.0, std::ptr::null(), 0, &mut summary) };
    if code != 0 { bail!("无法读取 MSI 架构摘要（Windows {code}）") }
    let summary = Handle(summary);
    let (mut data_type, mut int_value, mut time, mut length) = (0, 0, [0u32; 2], 0);
    let code = unsafe { MsiSummaryInfoGetPropertyW(summary.0, 7, &mut data_type, &mut int_value, time.as_mut_ptr(), std::ptr::null_mut(), &mut length) };
    if !matches!(code, 0 | 234) || length == 0 || length > 4096 { bail!("MSI 架构摘要缺失或异常") }
    let mut buffer = vec![0u16; length as usize + 1]; length += 1;
    let code = unsafe { MsiSummaryInfoGetPropertyW(summary.0, 7, &mut data_type, &mut int_value, time.as_mut_ptr(), buffer.as_mut_ptr(), &mut length) };
    if code != 0 { bail!("读取 MSI 架构失败（Windows {code}）") }
    let template = String::from_utf16(&buffer[..length as usize]).context("MSI 架构摘要不是有效文本")?;
    if !template.split(';').next().is_some_and(|platform| platform.eq_ignore_ascii_case("x64")) { bail!("官方 MSI 不是 x64 包：{template}") }
    Ok(())
}
pub(super) fn status(kind: &str) -> Option<DriverStatus> {
    let spec = artifact(kind)?;
    let path = root(kind).join(spec.filename);
    let instructions = "请自行阅读安装许可并以管理员权限安装；需要 Microsoft VC++ v14 x64 运行库。未执行安装器、接受安装许可或验证远程账户。安装后重新检测完整 ODBC DLL 和依赖。";
    match verify(&path, &spec) {
        Ok(()) => {
            let mut status = base_status(kind, "manual", format!("{} {} 官方完整 x64 MSI 已下载并验证大小、固定 SHA-256、MSI 结构及 x64 摘要；下载不等于安装或驱动可用。{} 文件：{}", kind, spec.version, instructions, path.display()), false);
            status.download_path = Some(path.to_string_lossy().into_owned()); Some(status)
        },
        Err(error) => Some(base_status(kind, "missing", format!("可直接下载 {} {} 官方完整 x64 安装包到 data；{} 当前包未通过校验：{error:#}", kind, spec.version, instructions), true)),
    }
}
pub(super) async fn download(kind: &str) -> Result<DriverStatus, String> {
    let spec = artifact(kind).ok_or("此平台或数据源没有固定完整安装包")?;
    let _permit = DOWNLOAD_GATE.get_or_init(|| tokio::sync::Semaphore::new(1)).try_acquire().map_err(|_| "已有驱动正在下载，请等待完成后重试")?;
    let destination = root(kind).join(spec.filename);
    let parent = destination.parent().ok_or("驱动包目录无效")?.to_path_buf();
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let staging = Staging(parent.join(format!(".download-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&staging.0).map_err(|error| error.to_string())?;
    let temporary = staging.0.join("official-package.partial");
    tokio::time::timeout(Duration::from_secs(180), download_file(&spec.package, &temporary)).await.map_err(|_| "官方安装包下载超时；partial 已清理")?
        .map_err(|error| format!("官方完整包下载失败：{error:#}"))?;
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        let _permit = _permit;
        verify(&temporary, &spec).map_err(|error| format!("官方安装包校验失败：{error:#}"))?;
        let retained_backup = publication::replace(&temporary, &destination)
            .map_err(|error| format!("发布完整官方包失败：{error:#}"))?;
        let mut status = status(&kind).ok_or_else(|| "发布后数据源无法重新检测".to_string())?;
        if let Some(path) = retained_backup { status.message.push_str(&format!(" 原包备份暂被占用，保留于 {}。", path.display())); }
        Ok(status)
    }).await.map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_pinned_platform_and_packages_are_downloadable() {
        assert!(!supports("trino")); assert!(!supports("prestosql"));
        if cfg!(all(target_os="windows", target_arch="x86_64")) {
            let spec = artifact("snowflake").unwrap(); assert_eq!(spec.package.size, 24686592);
            assert_eq!(spec.kind, "snowflake"); assert_eq!(spec.package.sha256.len(), 64);
        }
    }
    #[test]
    fn html_or_partial_msi_cannot_become_ready() {
        if let Some(spec) = artifact("snowflake") {
            let directory = std::env::temp_dir().join(format!("navkit-artifact-{}", uuid::Uuid::new_v4())); fs::create_dir(&directory).unwrap();
            let path = directory.join("fake.msi"); fs::write(&path,b"<html>not an installer</html>").unwrap();
            assert!(verify(&path,&spec).is_err()); fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    #[ignore="downloads or validates the complete pinned official Snowflake MSI; never runs installation"]
    fn real_official_snowflake_download() {
        let spec = artifact("snowflake").expect("Windows x64 required");
        if let Some(path) = std::env::var_os("NAVKIT_SNOWFLAKE_MSI") { verify(Path::new(&path), &spec).unwrap(); }
        else {
            let directory = Staging(std::env::temp_dir().join(format!("navkit-snowflake-{}",uuid::Uuid::new_v4()))); fs::create_dir(&directory.0).unwrap();
            let file = directory.0.join("package.msi"); tokio::runtime::Runtime::new().unwrap().block_on(download_file(&spec.package,&file)).unwrap(); verify(&file,&spec).unwrap();
        }
        println!("REAL SNOWFLAKE PACKAGE OK: exact complete size/SHA256, read-only MSI x64 metadata; not installed, not driver-ready or server PASS");
    }
}
