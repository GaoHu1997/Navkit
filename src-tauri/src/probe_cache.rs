//! 成功的 Java 运行时探针结果缓存。
//!
//! 「新建连接」每次打开都会做驱动检测；没有缓存时，每个已下载的托管 JDBC/H2 包
//! 都要真实启动两次 java（-version + 源码编译探针），拖慢弹窗且频繁创建进程。
//! 缓存键包含 java 可执行文件与 JAR 的路径、大小和修改时间：任意一侧被替换
//! （升级 Java、重新下载驱动）都会自动重新探测。失败结果永不缓存，保证
//! 「包完整但运行时坏了」之类的问题每次都能被重新发现，也避免把超时钉死。

use anyhow::Result;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::SystemTime,
};

/// (java 路径, java mtime, jar 路径, jar 大小, jar mtime, 探针标识)。
/// java 经 PATH 解析时 mtime 为 None；PATH 在会话中途变化属可忽略的边缘情况，
/// 应用重启即清空缓存。
type Key = (PathBuf, Option<(u64, u32)>, PathBuf, u64, Option<(u64, u32)>, String);

fn stamp(path: &Path) -> Option<(u64, u32)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let since = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some((since.as_secs(), since.subsec_nanos()))
}

fn cache() -> &'static Mutex<HashMap<Key, String>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `probe` 是探针语义标识（如 "h2" / "jdbc:<driver class>:<url>"）。
/// `run` 只在缓存未命中时执行；Ok 结果写入缓存，Err 直接透传且不缓存。
pub fn cached(
    java: &Path,
    jar: &Path,
    probe: &str,
    run: impl FnOnce() -> Result<String>,
) -> Result<String> {
    let jar_metadata = std::fs::metadata(jar).map_err(|e| anyhow::anyhow!(e))?;
    let key: Key = (
        java.to_path_buf(),
        stamp(java),
        jar.to_path_buf(),
        jar_metadata.len(),
        stamp(jar),
        probe.to_string(),
    );
    if let Some(hit) = cache().lock().get(&key) {
        return Ok(hit.clone());
    }
    let result = run()?;
    cache().lock().insert(key, result.clone());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn temporary_jar() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("navkit-probe-cache-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"jar-bytes").unwrap();
        path
    }

    #[test]
    fn hits_cache_and_invalidates_on_jar_change() {
        let jar = temporary_jar();
        let java = Path::new("java-under-test");
        let calls = Cell::new(0);
        let run = |calls: &Cell<u32>| {
            calls.set(calls.get() + 1);
            Ok("probe-ok".to_string())
        };
        assert_eq!(cached(java, &jar, "p", || run(&calls)).unwrap(), "probe-ok");
        assert_eq!(cached(java, &jar, "p", || run(&calls)).unwrap(), "probe-ok");
        assert_eq!(calls.get(), 1, "第二次调用必须命中缓存而不是重新探测");
        // jar 内容变化（大小改变）→ 键不同 → 重新探测。
        std::fs::write(&jar, b"jar-bytes-changed").unwrap();
        assert_eq!(cached(java, &jar, "p", || run(&calls)).unwrap(), "probe-ok");
        assert_eq!(calls.get(), 2);
        // 不同探针标识互不命中。
        assert_eq!(cached(java, &jar, "other", || run(&calls)).unwrap(), "probe-ok");
        assert_eq!(calls.get(), 3);
        std::fs::remove_file(&jar).unwrap();
    }

    #[test]
    fn errors_are_never_cached() {
        let jar = temporary_jar();
        let java = Path::new("java-under-test");
        let mut attempts = 0;
        for expected in ["bad-1", "bad-2", "good"] {
            let result = cached(java, &jar, "err", || {
                attempts += 1;
                if expected == "good" { Ok("good".to_string()) } else { Err(anyhow::anyhow!("{expected}")) }
            });
            if expected == "good" { assert_eq!(result.unwrap(), "good"); } else { assert!(result.is_err()); }
        }
        assert_eq!(attempts, 3, "失败不能被缓存；恢复后下一次应成功");
        // 成功后开始命中。
        assert!(cached(java, &jar, "err", || { attempts += 1; unreachable!() }).is_ok());
        std::fs::remove_file(&jar).unwrap();
    }
}
