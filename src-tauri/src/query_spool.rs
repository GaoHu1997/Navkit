//! 一次查询的本机 JSONL 快照；分页与导出只读取该快照，不重新执行 SQL。
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};

const INDEX_EVERY: u64 = 1024;
const MAX_SET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ROW_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PAGE_ROWS: u64 = 5000;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const STREAM_BATCH_BYTES: usize = 4 * 1024 * 1024;
const MAX_SPOOLS: usize = 12;
const SPOOL_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const STALE_FILE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
static CACHE_BYTES: AtomicU64 = AtomicU64::new(0);

fn spool_dir() -> PathBuf {
    // 测试并行时多个 fixture 共享同一 data 目录会让 sweep() 删除彼此的缓存文件；
    // 生产环境 data_dir 天然唯一，测试下按线程隔离（不影响生产语义）。
    #[cfg(test)]
    {
        let isolate = std::env::var("NAVKIT_SPOOL_ISOLATION").ok().is_some_and(|v| v == "1");
        if isolate {
            let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
            return crate::storage::data_dir().join("cache").join("spool-test").join(thread);
        }
    }
    crate::storage::data_dir().join("cache").join("spool")
}

fn private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).context("无法创建结果缓存目录")?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)] private_windows_dir(dir)?;
    Ok(())
}

/// 显式保护 Windows DACL，不依赖父目录可能授予的 Users/Everyone 权限。
#[cfg(windows)]
fn private_windows_dir(dir: &Path) -> Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: Handle) -> Handle;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, info: Handle, length: u32, needed: *mut u32) -> i32;
        fn ConvertSidToStringSidW(sid: Handle, output: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(text: *const u16, revision: u32, descriptor: *mut Handle, size: *mut u32) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: Handle) -> i32;
    }
    struct Token(Handle);
    impl Drop for Token { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
    struct Local(Handle);
    impl Drop for Local { fn drop(&mut self) { unsafe { LocalFree(self.0); } } }
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 0x0008, &mut token) == 0 { return Err(std::io::Error::last_os_error()).context("无法读取缓存所属 Windows 用户"); }
        let token = Token(token);
        let mut needed = 0;
        GetTokenInformation(token.0, 1, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 { return Err(std::io::Error::last_os_error()).context("无法读取 Windows 用户 SID"); }
        // TOKEN_USER 的首个字段为 SID 指针；usize 缓冲区保证指针对齐。
        let mut info = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
        if GetTokenInformation(token.0, 1, info.as_mut_ptr().cast(), needed, &mut needed) == 0 { return Err(std::io::Error::last_os_error()).context("无法读取 Windows 用户 SID"); }
        let sid = *(info.as_ptr().cast::<Handle>());
        let mut sid_text = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut sid_text) == 0 { return Err(std::io::Error::last_os_error()).context("无法转换 Windows 用户 SID"); }
        let sid_memory = Local(sid_text.cast());
        let mut length = 0;
        while *sid_text.add(length) != 0 { length += 1; }
        let sid = String::from_utf16(std::slice::from_raw_parts(sid_text, length))?;
        drop(sid_memory);
        let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})").encode_utf16().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) == 0 { return Err(std::io::Error::last_os_error()).context("无法创建缓存权限"); }
        let descriptor = Local(descriptor);
        let path: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        if SetFileSecurityW(path.as_ptr(), 0x8000_0004, descriptor.0) == 0 { return Err(std::io::Error::last_os_error()).context("无法将结果缓存限制为当前 Windows 用户私有"); }
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).context("无法创建结果缓存文件")
}

struct Writer {
    out: Option<BufWriter<File>>,
    offsets: Vec<u64>,
    bytes: u64,
    reserved: u64,
    rows: u64,
    max_bytes: u64,
    finished: bool,
    stopped: Option<String>,
}

/// 单个结果集；最后一个 Arc 释放后关闭并删除文件。
pub struct SpoolSet {
    names: Vec<String>,
    types: Vec<String>,
    file: PathBuf,
    writer: parking_lot::Mutex<Writer>,
    total: AtomicU64,
    truncated: AtomicBool,
}

fn row_bytes(row: &[Option<String>]) -> usize {
    row.iter().fold(row.len().saturating_mul(std::mem::size_of::<Option<String>>()), |n, v| n.saturating_add(v.as_ref().map_or(0, String::len)))
}

/// serde_json 不得先为超大单行分配无界的 Vec。
struct BoundedLine(Vec<u8>);
impl Write for BoundedLine {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_ROW_BYTES {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "单行超过 32 MiB 缓存内存上限"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl SpoolSet {
    fn create(dir: &Path, id: &str, index: usize, names: &[String], types: &[String]) -> Result<Self> {
        private_dir(dir)?;
        let file = dir.join(format!("{id}-{index}.jsonl"));
        let out = private_file(&file)?;
        Ok(Self {
            names: names.to_vec(), types: types.to_vec(), file,
            writer: parking_lot::Mutex::new(Writer { out: Some(BufWriter::new(out)), offsets: Vec::new(), bytes: 0, reserved: 0, rows: 0, max_bytes: MAX_SET_BYTES, finished: false, stopped: None }),
            total: AtomicU64::new(0), truncated: AtomicBool::new(false),
        })
    }

    fn unavailable(names: &[String], types: &[String], error: anyhow::Error) -> Self {
        Self {
            names: names.to_vec(), types: types.to_vec(), file: PathBuf::new(),
            writer: parking_lot::Mutex::new(Writer { out: None, offsets: Vec::new(), bytes: 0, reserved: 0, rows: 0, max_bytes: MAX_SET_BYTES, finished: false, stopped: Some(format!("结果缓存无法创建：{error:#}")) }),
            total: AtomicU64::new(0), truncated: AtomicBool::new(true),
        }
    }

    fn stop(&self, writer: &mut Writer, reason: String) {
        if writer.stopped.is_none() { writer.stopped = Some(reason); }
        self.truncated.store(true, Ordering::Release);
    }

    /// 缓存失败不改变数据库执行成败，但完整导出会明确拒绝。
    pub fn push(&self, values: &[Option<String>]) {
        let mut writer = self.writer.lock();
        if writer.stopped.is_some() { return; }
        if writer.finished {
            self.stop(&mut writer, "结果缓存已结束，不能追加行".into());
            return;
        }
        if values.len() != self.names.len() {
            self.stop(&mut writer, "结果行列数与缓存元数据不一致".into());
            return;
        }
        if row_bytes(values) > MAX_ROW_BYTES {
            self.stop(&mut writer, "单行超过 32 MiB 缓存内存上限".into());
            return;
        }
        let mut line = BoundedLine(Vec::new());
        if let Err(error) = serde_json::to_writer(&mut line, values).and_then(|_| line.write_all(b"\n").map_err(serde_json::Error::io)) {
            self.stop(&mut writer, format!("结果行序列化失败：{error}"));
            return;
        }
        let len = line.0.len() as u64;
        if writer.bytes.saturating_add(len) > writer.max_bytes {
            self.stop(&mut writer, "单个结果集超过 512 MiB 缓存上限".into());
            return;
        }
        if CACHE_BYTES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(len).filter(|n| *n <= MAX_CACHE_BYTES)).is_err() {
            self.stop(&mut writer, "本机查询缓存超过 2 GiB 总配额，请释放旧结果".into());
            return;
        }
        writer.reserved += len;
        let result = writer.out.as_mut().context("结果缓存写入器不可用").and_then(|out| out.write_all(&line.0).map_err(Into::into));
        if let Err(error) = result {
            self.stop(&mut writer, format!("写入结果缓存失败：{error:#}"));
            return;
        }
        if writer.rows % INDEX_EVERY == 0 {
            let offset = writer.bytes;
            writer.offsets.push(offset);
        }
        writer.bytes += len;
        writer.rows += 1;
        self.total.store(writer.rows, Ordering::Release);
    }

    pub fn finish(&self) {
        let mut writer = self.writer.lock();
        if writer.finished { return; }
        if let Some(mut out) = writer.out.take() {
            if let Err(error) = out.flush() {
                self.stop(&mut writer, format!("刷新结果缓存失败：{error}"));
            }
        }
        writer.finished = true;
    }

    pub fn total(&self) -> u64 { self.total.load(Ordering::Acquire) }
    pub fn truncated(&self) -> bool { self.truncated.load(Ordering::Acquire) }
    pub fn stop_reason(&self) -> Option<String> { self.writer.lock().stopped.clone() }
    pub fn names(&self) -> Vec<String> { self.names.clone() }
    pub fn types(&self) -> Vec<String> { self.types.clone() }

    pub fn ensure_complete(&self) -> Result<()> {
        let writer = self.writer.lock();
        if !writer.finished { bail!("查询结果仍在写入，请等待执行完成后导出"); }
        if let Some(reason) = &writer.stopped {
            bail!("本次结果只缓存了前 {} 行（{}），无法导出完整结果；请缩小结果集或分批查询", writer.rows, reason);
        }
        Ok(())
    }

    fn readable(&self) -> Result<()> {
        if !self.writer.lock().finished { bail!("查询结果仍在写入，请等待执行完成后分页"); }
        Ok(())
    }

    fn read_row(&self, reader: &mut impl BufRead, buffer: &mut Vec<u8>) -> Result<Vec<Option<String>>> {
        buffer.clear();
        loop {
            let available = reader.fill_buf().context("读取结果缓存失败")?;
            if available.is_empty() { bail!("结果缓存损坏：在预期行数之前意外结束"); }
            let take = available.iter().position(|b| *b == b'\n').map_or(available.len(), |n| n + 1);
            if buffer.len().saturating_add(take) > MAX_ROW_BYTES { bail!("结果缓存损坏：单行超过 32 MiB 上限"); }
            buffer.extend_from_slice(&available[..take]);
            reader.consume(take);
            if buffer.last() == Some(&b'\n') { break; }
        }
        struct RowVisitor { columns: usize }
        impl<'de> serde::de::Visitor<'de> for RowVisitor {
            type Value = Vec<Option<String>>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("有界的查询结果行") }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
                let mut row = Vec::with_capacity(self.columns.min(4096));
                let mut bytes = self.columns.saturating_mul(std::mem::size_of::<Option<String>>());
                if bytes > MAX_ROW_BYTES { return Err(serde::de::Error::custom("结果行列数超过内存预算")); }
                for _ in 0..self.columns {
                    let value: Option<String> = seq.next_element()?.ok_or_else(|| serde::de::Error::custom("结果行列数不足"))?;
                    bytes = bytes.saturating_add(value.as_ref().map_or(0, String::len));
                    if bytes > MAX_ROW_BYTES { return Err(serde::de::Error::custom("结果行超过内存预算")); }
                    row.push(value);
                }
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() { return Err(serde::de::Error::custom("结果行列数过多")); }
                Ok(row)
            }
        }
        let mut deserializer = serde_json::Deserializer::from_slice(buffer);
        let values = serde::de::Deserializer::deserialize_seq(&mut deserializer, RowVisitor { columns: self.names.len() }).context("结果缓存损坏：JSONL 行无效")?;
        deserializer.end().context("结果缓存损坏：行尾有额外内容")?;
        Ok(values)
    }

    /// 每页最多 5000 行、8 MiB 返回值；不足一整行时给出可理解的错误。
    pub fn fetch(&self, offset: u64, limit: u64) -> Result<Vec<Vec<Option<String>>>> {
        if limit > MAX_PAGE_ROWS { bail!("每页最多读取 5000 行"); }
        self.readable()?;
        let total = self.total();
        if limit == 0 || offset >= total { return Ok(Vec::new()); }
        let start = self.writer.lock().offsets.get((offset / INDEX_EVERY) as usize).copied().context("结果缓存索引损坏")?;
        let mut file = File::open(&self.file).context("结果缓存已失效")?;
        file.seek(SeekFrom::Start(start))?;
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        for _ in 0..offset % INDEX_EVERY { self.read_row(&mut reader, &mut buffer)?; }
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        for _ in 0..limit.min(total - offset) {
            let values = self.read_row(&mut reader, &mut buffer)?;
            let size = row_bytes(&values).saturating_add(std::mem::size_of::<Vec<Option<String>>>());
            if bytes.saturating_add(size.max(buffer.len())) > MAX_PAGE_BYTES {
                if rows.is_empty() { bail!("该行超过分页的 8 MiB 内存上限；请导出完整结果或在 SQL 中缩小大字段"); }
                break;
            }
            bytes += size.max(buffer.len());
            rows.push(values);
        }
        Ok(rows)
    }

    /// 按行数和 4 MiB 字节预算分批；单行最高 32 MiB，单行独占一批。
    pub fn stream<F>(&self, batch: usize, mut each: F) -> Result<u64>
    where F: FnMut(Vec<Vec<Option<String>>>) -> Result<()> {
        self.ensure_complete()?;
        let total = self.total();
        let file = File::open(&self.file).context("结果缓存已失效")?;
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        let mut count = 0u64;
        while count < total {
            let row = self.read_row(&mut reader, &mut buffer)?;
            let size = row_bytes(&row).max(buffer.len());
            if !rows.is_empty() && bytes.saturating_add(size) > STREAM_BATCH_BYTES {
                each(std::mem::take(&mut rows))?;
                bytes = 0;
            }
            bytes += size;
            rows.push(row);
            count += 1;
            if rows.len() >= batch.clamp(1, MAX_PAGE_ROWS as usize) || bytes >= STREAM_BATCH_BYTES {
                each(std::mem::take(&mut rows))?;
                bytes = 0;
            }
        }
        if !rows.is_empty() { each(rows)?; }
        if !reader.fill_buf()?.is_empty() { bail!("结果缓存损坏：预期行数之后仍有多余数据"); }
        Ok(count)
    }
}

impl Drop for SpoolSet {
    fn drop(&mut self) {
        let writer = self.writer.get_mut();
        drop(writer.out.take());
        if !self.file.as_os_str().is_empty() { let _ = std::fs::remove_file(&self.file); }
        CACHE_BYTES.fetch_sub(writer.reserved, Ordering::AcqRel);
    }
}

pub struct Spool {
    pub id: String,
    pub connection: String,
    pub owner: String,
    pub namespace: String,
    pub principal: String,
    dir: PathBuf,
    sets: parking_lot::Mutex<Vec<Option<Arc<SpoolSet>>>>,
    created: Instant,
    completed: AtomicBool,
}

impl Spool {
    pub fn begin_set(&self, index: usize, names: &[String], types: &[String]) -> Arc<SpoolSet> {
        let mut sets = self.sets.lock();
        while sets.len() <= index { sets.push(None); }
        if let Some(set) = &sets[index] { return set.clone(); }
        let set = Arc::new(SpoolSet::create(&self.dir, &self.id, index, names, types)
            .unwrap_or_else(|error| SpoolSet::unavailable(names, types, error)));
        live_sets().lock().push(Arc::downgrade(&set));
        if self.completed.load(Ordering::Acquire) { set.finish(); }
        sets[index] = Some(set.clone());
        set
    }

    pub fn set(&self, index: usize) -> Result<Arc<SpoolSet>> {
        self.sets.lock().get(index).and_then(|s| s.clone()).context("该结果集没有可用的完整结果缓存")
    }

    pub fn finish_all(&self) {
        let sets = self.sets.lock();
        for set in sets.iter().flatten() { set.finish(); }
        self.completed.store(true, Ordering::Release);
    }

    fn idle(&self) -> bool {
        self.completed.load(Ordering::Acquire) && self.sets.lock().iter().flatten().all(|set| Arc::strong_count(set) == 1)
    }
}

fn registry() -> &'static parking_lot::Mutex<HashMap<String, Arc<Spool>>> {
    static SPOOLS: std::sync::OnceLock<parking_lot::Mutex<HashMap<String, Arc<Spool>>>> = std::sync::OnceLock::new();
    SPOOLS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

/// 保留弱引用，已从注册表释放但仍被执行/导出持有的文件也不得被 sweep 删除。
fn live_spools() -> &'static parking_lot::Mutex<Vec<std::sync::Weak<Spool>>> {
    static LIVE: std::sync::OnceLock<parking_lot::Mutex<Vec<std::sync::Weak<Spool>>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
}
fn live_sets() -> &'static parking_lot::Mutex<Vec<std::sync::Weak<SpoolSet>>> {
    static LIVE: std::sync::OnceLock<parking_lot::Mutex<Vec<std::sync::Weak<SpoolSet>>>> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| parking_lot::Mutex::new(Vec::new()))
}

fn sweep() {
    let now = Instant::now();
    registry().lock().retain(|_, s| !(now.duration_since(s.created) > SPOOL_TTL && Arc::strong_count(s) == 1 && s.idle()));
    let mut protected = HashSet::new();
    let mut live = live_spools().lock();
    live.retain(|weak| {
        let Some(spool) = weak.upgrade() else { return false; };
        protected.insert(spool.id.clone());
        true
    });
    let mut sets = live_sets().lock();
    let mut files = HashSet::new();
    sets.retain(|weak| {
        let Some(set) = weak.upgrade() else { return false; };
        files.insert(set.file.clone());
        true
    });
    if let (Some(cutoff), Ok(entries)) = (SystemTime::now().checked_sub(STALE_FILE_AGE), std::fs::read_dir(spool_dir())) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // UUID 中本来就有 '-'，必须只切分最后一个结果集编号。
            let owner = name.strip_suffix(".jsonl").and_then(|s| s.rsplit_once('-')).and_then(|(id, index)| index.parse::<usize>().ok().map(|_| id));
            let Some(owner) = owner else { continue; };
            if uuid::Uuid::parse_str(owner).is_err() || protected.contains(owner) || files.contains(&entry.path()) { continue; }
            let stale = entry.metadata().map(|m| m.is_file() && m.modified().map(|t| t < cutoff).unwrap_or(false)).unwrap_or(false);
            if stale { let _ = std::fs::remove_file(entry.path()); }
        }
    }
}

/// 创建失败的新执行不得让同页上一次成功的结果失效。
pub fn create(connection: &str, owner: &str, namespace: &str, principal: &str) -> Arc<Spool> {
    sweep();
    let mut all = registry().lock();
    while all.len() >= MAX_SPOOLS {
        let oldest = all.iter().filter(|(_, s)| Arc::strong_count(s) == 1 && s.idle()).min_by_key(|(_, s)| s.created).map(|(id, _)| id.clone());
        let Some(id) = oldest else { break; };
        all.remove(&id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let spool = Arc::new(Spool {
        id: id.clone(), connection: connection.into(), owner: owner.into(), namespace: namespace.into(), principal: principal.into(),
        dir: spool_dir(), sets: parking_lot::Mutex::new(Vec::new()), created: Instant::now(), completed: AtomicBool::new(false),
    });
    all.insert(id, spool.clone());
    live_spools().lock().push(Arc::downgrade(&spool));
    spool
}

pub fn authorize(id: &str, connection: &str, owner: &str, namespace: &str, principal: &str) -> Result<Arc<Spool>> {
    sweep();
    let spool = registry().lock().get(id).cloned().context("结果缓存不存在或已被清理；请重新执行查询")?;
    if spool.connection != connection || spool.owner != owner || spool.namespace != namespace || spool.principal != principal {
        bail!("结果缓存不属于当前用户、查询页或连接");
    }
    Ok(spool)
}

pub fn drop_spool(id: &str) { registry().lock().remove(id); }
pub fn retained() -> usize { registry().lock().len() }
pub fn exists(id: &str) -> bool { registry().lock().contains_key(id) }

#[cfg(test)]
pub(crate) fn test_set(dir: &Path, names: &[String], types: &[String], quota: u64) -> Arc<SpoolSet> {
    let set = Arc::new(SpoolSet::create(dir, &uuid::Uuid::new_v4().to_string(), 0, names, types).unwrap());
    set.writer.lock().max_bytes = quota;
    set
}
#[cfg(test)]
pub(crate) fn corrupt_test_set(set: &SpoolSet, bytes: &[u8]) { std::fs::write(&set.file, bytes).unwrap(); }

#[cfg(test)]
#[path = "query_spool_tests.rs"]
mod tests;
