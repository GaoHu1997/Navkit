//! Test-only process-wide RSS sampling. Observations are not a per-task hard memory guarantee.
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::thread::JoinHandle;
use std::time::Duration;

#[cfg(windows)]
fn rss() -> Option<u64> {
    #[repr(C)]
    struct Counters {
        cb:u32, faults:u32, peak_ws:usize, ws:usize,
        peak_paged:usize, paged:usize, peak_nonpaged:usize, nonpaged:usize,
        pagefile:usize, peak_pagefile:usize,
    }
    #[link(name="kernel32")] extern "system" { fn GetCurrentProcess() -> *mut std::ffi::c_void; }
    #[link(name="psapi")] extern "system" { fn GetProcessMemoryInfo(process:*mut std::ffi::c_void,counters:*mut Counters,bytes:u32)->i32; }
    let mut counters=Counters{cb:std::mem::size_of::<Counters>() as u32,faults:0,peak_ws:0,ws:0,peak_paged:0,paged:0,peak_nonpaged:0,nonpaged:0,pagefile:0,peak_pagefile:0};
    // SAFETY: counters is initialized repr(C) PROCESS_MEMORY_COUNTERS; the pseudo
    // process handle is valid for this process and the buffer length is exact.
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of::<Counters>() as u32) } == 0 { None }
    else { Some(counters.ws as u64) }
}
#[cfg(target_os="linux")]
fn rss() -> Option<u64> {
    let status=std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line|line.strip_prefix("VmRSS:")).and_then(|value|value.split_whitespace().next())
        .and_then(|value|value.parse::<u64>().ok()).and_then(|kb|kb.checked_mul(1024))
}
#[cfg(not(any(windows,target_os="linux")))]
fn rss() -> Option<u64> { None }

pub(super) struct Probe {
    stop:Arc<AtomicBool>, peak:Arc<AtomicU64>, samples:Arc<AtomicU64>,
    thread:Option<JoinHandle<()>>, start:Option<u64>,
}
impl Probe {
    pub(super) fn start() -> Self {
        let stop=Arc::new(AtomicBool::new(false));let peak=Arc::new(AtomicU64::new(0));let samples=Arc::new(AtomicU64::new(0));
        let start=rss();let(s,p,n)=(stop.clone(),peak.clone(),samples.clone());
        let thread=std::thread::spawn(move || {
            while !s.load(Ordering::Acquire) {
                if let Some(value)=rss(){p.fetch_max(value,Ordering::Relaxed);n.fetch_add(1,Ordering::Relaxed);}
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self{stop,peak,samples,thread:Some(thread),start}
    }
    pub(super) fn finish(mut self) -> String {
        let end=rss();if let Some(value)=end {self.peak.fetch_max(value,Ordering::Relaxed);self.samples.fetch_add(1,Ordering::Relaxed);}
        self.stop.store(true,Ordering::Release);
        let joined=self.thread.take().map(|thread|thread.join().is_ok()).unwrap_or(true);
        format!("start={:?} end={end:?} sampled_peak={} samples={} interval_ms=20 joined={joined} scope=whole_test_process_not_per_task",self.start,self.peak.load(Ordering::Relaxed),self.samples.load(Ordering::Relaxed))
    }
}
impl Drop for Probe {
    fn drop(&mut self) {self.stop.store(true,Ordering::Release);if let Some(thread)=self.thread.take(){let _=thread.join();}}
}
