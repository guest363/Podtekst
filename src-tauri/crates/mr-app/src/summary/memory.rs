use std::path::Path;

use super::config;

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
/// получал бы low_memory при живых 5.4 ГиБ в RSS.
pub fn precheck(sidecar_loaded: bool) -> Result<GuardInfo, MemoryError> {
    let cores = logical_cores();
    let total_mb = crate::sysmem::total_bytes().map(|b| b / (1024 * 1024));
    let available_mb = crate::sysmem::available_bytes().map(|b| b / (1024 * 1024));
    if let Some(total) = total_mb {
        if total < config::min_system_ram_mb() {
            return Err(MemoryError::LowSystem { total_mb: total });
        }
    }
    if !sidecar_loaded {
        if let Some(available) = available_mb {
            if available < config::required_free_mb() {
                return Err(MemoryError::LowFree {
                    available_mb: available,
                });
            }
        }
        // Память неизвестна — отказывать не за что, поэтому только warning.
    }
    Ok(GuardInfo {
        threads: threads(cores),
    })
}

/// Пост-загрузочный пик. Платформы без дешёвого чтения возвращают None,
/// и тогда guard не блокирует (лучше, чем ложный отказ).
#[cfg(not(target_os = "android"))]
pub fn check_peak(pid: u32) -> Result<(), MemoryError> {
    match crate::sysmem::peak_mb(pid).map(|peak| peak.round() as u64) {
        Some(peak) if peak > config::required_free_mb() => Err(MemoryError::Peak { peak_mb: peak }),
        _ => Ok(()),
    }
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
}
