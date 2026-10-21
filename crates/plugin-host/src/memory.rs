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

/// Asks the system allocator to hand the memory it keeps for reuse back to
/// the system, so [`process_memory`] counts only memory in use. The
/// allocators on macOS, Linux and Windows keep large freed blocks to reuse.
pub fn release_free_memory() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone asks every zone for relief, and a goal of 0
        // releases all it can; the call frees only memory not in use.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: malloc_trim frees only memory not in use.
        unsafe {
            libc::malloc_trim(0);
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};
        // SAFETY: the process heap is valid for the process's lifetime, and
        // compacting it frees only memory not in use.
        unsafe {
            HeapCompact(GetProcessHeap(), 0);
        }
    }
}
