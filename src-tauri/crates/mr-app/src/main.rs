//! Тонкий CLI-вход: вся логика ядра живёт в lib-крейте `mr_app_lib`
//! (модули компилируются один раз; мобильная точка входа — там же).

/// Windows: запуск из Проводника создаёт отдельное окно консоли.
/// Скрываем его, только если в консоли нет других процессов (из cmd/bash не трогаем).
#[cfg(windows)]
fn hide_own_console() {
    use std::ffi::c_void;

    #[allow(non_snake_case)]
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> *mut c_void;
        fn GetConsoleProcessList(process_list: *mut u32, process_count: u32) -> u32;
    }
    #[allow(non_snake_case)]
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: *mut c_void, cmd_show: i32) -> i32;
    }
    const SW_HIDE: i32 = 0;

    // SAFETY: вызовы Win32 с валидными буферами; окно принадлежит текущему процессу.
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            return;
        }
        let mut pids = [0u32; 2];
        if GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) <= 1 {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn main() {
    // CLI-вывод (--transcribe/--help/--version) не скрываем: он нужен в терминале.
    #[cfg(windows)]
    let cli_output = std::env::args().any(|a| {
        matches!(
            a.as_str(),
            "--transcribe" | "--help" | "-h" | "--version" | "-V"
        )
    });
    #[cfg(windows)]
    if !cli_output {
        hide_own_console();
    }
    mr_app_lib::run_cli();
}
