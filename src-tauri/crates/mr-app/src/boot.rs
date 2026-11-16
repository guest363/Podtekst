use clap::Parser;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use crate::cli::Cli;
use crate::config::{app_data_dir, cleanup_stale_files};
use crate::http::run_http_server;
use crate::ipc::{handle_ipc_client, send_ipc, IPC_PORT};
use crate::recording::start_recording_with_notify;
use crate::{crash, desktop, diagnostics, ipc, logging, recording, summary, system};

/// Быстрый выход теряет хвост dev-лога, если не сбросить очередь writer-потока.
pub(crate) fn exit_now(code: i32) -> ! {
    logging::flush();
    std::process::exit(code)
}

fn print_ipc(resp: &str) {
    println!("{resp}");
    if resp.starts_with("ERROR:") {
        exit_now(1);
    }
}

pub(crate) fn opener_url(url: &str) {
    #[cfg(target_os = "windows")]
    {
        // explorer.exe — GUI-процесс без консоли; URL уходит одним аргументом,
        // без cmd.exe и переразбора метасимволов (&, ^, |) из ссылки.
        let _ = std::process::Command::new("explorer").arg(url).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    // Android: системный intent VIEW. xdg-open в APK нет.
    #[cfg(target_os = "android")]
    let _ = std::process::Command::new("/system/bin/am")
        .args(["start", "-a", "android.intent.action.VIEW", "-d", url])
        .spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "android")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
#[cfg(feature = "tauri-ui")]
#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    tauri::mobile_entry_point
)]
pub fn mobile_main() {
    logging::init();
    logging::claim_owner();
    crash::install_hook();
    let state = Arc::new(Mutex::new(recording::RecordingState::default()));
    cleanup_stale_files();
    summary::init();
    ipc::expect_ui();
    let code = crate::tauri_shell::run_tauri_window(state, crate::cli::DEFAULT_HTTP_PORT, true);
    summary::shutdown();
    logging::shutdown();
    exit_now(code);
}

#[doc(hidden)]
pub fn spawn_test_http_server(port: u16) {
    summary::init();
    let state = Arc::new(Mutex::new(recording::RecordingState::default()));
    std::thread::spawn(move || {
        run_http_server(state, port);
    });
}

#[doc(hidden)]
pub fn shutdown_summary_for_test() {
    summary::shutdown();
}

#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn set_transcribing_for_test(running: bool) {
    crate::transcribe::events::set_running_for_test(running);
}

// TCC и реальный SCK иначе проверяются только руками на живом Mac.
#[cfg(target_os = "macos")]
fn macos_selftest() -> i32 {
    let dir = std::env::temp_dir().join(format!("podtekst-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::note!("Не создана временная папка: {e}");
        return 1;
    }
    let system_path = dir.join("sys.f32");
    let request = mr_audio::capture::CaptureRequest {
        rate: 16000,
        mic: None,
        system: Some("sck".into()),
        mic_path: dir.join("mic.f32"),
        system_path: system_path.clone(),
    };
    let mut session = match mr_audio::capture::start_capture(&request) {
        Ok(session) => session,
        Err(e) => {
            crate::note!("Захват не стартовал: {e}");
            let _ = std::fs::remove_dir_all(&dir);
            return 1;
        }
    };
    std::thread::sleep(std::time::Duration::from_secs(5));
    session.stop();
    let samples = mr_audio::capture::read_raw_f32(&system_path).unwrap_or_default();
    let rms = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    };
    println!("samples={} rms={rms:.4}", samples.len());
    let _ = std::fs::remove_dir_all(&dir);
    if samples.is_empty() {
        1
    } else {
        0
    }
}

#[cfg(not(target_os = "macos"))]
fn macos_selftest() -> i32 {
    crate::note!("--macos-selftest: только macOS");
    2
}

pub fn run_cli() {
    let cli = Cli::parse();
    logging::init();
    crash::install_hook();
    if let Err(e) = cli.validate_speakers() {
        crate::note!("{e}");
        exit_now(2);
    }
    if cli.diagnostics {
        match diagnostics::collect(cli.with_dev_logs) {
            Ok(path) => println!("{}", path.display()),
            Err(e) => {
                crate::note!("Отчёт диагностики не собрался: {e}");
                exit_now(1);
            }
        }
        logging::flush();
        return;
    }
    if cli.macos_selftest {
        exit_now(macos_selftest());
    }

    // Повторная регистрация в tauri-сетапе: на мобильных env папки данных
    // известен только после инициализации Tauri.
    mr_audio::tools::set_extra_bin_dir(crate::config::app_data_dir().join("bin"));

    // Локальная ONNX-расшифровка из CLI (потоковый вывод JSON-событий в stdout)
    #[cfg(feature = "onnx")]
    if let Some(ref audio) = cli.transcribe {
        let opts = cli.transcribe_options();
        match crate::transcribe::transcribe_file(audio, &opts, &|pct, msg| {
            println!(
                "{}",
                serde_json::json!({"type": "stage", "pct": pct, "message": msg})
            );
        }) {
            Ok(rep) => {
                println!(
                    "{}",
                    serde_json::json!({"type": "done", "code": 0, "file": audio.to_string_lossy(),
                        "md": rep.md_path.to_string_lossy(), "txt": rep.txt_path.to_string_lossy(),
                        "json": rep.json_path.to_string_lossy(), "utterances": rep.utterances,
                        "speakers": rep.speakers, "words": rep.words, "engine": rep.engine,
                        "speech_sec": rep.speech_sec, "duration_sec": rep.duration_sec,
                        "speaker_method": rep.speaker_method})
                );
                exit_now(0);
            }
            Err(e) => {
                println!("{}", serde_json::json!({"type": "error", "message": e}));
                exit_now(1);
            }
        }
    }
    #[cfg(not(feature = "onnx"))]
    if cli.transcribe.is_some() {
        crate::note!("Пересоберите с --features onnx для локальной расшифровки");
        exit_now(2);
    }

    if cli.autostart_on {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "podtekst".to_string());
        match desktop::install_autostart(&exe) {
            Ok(()) => println!("Автозапуск включён."),
            Err(e) => crate::note!("Не удалось включить автозапуск: {e}"),
        }
        return;
    }
    if cli.autostart_off {
        match desktop::uninstall_autostart() {
            Ok(true) => println!("Автозапуск выключён."),
            Ok(false) => println!("Автозапуск и так выключён."),
            Err(e) => crate::note!("Не удалось выключить автозапуск: {e}"),
        }
        return;
    }

    let state = Arc::new(Mutex::new(recording::RecordingState::default()));
    #[cfg(unix)]
    recording::shutdown::install(state.clone());
    let mut start_on_launch = false;

    if cli.toggle {
        if let Some(resp) = send_ipc("TOGGLE") {
            print_ipc(&resp);
            return;
        }
        start_on_launch = true;
    }
    if cli.start {
        if let Some(resp) = send_ipc("START") {
            print_ipc(&resp);
            return;
        }
        start_on_launch = true;
    }
    if cli.stop {
        if let Some(resp) = send_ipc("STOP") {
            print_ipc(&resp);
            return;
        }
        crate::note!("Не удалось остановить запись: запущенное приложение не найдено.");
        exit_now(1);
    }
    if cli.show || cli.hide || cli.quit {
        let cmd = if cli.show {
            "SHOW"
        } else if cli.hide {
            "HIDE"
        } else {
            "QUIT"
        };
        if let Some(resp) = send_ipc(cmd) {
            print_ipc(&resp);
            return;
        }
        if cli.quit {
            return;
        }
    }

    #[cfg(not(feature = "tauri-ui"))]
    if !cli.web
        && !cli.browser
        && !cli.window
        && !cli.no_browser
        && !cli.toggle
        && !cli.start
        && !cli.stop
    {
        crate::note!("Сборка без нативного окна (tauri-ui). Запустите ./run.sh --build");
        exit_now(1);
    }

    // Повторный клик по иконке (запуск без флагов) поднимает уже работающее окно,
    // а не молча выходит из-за занятого IPC.
    let bare_launch = !cli.toggle
        && !cli.start
        && !cli.stop
        && !cli.show
        && !cli.hide
        && !cli.quit
        && !cli.tray
        && !cli.web
        && !cli.browser;
    if bare_launch {
        match send_ipc("SHOW").as_deref() {
            Some("OK") => return,
            Some(resp) => {
                crate::note!(
                    "Подтекст уже запущен без окна ({resp}). Завершите фоновый экземпляр и запустите снова."
                );
                exit_now(1);
            }
            None => {}
        }
    }

    let ipc_listener = TcpListener::bind(format!("127.0.0.1:{IPC_PORT}"));
    match ipc_listener {
        Ok(l) => {
            ipc::mark_active();
            let st = state.clone();
            std::thread::spawn(move || {
                // Каждый клиент — в своём потоке: молчащий сокет не задерживает
                // остальные команды управления записью.
                for stream in l.incoming().flatten() {
                    let st = st.clone();
                    std::thread::spawn(move || handle_ipc_client(stream, st));
                }
            });
        }
        Err(_) => {
            // Порт занят не нашим инстансом (send_ipc выше не нашёл живого):
            // продолжаем без IPC — окно и HTTP работают, теряется только
            // управление вторым кликом/--toggle.
            println!("Порт IPC {IPC_PORT} занят посторонним процессом — запускаюсь без IPC.");
        }
    }

    // Файловый lock как второй рубеж single-instance
    let lock_path = app_data_dir().join("podtekst.lock");
    let _ = std::fs::create_dir_all(app_data_dir());
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path);
    let _lock = match lock_file.and_then(|f| {
        use fs2::FileExt;
        f.try_lock_exclusive()
            .map(|_| f)
            .map_err(std::io::Error::other)
    }) {
        Ok(f) => f,
        Err(e) => {
            crate::note!(
                "Подтекст уже запущен (lock {}): {e}",
                crate::logging::file_token(&lock_path)
            );
            exit_now(1);
        }
    };
    // Ротацию app.log делает только владелец: клиент `--toggle` её не трогает.
    logging::claim_owner();
    cleanup_stale_files();
    system::cleanup_stale();
    summary::init();
    crash::begin_run();

    if start_on_launch {
        let res = start_recording_with_notify(&state);
        if res.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            if let Some(err) = res.get("error").and_then(|x| x.as_str()) {
                crate::note!("{err}");
            }
        }
    }

    println!(
        "Подтекст (Rust-ядро v{}) — HTTP http://127.0.0.1:{}",
        env!("CARGO_PKG_VERSION"),
        cli.port
    );
    #[cfg(feature = "tauri-ui")]
    if !cli.web && !cli.browser && !cli.no_browser {
        let visible = cli.show || !cli.tray;
        ipc::expect_ui();
        let code = crate::tauri_shell::run_tauri_window(state.clone(), cli.port, visible);
        summary::shutdown();
        logging::shutdown();
        if code != 0 {
            exit_now(code);
        }
        crash::mark_clean();
        return;
    }
    // Без фичи `tauri-ui` флаг --window открывает системный браузер на тот же UI
    if cli.window && !cli.no_browser {
        let url = format!("http://127.0.0.1:{}", cli.port);
        let _ = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            opener_url(&url);
        });
    }
    let served = run_http_server(state, cli.port);
    logging::shutdown();
    crash::mark_clean();
    if !served {
        exit_now(1);
    }
}
