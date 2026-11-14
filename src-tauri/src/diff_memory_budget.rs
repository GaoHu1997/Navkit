//! Accounted comparison buffers, NOT an allocator/RSS or SQLx-driver hard limit.
//! Ceilings freeze at admission; incremental reservations share one process ledger.
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SCOPE: &str = "accounted comparison buffers, not process RSS";
pub(super) const WORKSPACE: u64 = 48 * 1024 * 1024;
const MIN_LIMIT: u64 = 64 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffMemoryOptions {
    pub mode: String,
    #[serde(default)] pub limit_bytes: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffMemoryStats {
    pub mode: String, pub available_bytes: u64, pub limit_bytes: u64,
    pub used_bytes: u64, pub peak_bytes: u64, pub spilled_bytes: u64,
    pub backend: String, pub scope: String, pub pressure_events: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffMemoryPreview {
    pub available_bytes: u64, pub suggested_limit_bytes: u64, pub global_limit_bytes: u64,
    pub scope: String, pub limitations: Vec<String>,
}
pub(super) trait AvailableMemory: Send + Sync { fn available(&self) -> Result<u64>; }
struct SystemMemory;
impl AvailableMemory for SystemMemory { fn available(&self) -> Result<u64> { effective_available() } }
struct Ledger { used: u64, pending: u64, limit: u64, sample: Option<(Instant, u64, u64)> }
pub(super) struct Controller { provider: Arc<dyn AvailableMemory>, ledger: Mutex<Ledger> }
impl Controller {
    pub(super) fn new(provider: Arc<dyn AvailableMemory>) -> Arc<Self> {
        Arc::new(Self { provider, ledger: Mutex::new(Ledger { used: 0, pending: 0, limit: 0, sample: None }) })
    }
    fn sample(&self, ledger: &mut Ledger) -> Result<u64> {
        if let Some((when, available, _)) = ledger.sample {
            if when.elapsed() < Duration::from_secs(1) { return Ok(available); }
        }
        let available = self.provider.available()?;
        if available == 0 { bail!("MEMORY_BUDGET_UNAVAILABLE: 有效可用内存为零"); }
        ledger.sample = Some((Instant::now(), available, ledger.used.saturating_sub(ledger.pending)));
        Ok(available)
    }
    pub(super) fn preview(&self, opts: Option<&DiffMemoryOptions>) -> Result<DiffMemoryPreview> {
        let mut ledger = self.ledger.lock().map_err(|_| anyhow!("内存预算台账不可用"))?;
        let available = self.sample(&mut ledger)?;
        let (limit, global) = limits(available, opts)?;
        Ok(DiffMemoryPreview { available_bytes: available, suggested_limit_bytes: limit, global_limit_bytes: global,
            scope: SCOPE.into(), limitations: vec!["仅认证原生文件 SQLite；每侧独立只读快照，不是分布式原子快照".into(),
                "预算是保守比较容量核算，不包含整个进程RSS、驱动预读、allocator滞留、已返回的报告/IPC序列化或未接入预算的任务".into(),
                "最低64MiB，其中48MiB为未物理预分配的保守解码/编码/迁移/精确比较暂存reservation；不是SQLx/native/cache硬RSS证明，单行仍须通过8MiB/16MiB及最坏工作区校验".into(),
                "手动额度受后端有效available与共享池限制，超大请求会显示实际clamp；执行时重新采样".into()] })
    }
    pub(super) fn start(self: &Arc<Self>, opts: &DiffMemoryOptions) -> Result<Budget> {
        let mut ledger = self.ledger.lock().map_err(|_| anyhow!("内存预算台账不可用"))?;
        // Admission explicitly revalidates, rather than trusting a UI preview/cache.
        ledger.sample = None;
        let available = self.sample(&mut ledger)?;
        let (limit, global) = limits(available, Some(opts))?;
        // Active tasks retain the original shared ceiling; a new task cannot raise it.
        if ledger.used == 0 { ledger.limit = global; }
        else { ledger.limit = ledger.limit.min(global.saturating_add(ledger.used)); }
        drop(ledger);
        let mut budget = Budget { controller: self.clone(), mode: opts.mode.clone(), available, limit,
            used: 0, pending: 0, peak: 0, spilled: 0, pressure: 0 };
        // Workspace is an outstanding promise, not a physical 48MiB allocation.
        // Keep it pending so growth cannot spend its real headroom a second time.
        budget.reserve(WORKSPACE)?;
        Ok(budget)
    }
    #[cfg(test)] pub(super) fn used(&self) -> u64 { self.ledger.lock().unwrap().used }
    #[cfg(test)] pub(super) fn invalidate(&self) { self.ledger.lock().unwrap().sample = None; }
}
fn limits(available: u64, opts: Option<&DiffMemoryOptions>) -> Result<(u64,u64)> {
    let effective = available.saturating_sub(available / 10);
    let global = effective / 5 * 2;
    let automatic = effective / 4;
    let limit = match opts {
        Some(opts) => {
            if !["speed", "auto", "manual"].contains(&opts.mode.as_str()) { bail!("MEMORY_BUDGET_INVALID: mode必须为speed/auto/manual"); }
            if opts.mode == "manual" { opts.limit_bytes.ok_or_else(|| anyhow!("MEMORY_BUDGET_INVALID: manual缺少limit_bytes"))?.min(global) }
            else { automatic }
        }, None => automatic,
    };
    if limit < MIN_LIMIT { bail!("MEMORY_BUDGET_TOO_SMALL: 实际最高{limit}字节，最低工作预算为64MiB（不会向上虚假clamp）"); }
    Ok((limit,global))
}
pub(super) struct Budget {
    controller: Arc<Controller>, mode: String, available: u64, limit: u64,
    used: u64, pending: u64, peak: u64, spilled: u64, pressure: u32,
}
impl Budget {
    pub(super) fn reserve(&mut self, bytes: u64) -> Result<()> {
        let mut ledger = self.controller.ledger.lock().map_err(|_|anyhow!("内存预算台账不可用"))?;
        let available = match self.controller.sample(&mut ledger) {
            Ok(n) => n, Err(error) => { self.pressure = self.pressure.saturating_add(1); return Err(error); }
        };
        // available already excludes actual allocations: only outstanding commitments
        // are subtracted. Do NOT reapply 25% to a task's declining self-memory.
        let since_sample = ledger.sample.map_or(0, |(_, _, observed)| ledger.used.saturating_sub(ledger.pending).saturating_sub(observed));
        let headroom = available.saturating_sub(self.available / 10).saturating_sub(since_sample);
        if self.used.checked_add(bytes).map_or(true, |n| n > self.limit)
            || ledger.used.checked_add(bytes).map_or(true, |n| n > ledger.limit)
            || ledger.pending.checked_add(bytes).map_or(true, |n| n > headroom) {
            self.pressure = self.pressure.saturating_add(1);
            bail!("MEMORY_BUDGET_EXCEEDED: mode={} limit={} used={} requested={} available={}; 未发布不完整报告",self.mode,self.limit,self.used,bytes,available);
        }
        self.used += bytes; self.pending += bytes;
        ledger.used += bytes; ledger.pending += bytes;
        self.peak = self.peak.max(self.used);
        Ok(())
    }
    pub(super) fn commit(&mut self, bytes: u64) {
        let committed = bytes.min(self.pending); self.pending -= committed;
        if let Ok(mut ledger) = self.controller.ledger.lock() { ledger.pending -= committed; }
    }
    pub(super) fn release(&mut self, bytes: u64) {
        let released = bytes.min(self.used); self.used -= released;
        if let Ok(mut ledger) = self.controller.ledger.lock() { ledger.used -= released; }
    }
    pub(super) fn cancel_reservation(&mut self, bytes: u64) { self.commit(bytes); self.release(bytes); }
    pub(super) fn reader_buffer_rows(&mut self) -> usize {
        // Two readers can each queue seven extra native rows. Every row retains
        // the unchanged SQLite 8MiB native limit. This is a conservative outstanding
        // promise, not a physical allocation. Keep most of the ceiling for indexes.
        let extra = 2 * 7 * 8 * 1024 * 1024;
        if self.limit >= 512 * 1024 * 1024 && self.reserve(extra).is_ok() { 8 } else { 1 }
    }
    pub(super) fn allow_spill(&self) -> bool { self.mode != "speed" }
    pub(super) fn spill_bytes(&mut self, bytes: u64) { self.spilled = self.spilled.saturating_add(bytes); }
    pub(super) fn stats(&self, backend: &str) -> DiffMemoryStats {
        DiffMemoryStats { mode: self.mode.clone(), available_bytes: self.available, limit_bytes: self.limit,
            used_bytes: self.used, peak_bytes: self.peak, spilled_bytes: self.spilled,
            backend: backend.into(), scope: SCOPE.into(), pressure_events: self.pressure }
    }
}
impl Drop for Budget {
    fn drop(&mut self) { if let Ok(mut ledger) = self.controller.ledger.lock() { ledger.used -= self.used; ledger.pending -= self.pending; } }
}
pub(super) fn controller() -> &'static Arc<Controller> {
    static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();
    CONTROLLER.get_or_init(|| Controller::new(Arc::new(SystemMemory)))
}
pub fn preview_diff_memory(opts: Option<DiffMemoryOptions>) -> Result<DiffMemoryPreview> { controller().preview(opts.as_ref()) }

#[cfg(windows)]
fn effective_available() -> Result<u64> {
    #[repr(C)] struct Status { length: u32, load: u32, total_phys: u64, avail_phys: u64,
        total_page: u64, avail_page: u64, total_virtual: u64, avail_virtual: u64, avail_extended: u64 }
    #[link(name="kernel32")] extern "system" { fn GlobalMemoryStatusEx(status: *mut Status) -> i32; }
    let mut status = Status { length: std::mem::size_of::<Status>() as u32, load: 0, total_phys: 0, avail_phys: 0,
        total_page: 0, avail_page: 0, total_virtual: 0, avail_virtual: 0, avail_extended: 0 };
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 { bail!("MEMORY_BUDGET_UNAVAILABLE: GlobalMemoryStatusEx失败"); }
    Ok(status.avail_phys.min(status.avail_page))
}
#[cfg(target_os="linux")]
fn effective_available() -> Result<u64> {
    let info = std::fs::read_to_string("/proc/meminfo")?;
    let available = info.lines().find_map(|line| line.strip_prefix("MemAvailable:")).and_then(|s|s.split_whitespace().next())
        .ok_or_else(||anyhow!("MEMORY_BUDGET_UNAVAILABLE: 缺少MemAvailable"))?.parse::<u64>()?.checked_mul(1024).ok_or_else(||anyhow!("available溢出"))?;
    let membership = std::fs::read_to_string("/proc/self/cgroup")?;
    let mut effective = available;
    let mut checked = false;
    for line in membership.lines() {
        let fields = line.splitn(3, ':').collect::<Vec<_>>();
        if fields.len() != 3 { continue; }
        let (root, limit_name, used_name) = if fields[0] == "0" && fields[1].is_empty() {
            (std::path::PathBuf::from("/sys/fs/cgroup"), "memory.max", "memory.current")
        } else if fields[1].split(',').any(|c|c=="memory") {
            (std::path::PathBuf::from("/sys/fs/cgroup/memory"), "memory.limit_in_bytes", "memory.usage_in_bytes")
        } else { continue; };
        let relative = std::path::Path::new(fields[2].trim_start_matches('/'));
        if relative.components().any(|c| !matches!(c,std::path::Component::Normal(_))) { bail!("MEMORY_BUDGET_UNAVAILABLE: cgroup路径不可验证"); }
        let mut current = root.join(relative);
        loop {
            let used = std::fs::read_to_string(current.join(used_name))?.trim().parse::<u64>()?;
            let limit = match std::fs::read_to_string(current.join(limit_name)) {
                Ok(limit) => Some(limit),
                // Real cgroup v2 root has memory.current but intentionally no
                // memory.max knob. MemAvailable is its physical host constraint;
                // all visible non-root ancestors were checked, never guessed away.
                Err(error) if current == root && limit_name == "memory.max" && error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(limit)=limit { if limit.trim() != "max" { effective = effective.min(limit.trim().parse::<u64>()?.saturating_sub(used)); } }
            checked = true;
            if current == root { break; }
            if !current.pop() || !current.starts_with(&root) { bail!("MEMORY_BUDGET_UNAVAILABLE: cgroup ancestry不可验证"); }
        }
    }
    if !checked { bail!("MEMORY_BUDGET_UNAVAILABLE: 无法验证可见cgroup内存约束"); }
    Ok(effective)
}
#[cfg(not(any(windows,target_os="linux")))]
fn effective_available() -> Result<u64> { bail!("MEMORY_BUDGET_UNAVAILABLE: 当前平台有效available采样尚未认证") }
#[cfg(test)] #[path="diff_memory_budget_tests.rs"] mod tests;
