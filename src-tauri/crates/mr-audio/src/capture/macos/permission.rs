use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use screencapturekit::prelude::SCShareableContent;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    // Apple Boolean — unsigned char, не C99 _Bool.
    fn CGPreflightScreenCaptureAccess() -> u8;
}

const UNKNOWN: u8 = 0;
const GRANTED: u8 = 1;
const DENIED: u8 = 2;

fn sck_granted() -> bool {
    static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);
    static PROBING: AtomicBool = AtomicBool::new(false);

    match STATE.load(Ordering::Acquire) {
        GRANTED => return true,
        DENIED => return false,
        _ => {}
    }
    // Ограничение macOS: системный диалог TCC блокирует вызов,
    // предотвращаем параллельные запросы доступа.
    if PROBING.swap(true, Ordering::AcqRel) {
        return false;
    }
    let granted = SCShareableContent::get().is_ok();
    STATE.store(if granted { GRANTED } else { DENIED }, Ordering::Release);
    PROBING.store(false, Ordering::Release);
    granted
}

pub fn screen_recording_granted() -> bool {
    // SAFETY: вызов CoreGraphics без аргументов и без побочных эффектов.
    if unsafe { CGPreflightScreenCaptureAccess() != 0 } {
        return true;
    }
    // macOS 26 иногда отвечает false при выданном доступе. Авторитетна только
    // реальная проверка ScreenCaptureKit — тем же API, которым идёт захват.
    // Ответ кэшируем на процесс: выдача доступа всё равно требует перезапуска.
    sck_granted()
}
