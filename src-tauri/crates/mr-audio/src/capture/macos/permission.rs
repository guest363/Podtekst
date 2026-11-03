//! Разрешение на запись экрана — без запроса: первый
//! `SCShareableContent::get()` показывает системный диалог сам.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub fn screen_recording_granted() -> bool {
    // SAFETY: вызов CoreGraphics без аргументов и без побочных эффектов.
    unsafe { CGPreflightScreenCaptureAccess() }
}
