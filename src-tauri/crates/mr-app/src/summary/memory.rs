use std::path::Path;

use super::config::{MIN_SYSTEM_RAM_MB, REQUIRED_FREE_MB};

#[derive(Debug, Clone, Copy)]
pub struct GuardInfo {
    pub threads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// Машина меньше жёсткого минимума: 12 ГиБ total.
    LowSystem { total_mb: u64 },
    /// Свободной памяти меньше `required_free`; sidecar ещё не загружен.
    LowFree { available_mb: u64 },
    /// После загрузки пик превысил бюджет — kill, артефакта нет (SSE oom).
    #[cfg(not(target_os = "android"))]
    Peak { peak_mb: u64 },
}

impl MemoryError {
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::LowSystem { .. } | Self::LowFree { .. } => {
                "Не хватает памяти. Нужно не меньше 12 ГБ ОЗУ и 6,5 ГБ свободно; рекомендуется 16 ГБ."
            }
            #[cfg(not(target_os = "android"))]
            Self::Peak { .. } => {
                "Не хватило памяти для модели итогов. Закройте приложения или выберите «Кратко»."
            }
        }
    }
}

/// `min(8, max(2, logical))` (§6, D11): 4-ядерный tier получает 4 потока.
pub fn threads(logical: usize) -> u32 {
    logical.clamp(2, 8) as u32
}

pub fn logical_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Предзапуск: жёсткий total и свободная память. Если тот же sidecar уже
/// загружен, свободную память не проверяем — иначе второй job на 16 ГиБ
/// получал бы low_memory при живых 5.4 ГиБ в RSS (H09).
pub fn precheck(sidecar_loaded: bool) -> Result<GuardInfo, MemoryError> {
    let cores = logical_cores();
    let total_mb = crate::sysmem::total_bytes().map(|b| b / (1024 * 1024));
    let available_mb = crate::sysmem::available_bytes().map(|b| b / (1024 * 1024));
    if let Some(total) = total_mb {
        if total < MIN_SYSTEM_RAM_MB {
            return Err(MemoryError::LowSystem { total_mb: total });
        }
    }
    if !sidecar_loaded {
        if let Some(available) = available_mb {
            if available < REQUIRED_FREE_MB {
                return Err(MemoryError::LowFree {
                    available_mb: available,
                });
            }
        }
        // Неизвестная память — warning, не отказ (§6).
    }
    Ok(GuardInfo {
        threads: threads(cores),
    })
}

/// Пост-загрузочный пик. Linux — `VmHWM`; платформы без дешёвого чтения
/// возвращают None, и тогда guard не блокирует (лучше, чем ложный отказ).
#[cfg(not(target_os = "android"))]
pub fn peak_mb(pid: u32) -> Option<u64> {
    peak_mb_impl(pid)
}

#[cfg(target_os = "linux")]
fn peak_mb_impl(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_vm_hwm_kb(&text).map(|kb| kb / 1024)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn peak_mb_impl(pid: u32) -> Option<u64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: proc_pid_rusage заполняет структуру известного размера.
    let ok = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
        )
    };
    (ok == 0).then_some(info.ri_phys_footprint / (1024 * 1024))
}

#[cfg(windows)]
fn peak_mb_impl(pid: u32) -> Option<u64> {
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
        (ok != 0).then_some(counters.PeakWorkingSetSize as u64 / (1024 * 1024))
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    windows
)))]
fn peak_mb_impl(_pid: u32) -> Option<u64> {
    None
}

#[cfg(not(target_os = "android"))]
pub fn check_peak(pid: u32) -> Result<(), MemoryError> {
    match peak_mb(pid) {
        Some(peak) if peak > REQUIRED_FREE_MB => Err(MemoryError::Peak { peak_mb: peak }),
        _ => Ok(()),
    }
}

#[cfg(target_os = "linux")]
fn parse_vm_hwm_kb(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

pub fn free_disk_mb(path: &Path) -> Option<u64> {
    fs2::available_space(path).ok().map(|b| b / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_cap_and_floor() {
        assert_eq!(threads(1), 2);
        assert_eq!(threads(4), 4);
        assert_eq!(threads(8), 8);
        assert_eq!(threads(20), 8);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn parses_vm_hwm() {
        let text = "Name:\tllama-server\nVmHWM:\t  5554176 kB\n";
        assert_eq!(parse_vm_hwm_kb(text), Some(5_554_176));
        assert_eq!(parse_vm_hwm_kb("Name:\tx\n"), None);
    }

    #[test]
    fn peak_of_current_process_is_readable_on_linux() {
        #[cfg(target_os = "linux")]
        {
            let peak = peak_mb(std::process::id());
            assert!(
                peak.is_some_and(|mb| mb > 0),
                "VmHWM живого процесса не прочитан"
            );
        }
    }
}
