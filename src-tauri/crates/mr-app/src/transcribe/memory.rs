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
    let need = peak_bytes(duration, channels);
    let available = available_bytes();
    log::debug!(
        target: "run",
        avail_mb = available.map(|bytes| bytes / (1024 * 1024)),
        need_mb = need / (1024 * 1024),
        dur_sec = duration,
        channels = channels,
        decision = if available.is_some_and(|available| fits(need, available)) {
            "ok"
        } else if available.is_some() {
            "refuse"
        } else {
            "unknown"
        };
        "ev=memory.check"
    );
    check_peak(need, available)
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

pub(crate) fn available_bytes() -> Option<u64> {
    crate::sysmem::available_bytes()
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

    #[test]
    fn refusal_names_needed_and_available() {
        let gib = 1024 * 1024 * 1024;
        let err = check_peak(6 * gib, Some(4 * gib)).unwrap_err();
        assert!(err.contains("6.0 ГБ") && err.contains("4.0 ГБ"), "{err}");
        assert!(check_peak(gib, Some(8 * gib)).is_ok());
        assert!(check_peak(u64::MAX, None).is_ok());
    }
}
