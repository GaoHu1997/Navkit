//! Publish only an already verified file/directory; never discard the previous package on failure.
use anyhow::{bail, Context, Result};
use std::{fs, io, path::{Path, PathBuf}, thread, time::{Duration, Instant}};

#[derive(Clone, Copy)]
struct RetryPolicy { budget: Duration, interval: Duration }
const RETRY: RetryPolicy = RetryPolicy { budget: Duration::from_secs(3), interval: Duration::from_millis(25) };

fn temporary_lock(error: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED / SHARING_VIOLATION / LOCK_VIOLATION. A persistent
    // permissions error still fails after the same short, bounded deadline.
    cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33))
}
fn retry<T>(mut operation: impl FnMut() -> io::Result<T>, policy: RetryPolicy) -> io::Result<T> {
    let deadline = Instant::now() + policy.budget;
    loop {
        match operation() {
            Err(error) if temporary_lock(&error) && Instant::now() < deadline => {
                thread::sleep(policy.interval.min(deadline.saturating_duration_since(Instant::now())));
            }
            result => return result,
        }
    }
}
fn check_source(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { bail!("发布路径不得为 reparse point：{}", path.display()); }
    }
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        bail!("发布路径必须是普通文件或目录：{}", path.display());
    }
    Ok(())
}
fn remove_previous(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// The returned path is an old backup whose cleanup was temporarily blocked.
/// Rollback failures always return Err with the recovery path, outside staging.
/// No destination is deleted to make a rename succeed, and no copy fallback can
/// expose a partially written package. Each Windows rename has a 3-second budget.
pub(super) fn replace(source: &Path, destination: &Path) -> Result<Option<PathBuf>> {
    replace_using(source, destination, |from, to| fs::rename(from, to), remove_previous, RETRY)
}
fn replace_using(
    source: &Path,
    destination: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    mut remove: impl FnMut(&Path) -> io::Result<()>,
    policy: RetryPolicy,
) -> Result<Option<PathBuf>> {
    if source == destination { bail!("发布源与目标不得相同"); }
    check_source(source).context("完整驱动暂存路径无效")?;
    let parent = destination.parent().filter(|p| p.is_dir()).context("驱动发布父目录不存在")?;
    let previous = destination.try_exists()?;
    if previous { check_source(destination).context("原驱动路径无效")?; }
    // A Staging guard may delete the download tree at any point after this
    // function returns. Recovery data must never be placed inside that tree.
    let backup = parent.join(format!(".driver-backup-{}", uuid::Uuid::new_v4()));
    if previous {
        retry(|| rename(destination, &backup), policy).context("原驱动正在使用或目录不可写；原包保持原位")?;
    }
    if let Err(publish_error) = retry(|| rename(source, destination), policy) {
        if previous {
            let rollback = if destination.try_exists().unwrap_or(true) {
                Err(io::Error::new(io::ErrorKind::AlreadyExists, "目标被其他操作占用，不能覆盖回滚"))
            } else {
                retry(|| rename(&backup, destination), policy)
            };
            if let Err(rollback_error) = rollback {
                bail!("驱动发布失败：{publish_error}；回滚也失败：{rollback_error}。原包完整保留于 {}；请解除文件占用后恢复到 {}", backup.display(), destination.display());
            }
        }
        return Err(publish_error).context(if previous { "发布完整驱动失败；原包已恢复" } else { "发布完整驱动失败；没有替换任何旧包" });
    }
    if previous && retry(|| remove(&backup), policy).is_err() { return Ok(Some(backup)); }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("navkit-publish-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap(); Self(path)
        }
        fn directory(&self, name: &str, data: &[u8]) -> PathBuf {
            let path = self.0.join(name); fs::create_dir_all(&path).unwrap();
            fs::write(path.join("driver.jar"), data).unwrap(); path
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn short() -> RetryPolicy { RetryPolicy { budget: Duration::from_millis(80), interval: Duration::from_millis(5) } }

    #[test]
    fn verified_new_directories_and_files_replace_without_partial_copy() {
        let f = Fixture::new(); let source = f.directory("staging/package", b"new\0full");
        let destination = f.0.join("installed");
        assert_eq!(replace(&source, &destination).unwrap(), None);
        assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"new\0full");
        let source = f.directory("staging/package", b"next");
        assert_eq!(replace(&source, &destination).unwrap(), None);
        assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"next");
        let old_file = f.0.join("h2.jar"); fs::write(&old_file, b"old").unwrap();
        let new_file = f.0.join("staging/new.jar"); fs::write(&new_file, b"new").unwrap();
        assert_eq!(replace(&new_file, &old_file).unwrap(), None);
        assert_eq!(fs::read(old_file).unwrap(), b"new");
        assert!(!fs::read_dir(&f.0).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".driver-backup-")));
    }
    #[test]
    fn permanent_publication_failure_restores_the_original_bytes() {
        let f = Fixture::new(); let source = f.directory("staging/package", b"new");
        let destination = f.directory("installed", b"old");
        let error = replace_using(&source, &destination, |a, b| {
            if a == source { Err(io::Error::new(io::ErrorKind::InvalidInput, "injected permanent publish failure")) } else { fs::rename(a, b) }
        }, remove_previous, short()).unwrap_err();
        assert!(format!("{error:#}").contains("原包已恢复"));
        assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"old");
        assert_eq!(fs::read(source.join("driver.jar")).unwrap(), b"new");
    }
    #[test]
    fn failed_rollback_preserves_backup_after_staging_guard_is_dropped() {
        let f = Fixture::new(); let staging = super::super::Staging(f.0.join("staging"));
        let source = f.directory("staging/package", b"new"); let destination = f.directory("installed", b"original");
        let mut backup = None;
        let error = replace_using(&source, &destination, |a, b| {
            if a == destination { backup = Some(b.to_path_buf()); fs::rename(a, b) }
            else { Err(io::Error::new(io::ErrorKind::InvalidInput, "injected permanent publish/rollback failure")) }
        }, remove_previous, short()).unwrap_err();
        let backup = backup.unwrap(); let message = format!("{error:#}");
        assert!(message.contains("回滚也失败")); assert!(message.contains(&backup.display().to_string()));
        assert!(!backup.starts_with(&staging.0)); drop(staging);
        assert!(!f.0.join("staging").exists());
        assert_eq!(fs::read(backup.join("driver.jar")).unwrap(), b"original");
        assert!(!destination.exists());
    }
    #[test]
    fn cleanup_failure_reports_retained_backup_without_undoing_new_package() {
        let f = Fixture::new(); let source = f.directory("staging/package", b"new"); let destination = f.directory("installed", b"old");
        let backup = replace_using(&source, &destination, |from, to| fs::rename(from, to), |_| Err(io::Error::new(io::ErrorKind::InvalidInput, "cleanup failure")), short()).unwrap().unwrap();
        assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"new");
        assert_eq!(fs::read(backup.join("driver.jar")).unwrap(), b"old");
    }
    #[cfg(windows)]
    #[test]
    fn transient_windows_denials_retry_but_permanent_errors_are_bounded() {
        let f = Fixture::new(); let source = f.directory("staging/package", b"new"); let destination = f.0.join("installed");
        let mut attempts = 0;
        replace_using(&source, &destination, |a, b| {
            attempts += 1; if attempts <= 2 { Err(io::Error::from_raw_os_error(5)) } else { fs::rename(a, b) }
        }, remove_previous, short()).unwrap();
        assert_eq!(attempts, 3); assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"new");
        let source = f.directory("staging/package", b"next"); let start = Instant::now();
        assert!(replace_using(&source, &destination, |_, _| Err(io::Error::from_raw_os_error(5)), remove_previous, short()).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(fs::read(destination.join("driver.jar")).unwrap(), b"new");
    }
    #[cfg(windows)]
    #[test]
    fn actual_windows_open_handle_is_retried_until_released() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new(); let source = f.0.join("new.jar"); let destination = f.0.join("h2.jar");
        fs::write(&source, b"new").unwrap(); fs::write(&destination, b"old").unwrap();
        let handle = fs::OpenOptions::new().read(true).share_mode(1).open(&destination).unwrap();
        assert!(fs::rename(&destination, f.0.join("blocked.jar")).is_err(), "real handle must actually deny rename");
        let release = thread::spawn(move || { thread::sleep(Duration::from_millis(150)); drop(handle); });
        let result = replace(&source, &destination); release.join().unwrap(); result.unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"new");
    }
}
