//! Windows: живой захват WASAPI пока не подключён.
//!
//! Импорт готовых файлов и расшифровка работают; запись с микрофона и
//! системного звука вернёт понятную ошибку вместо паники при сборке.

use std::path::Path;

use super::{AudioError, CaptureStream, DeviceCatalog, Result, StreamKind};

/// Устройств нет: без WASAPI нечего показывать в списке.
pub fn devices() -> DeviceCatalog {
    DeviceCatalog {
        platform: "win32".to_string(),
        ..Default::default()
    }
}

/// Поток не запускается, пока нет бэкенда WASAPI.
pub fn start_stream(
    _kind: StreamKind,
    _device: Option<&str>,
    _path: &Path,
    _rate: u32,
) -> Result<CaptureStream> {
    Err(AudioError::Other(
        "живой захват на Windows ещё не реализован: импортируйте готовый аудиофайл".to_string(),
    ))
}
