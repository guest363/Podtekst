//! Оболочка Tauri v2 (компилируется только с фичей `tauri-ui`).
//!
//! Архитектура повторяет бывший pywebview-режим: нативное окно WebView открывает
//! HTTP-ядро того же процесса (`http://127.0.0.1:<port>`), поэтому Web UI
//! работает без изменений (относительные `/api/*`). Глобальная горячая клавиша
//! `Ctrl+Shift+R` (`Cmd+Shift+R` на macOS) переключает запись через общее
//! состояние `RecordingState`. Клик по трею показывает окно.
//!
//! Мобильные сборки (Android/iOS): окно + HTTP-ядро. Трей, меню, глобальные
//! клавиши, автостарт, апдейтер и single-instance — десктоп-эксклюзивы Tauri,
//! на мобильных таргетах этих API нет, поэтому они за cfg-гейтами.
//!
//! Требуется системный WebView (CI ставит зависимости):
//! * Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev ...`
//! * macOS: WKWebView из коробки; Windows: WebView2 из коробки.

#[cfg(feature = "tauri-ui")]
use std::sync::{Arc, Mutex};

// Десктоп-эксклюзив Tauri: меню есть только на desktop-таргетах.
#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
use tauri::menu::{Menu, MenuItem};
#[cfg(feature = "tauri-ui")]
use tauri::Manager;

#[cfg(feature = "tauri-ui")]
use crate::recording::RecordingState;
// Переключение записи по хоткею/меню трея — десктоп-путь.
#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
use crate::recording::toggle_recording_with_notify;

/// Геометрия главного окна: ширина равна контенту (`.page` max-width 860px
/// + отступы body 32px + запас на скроллбар).
#[cfg(feature = "tauri-ui")]
pub(crate) const WINDOW_W: f64 = 920.0;
/// Высота главного окна.
#[cfg(feature = "tauri-ui")]
pub(crate) const WINDOW_H: f64 = 860.0;
/// Минимальная ширина окна: ниже 480px ломается мобильная вёрстка.
#[cfg(feature = "tauri-ui")]
pub(crate) const WINDOW_MIN_W: f64 = 480.0;
/// Минимальная высота окна.
#[cfg(feature = "tauri-ui")]
pub(crate) const WINDOW_MIN_H: f64 = 560.0;

#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
fn toggle_recording(state: &Arc<Mutex<RecordingState>>) {
    let mut guard = state.lock().unwrap();
    toggle_recording_with_notify(&mut guard);
}

/// X11/macOS/Windows: глобальная комбинация через Tauri-плагин.
#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
fn register_tauri_shortcut(app: &tauri::AppHandle<tauri::Wry>) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    app.global_shortcut()
        .on_shortcut("CommandOrControl+Shift+R", |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                if let Some(st) = app.try_state::<Arc<Mutex<RecordingState>>>() {
                    toggle_recording(st.inner());
                }
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(all(feature = "tauri-ui", target_os = "linux"))]
fn is_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(all(feature = "tauri-ui", target_os = "linux"))]
fn is_gnome() -> bool {
    ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION"]
        .iter()
        .any(|key| {
            std::env::var(key)
                .map(|v| v.to_lowercase().contains("gnome"))
                .unwrap_or(false)
        })
}

#[cfg(all(feature = "tauri-ui", target_os = "linux"))]
fn gsettings(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// GNOME/Wayland: X11-grab не видит клавиши вне фокуса, а портал
/// GlobalShortcuts в текущем GNOME зависает без parent window.
/// Поэтому заводим обычную системную комбинацию, которую видно в настройках GNOME.
#[cfg(all(feature = "tauri-ui", target_os = "linux"))]
fn setup_gnome_shortcut() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/meeting-recorder/";
    let base = "org.gnome.settings-daemon.plugins.media-keys";

    // Сначала регистрируем путь в списке: GNOME иначе может сбросить ключи.
    // Список перезаписываем при каждом старте одним и тем же значением:
    // запись в dconf дёргает watcher mutter, и биндинг, не подхваченный
    // в прошлой сессии, оживает без перелогина.
    let current = gsettings(&["get", base, "custom-keybindings"])?;
    let existed = current.contains(path);
    if !existed {
        let inner = current
            .trim()
            .trim_start_matches("@as ")
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim();
        let next = if inner.is_empty() {
            format!("['{path}']")
        } else {
            format!("[{inner}, '{path}']")
        };
        gsettings(&["set", base, "custom-keybindings", &next])?;
    } else {
        gsettings(&[
            "set",
            base,
            "custom-keybindings",
            &current.replace('\n', ""),
        ])?;
    }

    let schema = format!("org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:{path}");
    // gsettings ждёт GVariant-строку: кавычки вокруг всей команды обязательны.
    let command = format!("'{} --tray --toggle'", exe.display());
    gsettings(&["set", &schema, "name", "Запись встреч – старт/стоп"])?;
    gsettings(&["set", &schema, "command", &command])?;
    // Комбинацию задаём только при первом создании: ручную настройку не затираем.
    let binding = gsettings(&["get", &schema, "binding"]).unwrap_or_default();
    if !existed || binding.trim_matches('\'').is_empty() {
        gsettings(&["set", &schema, "binding", "<Control><Shift>r"])?;
    }
    eprintln!("глобальная клавиша GNOME: Ctrl+Shift+R");
    Ok(())
}

#[cfg(all(
    feature = "tauri-ui",
    not(any(target_os = "android", target_os = "ios"))
))]
fn setup_global_shortcut(app: &tauri::AppHandle<tauri::Wry>) {
    #[cfg(target_os = "linux")]
    if is_wayland() && is_gnome() {
        if let Err(e) = setup_gnome_shortcut() {
            eprintln!("глобальная клавиша GNOME: {e}");
            if let Err(e) = register_tauri_shortcut(app) {
                eprintln!("глобальная клавиша: резервная регистрация не удалась: {e}");
            }
        }
        return;
    }
    if let Err(e) = register_tauri_shortcut(app) {
        eprintln!("глобальная клавиша: {e}");
    }
}

/// Запускает HTTP-ядро в фоне и открывает нативное окно Tauri. Не возвращается.
#[cfg(feature = "tauri-ui")]
pub fn run_tauri_window(state: Arc<Mutex<RecordingState>>, http_port: u16, visible: bool) -> ! {
    // 1. HTTP-ядро (тот же REST, что и в headless-режиме)
    let st = state.clone();
    std::thread::spawn(move || {
        crate::run_http_server(st, http_port);
    });
    // Ждём поднятия порта, чтобы окно не открылось на пустой странице
    for _ in 0..50 {
        if std::net::TcpStream::connect(format!("127.0.0.1:{http_port}")).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // 2. Нативное окно поверх HTTP-ядра + плагины
    let url = format!("http://127.0.0.1:{http_port}");
    let builder = tauri::Builder::default().manage(state.clone());
    // Десктоп-плагины: на мобильных их API не существует.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(
            |app: &tauri::AppHandle<tauri::Wry>, args: Vec<String>, _cwd: String| {
                let show = !args.iter().any(|a| a == "--hide" || a == "--tray");
                if show {
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.unminimize();
                        let _ = win.set_focus();
                    }
                }
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        // Сначала без комбинации: на Wayland её регистрирует портал,
        // на X11/macOS/Windows — плагин. Иначе на Wayland был бы двойной тумблер.
        .plugin(tauri_plugin_global_shortcut::Builder::new().build());
    builder
        .on_window_event(|win, event| {
            // Закрытие окна = свернуть, а не выйти (десктоп: выход — через меню
            // трея; на мобильных система сама прибивает процесс).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = win.hide();
                api.prevent_close();
            }
        })
        .setup(move |app| {
            // Мобильные платформы: каталог данных даёт система (файлы приложения).
            // Прописываем его ДО первого обращения конфига — save_dir, модели
            // и паки читают app_data_dir лениво через env.
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if let Ok(dir) = app.path().app_data_dir() {
                std::env::set_var("MEETING_RECORDER_DATA_DIR", &dir);
            }

            // Трей с меню — десктоп-эксклюзив (открыть окно / старт-стоп / выйти).
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let open = MenuItem::with_id(app, "open", "Открыть окно", true, None::<&str>)?;
                let toggle =
                    MenuItem::with_id(app, "toggle", "Старт / Стоп записи", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&open, &toggle, &quit])?;
                let app_handle = app.handle().clone();
                let icon_idle = tauri::include_image!("icons/32x32.png");
                let icon_rec = tauri::include_image!("icons/recording-32x32.png");
                if let Some(tray) = app.tray_by_id("main") {
                    let _ = tray.set_tooltip(Some("Запись встреч"));
                    let _ = tray.set_icon_as_template(false);
                    tray.set_menu(Some(menu))?;
                }
                let initial_rec = app
                    .try_state::<Arc<Mutex<RecordingState>>>()
                    .map(|s| s.lock().map(|g| g.is_recording).unwrap_or(false))
                    .unwrap_or(false);
                if initial_rec {
                    if let Some(tray) = app.tray_by_id("main") {
                        let _ = tray.set_icon_as_template(false);
                        let _ = tray.set_icon(Some(icon_rec.clone()));
                        let _ = tray.set_tooltip(Some("Запись встреч (идёт запись…)"));
                    }
                }
                crate::recording::set_recording_state_callback(Box::new(move |recording| {
                    let handle = app_handle.clone();
                    let icon = if recording {
                        icon_rec.clone()
                    } else {
                        icon_idle.clone()
                    };
                    let tooltip = if recording {
                        "Запись встреч (идёт запись…)"
                    } else {
                        "Запись встреч"
                    };
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if let Some(tray) = h.tray_by_id("main") {
                            let _ = tray.set_icon_as_template(false);
                            let _ = tray.set_icon(Some(icon));
                            let _ = tray.set_tooltip(Some(tooltip));
                        }
                    });
                }));
                app.on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                    "toggle" => {
                        if let Some(st) = app.try_state::<Arc<Mutex<RecordingState>>>() {
                            toggle_recording(st.inner());
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
                // Клик по иконке трея возвращает окно (или переключает видимость).
                if let Some(tray) = app.tray_by_id("main") {
                    tray.on_tray_icon_event(|tray, event| {
                        if matches!(
                            event,
                            tauri::tray::TrayIconEvent::Click {
                                button: tauri::tray::MouseButton::Left,
                                button_state: tauri::tray::MouseButtonState::Up,
                                ..
                            } | tauri::tray::TrayIconEvent::DoubleClick {
                                button: tauri::tray::MouseButton::Left,
                                ..
                            }
                        ) {
                            if let Some(w) = tray.app_handle().get_webview_window("main") {
                                let is_visible = w.is_visible().unwrap_or(false);
                                let is_minimized = w.is_minimized().unwrap_or(false);
                                if is_visible && !is_minimized {
                                    let _ = w.hide();
                                } else {
                                    let _ = w.show();
                                    let _ = w.unminimize();
                                    let _ = w.set_focus();
                                }
                            }
                        }
                    });
                }
                setup_global_shortcut(app.handle());
            }

            // IPC из ядра (--show/--hide/второй клик по иконке) управляет окном.
            let ipc_handle = app.handle().clone();
            crate::ipc::set_ui_ipc_callback(Box::new(move |cmd| {
                let handle = ipc_handle.clone();
                match cmd {
                    "SHOW" | "HIDE" => {
                        let cmd = cmd.to_string();
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || {
                            if let Some(win) = h.get_webview_window("main") {
                                if cmd == "SHOW" {
                                    let _ = win.show();
                                    // unminimize — десктоп-API, на мобильных окна не сворачивают.
                                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                                    let _ = win.unminimize();
                                    let _ = win.set_focus();
                                } else {
                                    let _ = win.hide();
                                }
                            }
                        });
                    }
                    "QUIT" => {
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || h.exit(0));
                    }
                    _ => {}
                }
            }));
            // url = format!("http://127.0.0.1:{port}") — парсинг не может упасть.
            let u = url.parse().unwrap_or_else(|_| {
                "http://127.0.0.1:8765"
                    .parse()
                    .expect("константный URL валиден")
            });
            if let Err(e) =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(u))
                    .title("Запись встреч")
                    .inner_size(WINDOW_W, WINDOW_H)
                    .min_inner_size(WINDOW_MIN_W, WINDOW_MIN_H)
                    .visible(visible)
                    // target="_blank" (донат, внешние ссылки) открываем в системном
                    // браузере: WebView2 сам новое окно не создаёт.
                    .on_new_window(|url, _| {
                        if matches!(url.scheme(), "http" | "https") {
                            crate::opener_url(url.as_str());
                        }
                        tauri::webview::NewWindowResponse::Deny
                    })
                    // Живой VU-метр до старта записи использует getUserMedia.
                    .on_permission_request(|_, kind| match kind {
                        tauri::webview::PermissionKind::Microphone => {
                            tauri::webview::PermissionResponse::Allow
                        }
                        _ => tauri::webview::PermissionResponse::Default,
                    })
                    .build()
            {
                eprintln!("не удалось создать окно: {e}");
            }
            // Android: без foreground service микрофон глушится при погасшем
            // экране. Регистрируем JNI-мост заранее — им пользуются запись и
            // расшифровка из фоновых потоков.
            #[cfg(target_os = "android")]
            if let Some(win) = app.get_webview_window("main") {
                let (tx, rx) = std::sync::mpsc::channel();
                if win
                    .with_webview(move |webview| {
                        let _ = tx.send(webview.jni_handle());
                    })
                    .is_ok()
                {
                    if let Ok(handle) = rx.recv_timeout(std::time::Duration::from_secs(2)) {
                        mr_audio::capture::set_foreground_handler(Box::new(
                            move |processing, start| {
                                handle.exec(move |env, activity, _| {
                                    let method = if !start {
                                        "stopRecordingService"
                                    } else if processing {
                                        "startProcessingService"
                                    } else {
                                        "startRecordingService"
                                    };
                                    let _ = env.call_method(activity, method, "()V", &[]);
                                });
                            },
                        ));
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("ошибка запуска Tauri-окна: {e}");
            std::process::exit(1);
        });
    std::process::exit(0);
}
