//! macOS: живой захват микрофона и системного звука через CoreAudio HAL
//! (AudioUnit).
//!
//! Системный звук CoreAudio не отдаёт без виртуального драйвера: если в системе
//! есть BlackHole/Loopback/Soundflower, он опознаётся как отдельный вход и
//! пишется во второй канал. Без такого драйвера запись идёт только с микрофона.
//!
//! CoreAudio — системный C API: отдельные крейты и внешние процессы не нужны.

mod audio;
mod devices;
mod ffi;

pub use audio::start_stream;
pub use devices::devices;
