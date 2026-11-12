//! Failure-only Restart Manager observation for real query fixture cleanup.
//! This module never closes another process, retries deletion, or delays a test.
use std::path::PathBuf;

#[cfg(not(windows))]
pub(super) fn describe_file_users(paths: &[PathBuf]) -> String {
    format!("restart_manager unavailable on this platform; self_pid={}; resources={paths:?}", std::process::id())
}

#[cfg(windows)]
pub(super) fn describe_file_users(paths: &[PathBuf]) -> String {
    windows::describe(paths)
}

#[cfg(windows)]
mod windows {
    use std::{os::windows::ffi::OsStrExt, path::PathBuf, ptr};

    const ERROR_MORE_DATA: u32 = 234;
    const MAX_PROCESSES: u32 = 4096;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UniqueProcess { pid: u32, started: FileTime }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessInfo {
        process: UniqueProcess,
        app_name: [u16; 256],
        service_name: [u16; 64],
        app_type: i32,
        app_status: u32,
        terminal_session: u32,
        restartable: i32,
    }
    impl ProcessInfo {
        const EMPTY: Self = Self {
            process: UniqueProcess { pid: 0, started: FileTime { low: 0, high: 0 } },
            app_name: [0; 256], service_name: [0; 64], app_type: 0,
            app_status: 0, terminal_session: 0, restartable: 0,
        };
    }

    // Win32 Restart Manager ABI: DWORD/UINT are u32, BOOL/RM_APP_TYPE are i32,
    // WCHAR is u16, and RM_UNIQUE_PROCESS's FILETIME consists of two DWORDs.
    #[link(name = "Rstrtmgr")]
    extern "system" {
        fn RmStartSession(handle: *mut u32, flags: u32, key: *mut u16) -> u32;
        fn RmRegisterResources(handle: u32, files: u32, filenames: *const *const u16,
            applications: u32, processes: *const UniqueProcess, services: u32,
            service_names: *const *const u16) -> u32;
        fn RmGetList(handle: u32, needed: *mut u32, count: *mut u32,
            affected: *mut ProcessInfo, reboot_reasons: *mut u32) -> u32;
        fn RmEndSession(handle: u32) -> u32;
    }

    struct Session { handle: u32, ended: bool }
    impl Session {
        fn end(mut self) -> u32 {
            self.ended = true;
            // SAFETY: handle was successfully produced by RmStartSession and is ended once.
            unsafe { RmEndSession(self.handle) }
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            if !self.ended {
                // SAFETY: unwind/early-exit fallback for this owned Restart Manager session.
                unsafe { RmEndSession(self.handle); }
            }
        }
    }
    fn text(value: &[u16]) -> String {
        String::from_utf16_lossy(&value[..value.iter().position(|ch| *ch == 0).unwrap_or(value.len())])
    }

    pub(super) fn describe(paths: &[PathBuf]) -> String {
        let self_pid = std::process::id();
        let mut handle = 0;
        let mut key = [0u16; 33]; // CCH_RM_SESSION_KEY + terminating NUL
        // SAFETY: both output buffers are valid for the documented Win32 call.
        let start = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        if start != 0 {
            return format!("restart_manager self_pid={self_pid}; resources={paths:?}; start_status={start}");
        }
        let session = Session { handle, ended: false };
        let names: Vec<Vec<u16>> = paths.iter().map(|path| path.as_os_str().encode_wide().chain(Some(0)).collect()).collect();
        if names.len() > 1024 || names.iter().any(|name| name[..name.len() - 1].contains(&0)) {
            let end = session.end();
            return format!("restart_manager self_pid={self_pid}; start_status={start}; invalid_resource_paths; end_status={end}");
        }
        let pointers: Vec<*const u16> = names.iter().map(|name| name.as_ptr()).collect();
        // SAFETY: names/pointers stay alive throughout registration; application/service lists are empty.
        let registered = unsafe { RmRegisterResources(handle, pointers.len() as u32, pointers.as_ptr(),
            0, ptr::null(), 0, ptr::null()) };
        if registered != 0 {
            let end = session.end();
            return format!("restart_manager self_pid={self_pid}; resources={paths:?}; start_status={start}; register_status={registered}; end_status={end}");
        }
        let mut needed = 0;
        let mut count = 0;
        let mut reboot = 0;
        // SAFETY: capacity query uses no process buffer, with valid DWORD output pointers.
        let capacity_status = unsafe { RmGetList(handle, &mut needed, &mut count, ptr::null_mut(), &mut reboot) };
        let mut list_status = None;
        let mut owners = Vec::new();
        if capacity_status == ERROR_MORE_DATA && needed > 0 && needed <= MAX_PROCESSES {
            let mut affected = vec![ProcessInfo::EMPTY; needed as usize];
            count = affected.len() as u32;
            // SAFETY: affected contains count writable ABI-compatible entries; all output pointers are valid.
            let status = unsafe { RmGetList(handle, &mut needed, &mut count, affected.as_mut_ptr(), &mut reboot) };
            list_status = Some(status);
            if status == 0 && count as usize <= affected.len() {
                owners = affected[..count as usize].iter().map(|app| {
                    let started = (u64::from(app.process.started.high) << 32) | u64::from(app.process.started.low);
                    format!("pid={} is_self={} app={:?} service={:?} started_filetime={} app_type={} app_status={} terminal_session={} restartable={}",
                        app.process.pid, app.process.pid == self_pid, text(&app.app_name), text(&app.service_name),
                        started, app.app_type, app.app_status, app.terminal_session, app.restartable != 0)
                }).collect();
            }
            // If the process list changes between the two observations, report ERROR_MORE_DATA as-is.
            // It is not proof of no owner and does not trigger another deletion attempt or a sleep.
        }
        let end = session.end();
        format!("restart_manager self_pid={self_pid}; resources={paths:?}; start_status={start}; register_status={registered}; capacity_status={capacity_status}; list_status={list_status:?}; needed={needed}; count={count}; reboot_reasons={reboot}; owners={owners:?}; end_status={end}")
    }
}
