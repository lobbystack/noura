//! How much memory this process holds, as the plugin runtime's measurements
//! and the collaboration sandbox's limit tests count it.

/// The process's physical footprint, the number Activity Monitor shows.
#[cfg(target_os = "macos")]
pub fn process_memory() -> Option<u64> {
    // SAFETY: `rusage_info_v4` is plain data, and `proc_pid_rusage` writes at
    // most one of them for the `RUSAGE_INFO_V4` flavor.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let result = libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            (&raw mut info).cast::<libc::rusage_info_t>(),
        );
        (result == 0).then_some(info.ri_phys_footprint)
    }
}

/// The process's resident set size.
#[cfg(target_os = "linux")]
pub fn process_memory() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    Some(resident_pages * u64::try_from(page_size).ok()?)
}

/// The process's private bytes, the commit charge Task Manager shows.
#[cfg(windows)]
pub fn process_memory() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the counters are plain data of the size `cb` states, and the
    // pseudo handle of the current process needs no closing.
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    (ok != 0).then_some(counters.PrivateUsage as u64)
}

/// No measurement on other platforms.
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn process_memory() -> Option<u64> {
    None
}
