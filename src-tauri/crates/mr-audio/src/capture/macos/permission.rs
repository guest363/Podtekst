//! Разрешение на запись экрана — без запроса: первый
//! `SCShareableContent::get()` показывает системный диалог сам.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    // Apple Boolean — unsigned char, не C99 _Bool.
    fn CGPreflightScreenCaptureAccess() -> u8;
}

pub fn screen_recording_granted() -> bool {
    // SAFETY: вызов CoreGraphics без аргументов и без побочных эффектов.
    unsafe { CGPreflightScreenCaptureAccess() != 0 }
}
