//! macOS: живой захват микрофона и системного звука.
//!
//! Микрофон идёт через CoreAudio HAL (AudioUnit). Системный звук — через
//! ScreenCaptureKit, если драйвер BlackHole не выбран явно: SCK не требует
//! виртуального аудиодрайвера. Выбор `sck` в списке устройств означает SCK,
//! любой другой id — конкретный вход CoreAudio (BlackHole/Loopback).

use std::path::Path;

use crate::capture::{CaptureStream, Result, StreamKind};

mod audio;
mod devices;
mod ffi;
mod permission;
mod sck;

pub use devices::devices;
pub use permission::screen_recording_granted;

pub fn start_stream(
    kind: StreamKind,
    device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    match kind {
        StreamKind::Microphone => audio::start_stream(kind, device, path, rate),
        StreamKind::System => {
            let native = device.is_none_or(|id| id.is_empty() || id == devices::NATIVE_SYSTEM_ID);
            if native {
                sck::start_stream(path, rate)
            } else {
                audio::start_stream(kind, device, path, rate)
            }
        }
    }
}
