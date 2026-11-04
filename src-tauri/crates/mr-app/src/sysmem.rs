#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn available_bytes() -> Option<u64> {
    parse_mem_available(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn parse_mem_available(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
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

#[cfg(test)]
#[cfg(any(target_os = "linux", target_os = "android"))]
mod tests {
    use super::*;

    #[test]
    fn parses_mem_available_from_proc() {
        let text = "MemTotal:       32000000 kB\nMemAvailable:    6600000 kB\n";
        assert_eq!(parse_mem_available(text), Some(6_600_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }
}
