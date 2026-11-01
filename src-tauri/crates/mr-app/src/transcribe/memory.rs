//! Предохранитель по памяти для длинных записей.
//!
//! Пайплайн держит всю запись в памяти как f32 16 кГц по каналам, поэтому
//! предел задаётся не часами, а оценкой пикового RSS против доступной памяти.
//! Оценка считается до декодирования — по заголовку файла.

use std::path::Path;

/// Постоянная часть пика: модели (GigaAM/Whisper/эмбеддер), арены ONNX и
/// служебные буферы. Замер на часовой стереозаписи — около 1.2 ГиБ.
const BASE_BYTES: u64 = 1280 * 1024 * 1024;
/// Разрешаем занять не больше 80% доступной памяти.
const SHARE: f64 = 0.8;
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub(crate) fn peak_bytes(duration_sec: f64, channels: usize) -> u64 {
    let samples = duration_sec.max(0.0) * 16_000.0 * 4.0 * channels.clamp(1, 2) as f64;
    BASE_BYTES + samples as u64
}

pub(crate) fn fits(need: u64, available: u64) -> bool {
    need as f64 <= available as f64 * SHARE
}

/// Проверка файла до декодирования. Если длительность, каналы или объём
/// доступной памяти неизвестны — не мешаем: сработает только физический предел.
pub(crate) fn check(path: &Path) -> Result<(), String> {
    let Some((duration, channels)) = mr_audio::decode::media_info(path) else {
        return Ok(());
    };
    check_peak(peak_bytes(duration, channels), available_bytes())
}

fn check_peak(need: u64, available: Option<u64>) -> Result<(), String> {
    let Some(available) = available else {
        return Ok(());
    };
    if fits(need, available) {
        return Ok(());
    }
    Err(format!(
        "Для записи нужно ~{:.1} ГБ памяти, свободно ~{:.1} ГБ. \
         Закройте приложения или разбейте файл на части.",
        need as f64 / GIB,
        available as f64 / GIB
    ))
}

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
mod tests {
    use super::*;

    #[test]
    fn peak_counts_two_channels_only() {
        let mono = peak_bytes(3600.0, 1);
        let stereo = peak_bytes(3600.0, 2);
        assert_eq!((stereo - mono) as f64, 3600.0 * 16_000.0 * 4.0);
        assert_eq!(peak_bytes(3600.0, 8), stereo);
        assert_eq!(peak_bytes(0.0, 2), BASE_BYTES);
    }

    #[test]
    fn refuses_only_when_available_memory_is_short() {
        assert!(fits(79, 100));
        assert!(!fits(81, 100));
        assert!(fits(peak_bytes(3600.0, 2), 6 * 1024 * 1024 * 1024));
        assert!(!fits(peak_bytes(10.0 * 3600.0, 2), 6 * 1024 * 1024 * 1024));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn parses_mem_available_from_proc() {
        let text = "MemTotal:       32000000 kB\nMemAvailable:    6600000 kB\n";
        assert_eq!(parse_mem_available(text), Some(6_600_000 * 1024));
        assert_eq!(parse_mem_available("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn refusal_names_needed_and_available() {
        let gib = 1024 * 1024 * 1024;
        let err = check_peak(6 * gib, Some(4 * gib)).unwrap_err();
        assert!(err.contains("6.0 ГБ") && err.contains("4.0 ГБ"), "{err}");
        assert!(check_peak(gib, Some(8 * gib)).is_ok());
        assert!(check_peak(u64::MAX, None).is_ok());
    }
}
