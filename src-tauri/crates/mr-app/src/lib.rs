//! Meeting Recorder — нативное ядро.
//!
//! Архитектура:
//! * CLI и IPC на порту `127.0.0.1:48291` (одиночный экземпляр)
//! * Встроенный HTTP REST API для Web UI (`ui/web/frontend`)
//! * Локальный ONNX-конвейер расшифровки (Silero VAD, GigaAM, Whisper, ERes2NetV2)
//! * База голосовых отпечатков в формате Voice Packs (`packs/`)
//!
//! Структура: [`config`] — настройки, [`ipc`] — одиночный экземпляр,
//! [`recording`] — запись, [`http`] — REST API, [`transcribe`] — ONNX-пайплайн.

use clap::Parser;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod config;
mod desktop;
mod http;
mod ipc;
mod recording;
#[cfg(feature = "tauri-ui")]
mod tauri_shell;
#[cfg(feature = "onnx")]
mod transcribe;

use config::{app_data_dir, cleanup_stale_files, configured_record_mode};
use http::run_http_server;
use ipc::{handle_ipc_client, send_ipc, IPC_PORT};
use recording::start_recording_with_notify;

#[derive(Debug, Parser)]
#[command(
    name = "meeting-recorder",
    version = "0.1.0",
    about = "Локальный рекордер встреч (Rust-ядро)"
)]
struct Cli {
    #[arg(long, default_value_t = DEFAULT_HTTP_PORT)]
    port: u16,
    #[arg(long)]
    web: bool,
    #[arg(long)]
    browser: bool,
    #[arg(long)]
    window: bool,
    #[arg(long)]
    no_browser: bool,
    #[arg(long)]
    tray: bool,
    #[arg(long)]
    toggle: bool,
    #[arg(long, alias = "record")]
    start: bool,
    #[arg(long)]
    stop: bool,
    #[arg(long)]
    show: bool,
    #[arg(long)]
    hide: bool,
    #[arg(long)]
    quit: bool,
    #[arg(long)]
    autostart_on: bool,
    #[arg(long)]
    autostart_off: bool,
    /// Расшифровать аудиофайл локальным ONNX-конвейером и выйти
    #[arg(long, value_name = "FILE")]
    transcribe: Option<PathBuf>,
    #[arg(long, default_value = "auto")]
    engine: String,
    /// Язык Whisper («ru», «en»…); auto – определить автоматически
    #[arg(long, default_value = "auto")]
    language: String,
    #[arg(long, default_value = "Я")]
    speaker_l: String,
    #[arg(long, default_value = "Собеседник")]
    speaker_r: String,
}

const DEFAULT_HTTP_PORT: u16 = 8765;

fn opener_url(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
/// Мобильная точка входа (Android/iOS): то же HTTP-ядро + окно WebView
/// Tauri, но без CLI/IPC/трея — их на мобильных платформах нет.
/// JNI-обёртку генерирует `tauri::mobile_entry_point`.
#[cfg(feature = "tauri-ui")]
#[cfg_attr(
    any(target_os = "android", target_os = "ios"),
    tauri::mobile_entry_point
)]
pub fn mobile_main() {
    let state = Arc::new(Mutex::new(recording::RecordingState::default()));
    cleanup_stale_files();
    tauri_shell::run_tauri_window(state, DEFAULT_HTTP_PORT, true);
}

/// Полный CLI-вход десктопа (флаги, IPC-команды, автозапуск, окно/браузер).
pub fn run_cli() {
    let cli = Cli::parse();

    // Локальная ONNX-расшифровка из CLI (потоковый вывод JSON-событий в stdout)
    #[cfg(feature = "onnx")]
    if let Some(ref audio) = cli.transcribe {
        let opts = transcribe::TranscribeOptions {
            speaker_left: cli.speaker_l.clone(),
            speaker_right: cli.speaker_r.clone(),
            engine: cli.engine.clone(),
            language: cli.language.to_lowercase(),
            min_speakers: 0,
            max_speakers: 0,
        };
        match transcribe::transcribe_file(audio, &opts, &|pct, msg| {
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
                std::process::exit(0);
            }
            Err(e) => {
                println!("{}", serde_json::json!({"type": "error", "message": e}));
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(feature = "onnx"))]
    if cli.transcribe.is_some() {
        eprintln!("Пересоберите с --features onnx для локальной расшифровки");
        std::process::exit(2);
    }

    // Управление автозапуском (кроссплатформенно, без Tauri)
    if cli.autostart_on {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "meeting-recorder".to_string());
        match desktop::install_autostart(&exe) {
            Ok(()) => println!("Автозапуск включён."),
            Err(e) => eprintln!("Не удалось включить автозапуск: {e}"),
        }
        return;
    }
    if cli.autostart_off {
        match desktop::uninstall_autostart() {
            Ok(true) => println!("Автозапуск выключён."),
            Ok(false) => println!("Автозапуск и так выключён."),
            Err(e) => eprintln!("Не удалось выключить автозапуск: {e}"),
        }
        return;
    }

    let state = Arc::new(Mutex::new(recording::RecordingState::default()));
    #[cfg(unix)]
    recording::shutdown::install(state.clone());
    let mut start_on_launch = false;

    // IPC-команды второму экземпляру
    if cli.toggle {
        if let Some(resp) = send_ipc("TOGGLE") {
            println!("{resp}");
            return;
        }
        // Приложение ещё не запущено: стартуем и сразу начинаем запись.
        start_on_launch = true;
    }
    if cli.start {
        if let Some(resp) = send_ipc("START") {
            println!("{resp}");
            return;
        }
        start_on_launch = true;
    }
    if cli.stop {
        if let Some(resp) = send_ipc("STOP") {
            println!("{resp}");
            return;
        }
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
            println!("{resp}");
            return;
        }
        if cli.quit {
            return;
        }
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
    if bare_launch && send_ipc("SHOW").is_some() {
        return;
    }

    // Одиночный экземпляр: пробуем занять IPC-порт, иначе выходим
    let ipc_listener = TcpListener::bind(format!("127.0.0.1:{IPC_PORT}"));
    let _ipc_guard = match ipc_listener {
        Ok(l) => {
            let st = state.clone();
            std::thread::spawn(move || {
                for stream in l.incoming().flatten() {
                    handle_ipc_client(stream, st.clone(), cli.port);
                }
            });
            // удерживаем listener в фоне через leak (живёт до конца процесса)
            None::<TcpListener>
        }
        Err(_) => {
            // Порт занят не нашим инстансом (send_ipc выше не нашёл живого):
            // продолжаем без IPC — окно и HTTP работают, теряется только
            // управление вторым кликом/--toggle.
            println!("Порт IPC {IPC_PORT} занят посторонним процессом — запускаюсь без IPC.");
            None::<TcpListener>
        }
    };
    let _ = &_ipc_guard;

    // Файловый lock как второй рубеж single-instance
    let lock_path = app_data_dir().join("meeting-recorder.lock");
    let _ = std::fs::create_dir_all(app_data_dir());
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path);
    let _lock = lock_file.and_then(|f| {
        use fs2::FileExt;
        f.try_lock_exclusive()
            .map(|_| f)
            .map_err(std::io::Error::other)
    });
    cleanup_stale_files();

    if start_on_launch {
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        let res = start_recording_with_notify(&mut st);
        if res.get("ok").and_then(|x| x.as_bool()) != Some(true) {
            if let Some(err) = res.get("error").and_then(|x| x.as_str()) {
                eprintln!("{err}");
            }
        }
    }

    println!(
        "Meeting Recorder (Rust-ядро v0.1.0) — HTTP http://127.0.0.1:{}",
        cli.port
    ); // Нативное окно Tauri (только CI-сборка с фичей `tauri-ui` и системным WebView)
    #[cfg(feature = "tauri-ui")]
    if !cli.web && !cli.browser && !cli.no_browser {
        // В трей — без окна; show — с окном; обычный запуск — с окном.
        let visible = cli.show || !cli.tray;
        tauri_shell::run_tauri_window(state.clone(), cli.port, visible);
    }
    // Без фичи `tauri-ui` флаг --window открывает системный браузер на тот же UI
    if cli.window && !cli.no_browser {
        let url = format!("http://127.0.0.1:{}", cli.port);
        let _ = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            opener_url(&url);
        });
    }
    run_http_server(state, cli.port);
}

#[cfg(test)]
use serde_json::json;

#[cfg(test)]
static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use recording::{recording_name, RecordingState};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    #[test]
    fn cli_parses_toggle() {
        let cli = Cli::try_parse_from(["meeting-recorder", "--toggle"]).unwrap();
        assert!(cli.toggle);
    }

    #[test]
    fn recording_name_has_interval_and_date() {
        let start = 1_700_000_000u64;
        let name = recording_name(start, Some(start + 65));
        let started = recording_name(start, None);
        assert!(
            !name.starts_with('[') && !name.contains(']') && name.ends_with(".wav"),
            "{name}"
        );
        assert!(
            name.contains("__") && started.contains("__"),
            "нет разделителя даты"
        );
        assert!(
            name.len() > started.len(),
            "после остановки нет времени окончания: {name}"
        );
        if !cfg!(windows) {
            assert_eq!(
                started.matches(':').count(),
                1,
                "в стартовом имени только ЧЧ:ММ: {started}"
            );
        }
    }

    #[test]
    fn recording_start_stop_roundtrip() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = RecordingState::default();
        // Подменяем save_dir через временную папку окружением MEETING_RECORDER_DATA_DIR
        let tmp = std::env::temp_dir().join(format!("mr-app-{}", std::process::id()));
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &tmp);
        // Фейк-захват: живой pw-record в юнит-тесте не нужен
        std::env::set_var("MR_FAKE_CAPTURE", "1");
        let _ = std::fs::create_dir_all(&tmp);
        let r = recording::start_recording_inner(&mut st, "split");
        assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(st.is_recording);
        let s = recording::stop_recording_inner(&mut st);
        assert_eq!(s.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(!st.is_recording);
        std::env::remove_var("MR_FAKE_CAPTURE");
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ipc_roundtrip_ephemeral() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"PING\n");
            stream.write_all(b"OK\n").unwrap();
        });
        let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        s.write_all(b"PING\n").unwrap();
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"OK\n");
    }

    #[test]
    fn cleanup_stale_files_preserves_active_files() {
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("mr-stale-{}", std::process::id()));
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &tmp);
        let save = crate::config::save_dir();
        let _ = std::fs::create_dir_all(&save);
        let active_file = save.join(".active_take.mic.f32");
        std::fs::write(&active_file, b"test").unwrap();
        cleanup_stale_files();
        assert!(active_file.exists(), "активный файл не должен удаляться");
        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Живой прогон HTTP API на эфемерном порту: регрессия бага `POST /api/upload → 404`
/// и проверка восстановленных после переезда на Rust эндпоинтов.
#[cfg(test)]
mod live_api_tests {
    use super::*;
    use http::{run_http_server, static_body};
    use recording::now_secs;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Минимальный HTTP/1.0-клиент на std: (статус, заголовки, тело).
    fn http(
        port: u16,
        method: &str,
        target: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let mut last_err = String::new();
        for _ in 0..100 {
            match std::net::TcpStream::connect(format!("127.0.0.1:{port}")) {
                Ok(mut s) => {
                    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                    let mut req = format!(
                        "{method} {target} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n"
                    );
                    for (k, v) in headers {
                        req.push_str(&format!("{k}: {v}\r\n"));
                    }
                    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
                    s.write_all(req.as_bytes()).unwrap();
                    s.write_all(body).unwrap();
                    let mut raw = Vec::new();
                    s.read_to_end(&mut raw).unwrap();
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let (head, bstart) = text
                        .find("\r\n\r\n")
                        .map(|i| (&text[..i], i + 4))
                        .unwrap_or((text.as_str(), text.len()));
                    let mut lines = head.lines();
                    let status = lines
                        .next()
                        .unwrap_or("")
                        .split_whitespace()
                        .nth(1)
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(0);
                    let hs = lines
                        .filter_map(|l| {
                            l.split_once(':')
                                .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
                        })
                        .collect();
                    return (status, hs, raw[bstart..].to_vec());
                }
                Err(e) => {
                    last_err = e.to_string();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        panic!("сервер не поднялся на порту {port}: {last_err}");
    }

    fn json(status_body: (u16, Vec<(String, String)>, Vec<u8>)) -> (u16, serde_json::Value) {
        let (st, _, body) = status_body;
        (
            st,
            serde_json::from_slice(&body).expect("тело должно быть JSON"),
        )
    }

    #[test]
    fn static_fallback_outside_repo() {
        // Регрессия «Not Found» в нативном окне: запуск не из корня репо обязан
        // отдать вшитую статику, а не 404.
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(std::env::temp_dir()).unwrap();
        let (kind, body) =
            static_body("index.html").expect("fallback index.html обязан существовать");
        assert!(kind.starts_with("text/html"));
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains(r#"id="app""#), "вшитый index.html устарел?");
        let (_, css) =
            static_body("css/app.css").expect("fallback css/app.css обязан существовать");
        assert!(String::from_utf8_lossy(&css).contains("--bg-canvas"));
        let (kind, js) = static_body("js/app.js").expect("fallback js/app.js обязан существовать");
        assert!(kind.starts_with("application/javascript"));
        assert!(String::from_utf8_lossy(&js).contains("transcript-reload"));
        let (kind, fav) =
            static_body("favicon.svg").expect("fallback favicon.svg обязан существовать");
        assert_eq!(kind, "image/svg+xml");
        assert!(String::from_utf8_lossy(&fav).contains("<svg"));
        let (kind, icn) = static_body("icon.png").expect("fallback icon.png обязан существовать");
        assert_eq!(kind, "image/png");
        assert!(!icn.is_empty());
        assert!(static_body("nope.txt").is_none());
        assert!(static_body("../main.rs").is_none());
        std::env::set_current_dir(prev).unwrap();
    }

    #[test]
    fn restored_endpoints_roundtrip() {
        let _guard = super::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base =
            std::env::temp_dir().join(format!("mr-live-{}-{}", std::process::id(), now_secs()));
        let rec = base.join("rec");
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &base);
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(
            base.join("config.json"),
            format!(r#"{{"save_dir":"{}"}}"#, rec.to_string_lossy()),
        )
        .unwrap();

        let port = free_port();
        let state = Arc::new(Mutex::new(recording::RecordingState::default()));
        std::thread::spawn(move || run_http_server(state, port));

        // Статика нового UI: Svelte-бандл с тем же контрактом (маркеры в бандле).
        let (st, _, body) = http(port, "GET", "/", &[], &[]);
        assert_eq!(st, 200);
        let html = String::from_utf8_lossy(&body).into_owned();
        assert!(html.contains(r#"id="app""#), "index.html не Svelte-шелл");
        for asset in ["css/app.css", "js/app.js"] {
            let (st, _, body) = http(port, "GET", &format!("/{asset}"), &[], &[]);
            assert_eq!(st, 200, "статика {asset} не отдалась");
            assert!(!body.is_empty());
        }
        let js_body = http(port, "GET", "/js/app.js", &[], &[]);
        let js = String::from_utf8_lossy(&js_body.2);
        assert!(
            js.contains("transcript-grid"),
            "нет двухколоночной расшифровки"
        );
        assert!(js.contains("packsListSettings"), "нет меню настроек");
        let (st, _, css) = http(port, "GET", "/css/app.css", &[], &[]);
        assert_eq!(st, 200);
        assert!(String::from_utf8_lossy(&css).contains("--bg-canvas"));

        // Регрессия: загрузка файла (было 404 + SyntaxError в WebView).
        // Настоящий WAV через mr-audio + имя с кириллицей и пробелом.
        let wav_src = rec.join("src.wav");
        mr_audio::write_stereo_wav(&wav_src, &vec![0.1; 16000], &vec![0.1; 16000], 16000).unwrap();
        let wav_bytes = std::fs::read(&wav_src).unwrap();
        let fname_enc = "%D1%82%D0%B5%D1%81%D1%82%20%D0%B7%D0%B2%D1%83%D0%BA.mp3";
        let (st, v) = json(http(
            port,
            "POST",
            "/api/upload",
            &[("X-Filename", fname_enc), ("Content-Type", "audio/mpeg")],
            &wav_bytes,
        ));
        assert_eq!(st, 200, "upload вернул {st}: {v}");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        let saved_name = "тест звук.mp3";
        assert!(rec.join(saved_name).exists(), "файл не сохранён на диск");

        // Список записей: поля метаданных (name/path/size_mb/mtime/date/…).
        let (st, v) = json(http(port, "GET", "/api/recordings", &[], &[]));
        assert_eq!(st, 200);
        let items = v
            .get("recordings")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let item = items
            .iter()
            .find(|x| x.get("name").and_then(|n| n.as_str()) == Some(saved_name))
            .expect("записи нет в списке");
        assert!(
            item.get("date")
                .and_then(|x| x.as_str())
                .map(|s| s.len() == 19)
                .unwrap_or(false),
            "нет поля date: {item}"
        );
        assert_eq!(
            item.get("has_transcript").and_then(|x| x.as_bool()),
            Some(false)
        );
        assert!(
            item.get("duration_sec")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0)
                > 0.5,
            "нет длительности: {item}"
        );

        // Раздача медиа + Range (нужно для сниппетов `#t=` и плеера).
        let audio_target = format!("/audio/{}", fname_enc);
        let (st, hs, body) = http(port, "GET", &audio_target, &[], &[]);
        assert_eq!(st, 200, "GET /audio вернул {st}");
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-type" && v.starts_with("audio/")),
            "нет audio content-type: {hs:?}"
        );
        assert_eq!(body.len(), wav_bytes.len());
        let (st, hs, part) = http(port, "GET", &audio_target, &[("Range", "bytes=0-9")], &[]);
        assert_eq!(st, 206, "Range не поддержан");
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-range" && v.starts_with("bytes 0-9/")),
            "нет Content-Range: {hs:?}"
        );
        assert_eq!(part.len(), 10);
        let (st, _, _) = http(port, "GET", "/audio/nope.mp3", &[], &[]);
        assert!(st == 404, "чужой файл должен давать 404, а не {st}");

        // Переименование записи: расширение сохраняется, старый файл исчезает.
        let rename_body = json!({"path": item.get("path").and_then(|x| x.as_str()).unwrap_or(""), "name": "Совещание с командой"});
        let (st, v) = json(http(
            port,
            "POST",
            "/api/recordings/rename",
            &[("Content-Type", "application/json")],
            &serde_json::to_vec(&rename_body).unwrap(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "rename: {v}"
        );
        assert!(rec.join("Совещание с командой.mp3").exists());
        assert!(!rec.join(saved_name).exists());

        // Режим записи хранится в config.json.
        let (st, v) = json(http(
            port,
            "POST",
            "/api/record/mode",
            &[("Content-Type", "application/json")],
            json!({"mode":"mix"}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let (_, v) = json(http(port, "GET", "/api/record/mode", &[], &[]));
        assert_eq!(v.get("mode").and_then(|x| x.as_str()), Some("mix"));

        // Авто-расшифровка: по умолчанию включена, настройка сохраняется.
        let (_, v) = json(http(port, "GET", "/api/transcribe/auto", &[], &[]));
        assert_eq!(v.get("enabled").and_then(|x| x.as_bool()), Some(true));
        let (st, v) = json(http(
            port,
            "POST",
            "/api/transcribe/auto",
            &[("Content-Type", "application/json")],
            json!({"enabled":false}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let (_, v) = json(http(port, "GET", "/api/transcribe/auto", &[], &[]));
        assert_eq!(v.get("enabled").and_then(|x| x.as_bool()), Some(false));
        let (_, v) = json(http(port, "GET", "/api/status", &[], &[]));
        assert_eq!(v.get("transcribing").and_then(|x| x.as_bool()), Some(false));

        // Чужая вкладка не должна управлять записью (CSRF): Origin не локальный — 403.
        let (st, _, _) = http(
            port,
            "GET",
            "/api/status",
            &[("Origin", "https://evil.example")],
            &[],
        );
        assert_eq!(st, 403);
        let origin = format!("http://127.0.0.1:{port}");
        let (st, _, _) = http(port, "GET", "/api/status", &[("Origin", &origin)], &[]);
        assert_eq!(st, 200);

        // Гости.
        let (st, v) = json(http(
            port,
            "POST",
            "/api/people/add_external",
            &[("Content-Type", "application/json")],
            json!({"name":"Тестовый Гость","role":"Клиент"})
                .to_string()
                .as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let (st, v) = json(http(port, "GET", "/api/people", &[], &[]));
        assert_eq!(st, 200);
        assert!(v
            .get("people")
            .and_then(|x| x.as_array())
            .map(|a| a
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("Тестовый Гость")))
            .unwrap_or(false));

        // Участника можно переименовать и убрать (не только гостя).
        let upd = json!({"old_name":"Тестовый Гость", "name":"Гость Обновлён", "role":"Партнёр"});
        let (st, v) = json(http(
            port,
            "POST",
            "/api/people/update",
            &[("Content-Type", "application/json")],
            &serde_json::to_vec(&upd).unwrap(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "people/update: {v}"
        );
        let (_, v) = json(http(port, "GET", "/api/people", &[], &[]));
        let updated = v.get("people").and_then(|x| x.as_array()).and_then(|a| {
            a.iter()
                .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("Гость Обновлён"))
        });
        assert_eq!(
            updated.and_then(|p| p.get("role")).and_then(|x| x.as_str()),
            Some("Партнёр")
        );
        let (st, v) = json(http(
            port,
            "POST",
            "/api/people/remove",
            &[("Content-Type", "application/json")],
            json!({"name":"Гость Обновлён"}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "people/remove: {v}"
        );
        let (_, v) = json(http(port, "GET", "/api/people", &[], &[]));
        assert!(!v
            .get("people")
            .and_then(|x| x.as_array())
            .map(|a| a
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("Гость Обновлён")))
            .unwrap_or(false));

        // Голосовые отпечатки: save → видны в people/voiceprints → delete.
        let emb: Vec<f64> = (0..32).map(|i| (i as f64) / 32.0).collect();
        let save_body = serde_json::json!({"name": "Тестовый Голос", "embedding": emb, "audio_file": "", "old_name": ""});
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voiceprints/save",
            &[("Content-Type", "application/json")],
            &serde_json::to_vec(&save_body).unwrap(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let (_, v) = json(http(port, "GET", "/api/voiceprints", &[], &[]));
        assert!(v
            .get("voiceprints")
            .and_then(|x| x.as_array())
            .map(|a| a
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("Тестовый Голос")))
            .unwrap_or(false));
        let (_, v) = json(http(port, "GET", "/api/people", &[], &[]));
        assert!(v
            .get("voiceprints")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().any(|n| n.as_str() == Some("Тестовый Голос")))
            .unwrap_or(false));
        assert!(v
            .get("people")
            .and_then(|x| x.as_array())
            .map(|a| a
                .iter()
                .any(|p| p.get("name").and_then(|n| n.as_str()) == Some("Тестовый Голос")))
            .unwrap_or(false));
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voiceprints/delete",
            &[("Content-Type", "application/json")],
            json!({"name":"Тестовый Голос"}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );

        // Пакеты и поддержка.
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voicepacks/create",
            &[("Content-Type", "application/json")],
            json!({"name":"Пакет Тест"}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let pack_id = v
            .get("pack")
            .and_then(|p| p.get("id"))
            .and_then(|x| x.as_str())
            .unwrap_or("default")
            .to_string();
        let tog = serde_json::json!({"pack_id": pack_id, "enabled": false});
        let (st, _) = json(http(
            port,
            "POST",
            "/api/voicepacks/toggle",
            &[("Content-Type", "application/json")],
            &serde_json::to_vec(&tog).unwrap(),
        ));
        assert_eq!(st, 200);
        let (st, _) = json(http(
            port,
            "POST",
            "/api/support/dismiss",
            &[("Content-Type", "application/json")],
            b"{}",
        ));
        assert_eq!(st, 200);
        let (_, v) = json(http(port, "GET", "/api/support/info", &[], &[]));
        assert_eq!(
            v.get("stats")
                .and_then(|s| s.get("donate_prompt_dismissed"))
                .and_then(|x| x.as_bool()),
            Some(true)
        );

        // Видео принимают в расшифровку (декодер: symphonia → ffmpeg-извлечение).
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok()
        {
            let clip = base.join("clip_src.mkv");
            let gen = std::process::Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=black:s=64x64:d=1:r=5",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "aac",
                    "-shortest",
                    &clip.to_string_lossy(),
                ])
                .status();
            if gen.map(|s| s.success()).unwrap_or(false) {
                let bytes = std::fs::read(&clip).unwrap();
                let (st, v) = json(http(
                    port,
                    "POST",
                    "/api/upload",
                    &[
                        (
                            "X-Filename",
                            "%D0%BA%D0%BB%D0%B8%D0%BF%20%D1%82%D0%B5%D1%81%D1%82.mkv",
                        ),
                        ("Content-Type", "video/x-matroska"),
                    ],
                    &bytes,
                ));
                assert_eq!(
                    v.get("ok").and_then(|x| x.as_bool()),
                    Some(true),
                    "upload mkv: {st} {v}"
                );
                let vpath = v
                    .get("path")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                assert!(rec.join("клип тест.mkv").exists());
                let start_body = serde_json::json!({"file": vpath, "engine": "auto"});
                let (st, v) = json(http(
                    port,
                    "POST",
                    "/api/transcribe/start",
                    &[("Content-Type", "application/json")],
                    &serde_json::to_vec(&start_body).unwrap(),
                ));
                assert_eq!(
                    v.get("ok").and_then(|x| x.as_bool()),
                    Some(true),
                    "transcribe/start для видео: {st} {v}"
                );
            }
        }

        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }
}
