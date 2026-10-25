//! macOS: живой захват CoreAudio пока не подключён.
//!
//! Импорт готовых файлов и расшифровка работают; запись с микрофона и
//! системного звука вернёт понятную ошибку вместо паники при сборке.

use std::path::Path;

use super::{AudioError, CaptureStream, DeviceCatalog, Result, StreamKind};

/// Устройств нет: без CoreAudio нечего показывать в списке.
pub fn devices() -> DeviceCatalog {
    DeviceCatalog {
        platform: "darwin".to_string(),
        ..Default::default()
    }
}

/// Поток не запускается, пока нет бэкенда CoreAudio.
pub fn start_stream(
    _kind: StreamKind,
    _device: Option<&str>,
    _path: &Path,
    _rate: u32,
) -> Result<CaptureStream> {
    Err(AudioError::Other(
        "живой захват на macOS ещё не реализован: импортируйте готовый аудиофайл".to_string(),
    ))
}
