#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn available_bytes() -> Option<u64> {
    let host = parse_mem_available(&std::fs::read_to_string("/proc/meminfo").ok()?);
    // Android и контейнеры живут в cgroup: /proc/meminfo показывает память
    // хоста, и preflight пропускает файл, который потом убивает LMK.
    match (host, cgroup_available()) {
        (Some(host), Some(cgroup)) => Some(host.min(cgroup)),
        (host, cgroup) => host.or(cgroup),
    }
}

/// Свободно по лимиту cgroup v2 (`memory.max` − `memory.current`).
#[cfg(any(target_os = "linux", target_os = "android"))]
fn cgroup_available() -> Option<u64> {
    let read = |name: &str| -> Option<u64> {
        let text = std::fs::read_to_string(format!("/sys/fs/cgroup/{name}")).ok()?;
        text.trim().parse::<u64>().ok()
    };
    let max = read("memory.max")?;
    let current = read("memory.current")?;
    Some(max.saturating_sub(current))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn parse_mem_available(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn total_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn available_bytes() -> Option<u64> {
    // `_SC_AVPHYS_PAGES` на macOS считает только совсем свободные страницы:
    // после работы любой программы он показывает единицы гигабайт, и часовая
    // запись не проходит проверку. Берём free + inactive + speculative из
    // vm_statistics64: это то, что система реально может отдать без свопа.
    // SAFETY: mach-вызовы заполняют структуру известного размера; указатели валидны.
    unsafe {
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let page_size = libc::sysconf(libc::_SC_PAGESIZE);
        if page_size <= 0 {
            return None;
        }
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let status = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut libc::vm_statistics64 as *mut libc::integer_t,
            &mut count,
        );
        if status != 0 {
            return None;
        }
        let pages =
            stats.free_count as u64 + stats.inactive_count as u64 + stats.speculative_count as u64;
        Some(pages * page_size as u64)
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn total_bytes() -> Option<u64> {
    let mut total: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    // SAFETY: sysctlbyname пишет ровно u64 по валидному указателю и длине.
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut total as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok == 0 && len == std::mem::size_of::<u64>()).then_some(total)
}

#[cfg(windows)]
pub(crate) fn available_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: структура целиком заполняется системным вызовом, размер передан.
    unsafe {
        let mut st: MEMORYSTATUSEX = std::mem::zeroed();
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut st) != 0).then_some(st.ullAvailPhys)
    }
}

#[cfg(windows)]
pub(crate) fn total_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: структура целиком заполняется системным вызовом, размер передан.
    unsafe {
        let mut st: MEMORYSTATUSEX = std::mem::zeroed();
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut st) != 0).then_some(st.ullTotalPhys)
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
pub(crate) fn available_bytes() -> Option<u64> {
    None
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
pub(crate) fn total_bytes() -> Option<u64> {
    None
}

/// Пиковый RSS процесса в мегабайтах. None — платформа не умеет дешёвое
/// чтение пика; ложное число в perf-поле хуже пустого.
pub(crate) fn peak_mb(pid: u32) -> Option<f64> {
    peak_mb_impl(pid)
}

#[cfg(target_os = "linux")]
fn peak_mb_impl(pid: u32) -> Option<f64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_vm_hwm_kb(&text).map(|kb| kb as f64 / 1024.0)
}

#[cfg(target_os = "macos")]
fn peak_mb_impl(pid: u32) -> Option<f64> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: proc_pid_rusage заполняет структуру известного размера.
    let ok = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        )
    };
    // ri_lifetime_max_phys_footprint — максимум за жизнь процесса, не снимок.
    (ok == 0).then_some(info.ri_lifetime_max_phys_footprint as f64 / (1024.0 * 1024.0))
}

#[cfg(windows)]
fn peak_mb_impl(pid: u32) -> Option<f64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: handle закрывается, структура заполняется системным вызовом.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = GetProcessMemoryInfo(handle, &mut counters, counters.cb);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        (ok != 0).then_some(counters.PeakWorkingSetSize as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn peak_mb_impl(_pid: u32) -> Option<f64> {
    None
}

#[cfg(target_os = "linux")]
fn parse_vm_hwm_kb(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
#[cfg(any(target_os = "linux", target_os = "android"))]
mod tests {
    use super::*;

    #[test]
    fn cgroup_limit_is_read_when_present() {
        // На хосте без cgroup v2 проверка не падает: значения просто нет.
        if let Some(bytes) = cgroup_available() {
            assert!(bytes > 0, "лимит cgroup должен быть положительным");
        }
    }

    #[test]
    fn parses_mem_available_from_proc() {
        let text = "MemTotal:       32000000 kB\nMemAvailable:    6600000 kB\n";
        assert_eq!(parse_mem_available(text), Some(6_600_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_vm_hwm() {
        let text = "Name:\tpodtekst\nVmHWM:\t  5554176 kB\n";
        assert_eq!(parse_vm_hwm_kb(text), Some(5_554_176));
        assert_eq!(parse_vm_hwm_kb("Name:\tx\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn peak_of_current_process_is_readable() {
        let peak = peak_mb(std::process::id());
        assert!(
            peak.is_some_and(|mb| mb > 0.0),
            "VmHWM живого процесса не прочитан"
        );
    }
}
