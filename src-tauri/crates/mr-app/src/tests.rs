use crate::cli::{Cli, CtcMode};
use crate::config::cleanup_stale_files;
use crate::http;
use crate::recording;
use clap::Parser;
use serde_json::json;
use std::net::TcpListener;

pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// Имя `tests` сохраняет прежние пути и имена тестов после выноса из `lib.rs`.
#[allow(clippy::module_inception)]
mod tests {
    use super::*;
    use recording::{recording_name, RecordingState};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    #[test]
    fn cli_parses_toggle() {
        let cli = Cli::try_parse_from(["podtekst", "--toggle"]).unwrap();
        assert!(cli.toggle);
    }

    #[test]
    fn cli_speakers_default_to_auto() {
        let cli = Cli::try_parse_from(["podtekst"]).unwrap();
        assert_eq!((cli.min_speakers, cli.max_speakers), (0, 0));
        assert!(cli.validate_speakers().is_ok());
    }

    #[test]
    fn cli_speakers_accept_bounds() {
        let fixed = Cli::try_parse_from([
            "podtekst",
            "--transcribe",
            "a.wav",
            "--min-speakers",
            "2",
            "--max-speakers",
            "2",
        ])
        .unwrap();
        assert_eq!((fixed.min_speakers, fixed.max_speakers), (2, 2));
        assert!(fixed.validate_speakers().is_ok());

        let range = Cli::try_parse_from([
            "podtekst",
            "--transcribe",
            "a.wav",
            "--min-speakers",
            "2",
            "--max-speakers",
            "5",
        ])
        .unwrap();
        assert!(range.validate_speakers().is_ok());

        let lower_only =
            Cli::try_parse_from(["podtekst", "--transcribe", "a.wav", "--min-speakers", "3"])
                .unwrap();
        assert!(lower_only.validate_speakers().is_ok());
    }

    #[test]
    fn cli_rejects_min_above_max() {
        let cli = Cli::try_parse_from([
            "podtekst",
            "--transcribe",
            "a.wav",
            "--min-speakers",
            "5",
            "--max-speakers",
            "2",
        ])
        .unwrap();
        let err = cli.validate_speakers().unwrap_err();
        assert!(err.contains("--min-speakers"), "{err}");
        assert!(err.contains("--max-speakers"), "{err}");
    }

    #[test]
    fn cli_ctc_defaults_to_auto_and_follows_settings() {
        let cli = Cli::try_parse_from(["podtekst"]).unwrap();
        assert_eq!(cli.ctc, CtcMode::Auto);
        assert!(CtcMode::Auto.allows(true));
        assert!(!CtcMode::Auto.allows(false), "auto — текущее поведение");
        assert!(!CtcMode::Off.allows(true));
        assert!(!CtcMode::Off.allows(false));
        assert!(CtcMode::On.allows(true));
        assert!(CtcMode::On.allows(false));
    }

    #[test]
    fn cli_ctc_modes_reach_transcribe_options() {
        let cli_off = Cli::try_parse_from([
            "podtekst",
            "--transcribe",
            "a.wav",
            "--engine",
            "gigaam",
            "--language",
            "RU",
            "--ctc",
            "off",
        ])
        .unwrap();
        let opts_off = cli_off.transcribe_options();
        assert!(!opts_off.ctc_enabled);
        assert_eq!(opts_off.engine, "gigaam");
        assert_eq!(opts_off.language, "ru");

        let cli_on = Cli::try_parse_from([
            "podtekst",
            "--transcribe",
            "a.wav",
            "--engine",
            "gigaam",
            "--language",
            "RU",
            "--ctc",
            "on",
        ])
        .unwrap();
        let opts_on = cli_on.transcribe_options();
        assert!(opts_on.ctc_enabled);
    }

    #[test]
    fn cli_ctc_rejects_unknown_value() {
        for value in ["maybe", "1", "true"] {
            assert!(
                Cli::try_parse_from(["podtekst", "--ctc", value]).is_err(),
                "--ctc {value} должен быть отвергнут"
            );
        }
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
        let tmp = std::env::temp_dir().join(format!("mr-app-{}", std::process::id()));
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &tmp);
        std::env::set_var("MR_FAKE_CAPTURE", "1");
        let _ = std::fs::create_dir_all(&tmp);
        let cfg = serde_json::json!({"save_dir": tmp.join("Recordings")});
        std::fs::write(tmp.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
        let r = recording::start_recording_inner(&mut st, "split", false);
        assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(st.is_recording);
        let s = recording::stop_recording_inner(&mut st, false);
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
        let _ = std::fs::create_dir_all(&tmp);
        let cfg = serde_json::json!({"save_dir": tmp.join("Recordings")});
        std::fs::write(tmp.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
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
                    let mut req = format!("{method} {target} HTTP/1.0\r\n");
                    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
                        req.push_str("Host: 127.0.0.1\r\n");
                    }
                    req.push_str("Connection: close\r\n");
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
        // Запуск не из корня репо обязан отдавать вшитую статику, а не 404.
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
        let cfg = serde_json::json!({"save_dir": rec.to_string_lossy()});
        std::fs::write(base.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();

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
        // Ищем классы, которые реально попадают в собранный бандл: у picker это
        // `summary-picker`, а не имя компонента.
        for marker in ["summary-block", "summary-picker", "summary-result"] {
            assert!(js.contains(marker), "нет summary-маркера {marker}");
        }
        assert!(js.contains("packsListSettings"), "нет меню настроек");
        let (st, _, css) = http(port, "GET", "/css/app.css", &[], &[]);
        assert_eq!(st, 200);
        assert!(String::from_utf8_lossy(&css).contains("--bg-canvas"));

        // Настоящий WAV через mr-audio + имя с кириллицей и пробелом.
        let wav_src = rec.join("src.wav");
        mr_audio::write_stereo_wav(&wav_src, &vec![0.1; 16000], &vec![0.1; 16000], 16000).unwrap();
        let wav_bytes = std::fs::read(&wav_src).unwrap();
        let fname_enc = "%D1%82%D0%B5%D1%81%D1%82%20%D0%B7%D0%B2%D1%83%D0%BA.wav";
        let (st, v) = json(http(
            port,
            "POST",
            "/api/upload",
            &[("X-Filename", fname_enc), ("Content-Type", "audio/wav")],
            &wav_bytes,
        ));
        assert_eq!(st, 200, "upload вернул {st}: {v}");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        let saved_name = "тест звук.wav";
        assert!(rec.join(saved_name).exists(), "файл не сохранён на диск");

        let (st, v) = json(http(
            port,
            "GET",
            &format!("/api/transcript/metrics?file={fname_enc}"),
            &[],
            &[],
        ));
        assert_eq!(st, 404, "без расшифровки метрики должны давать 404");
        assert!(v.get("error").is_some(), "нет текста ошибки: {v}");

        let transcript = serde_json::json!({
            "schema": 1,
            "meta": {
                "audio": saved_name,
                "speakers_info": [
                    {"id": "spk-A", "name": "Спикер A", "matched_name": "Я"},
                    {"id": "spk-B", "name": "Собеседник", "matched_name": null}
                ]
            },
            "utterances": [
                {"start": 0.0, "end": 0.5, "speaker": "Спикер A", "channel": 0, "text": "раз два"},
                {"start": 0.6, "end": 1.0, "speaker": "Собеседник", "channel": 1, "text": "да"}
            ]
        });
        std::fs::write(
            rec.join("тест звук.transcript.json"),
            serde_json::to_vec(&transcript).unwrap(),
        )
        .unwrap();
        let (st, v) = json(http(
            port,
            "GET",
            &format!("/api/transcript/metrics?file={fname_enc}"),
            &[],
            &[],
        ));
        assert_eq!(st, 200, "метрики не посчитались: {v}");
        assert_eq!(v.get("schema").and_then(|x| x.as_u64()), Some(1));
        assert!(v.get("totals").is_some(), "нет итогов по встрече: {v}");
        let speakers = v
            .get("speakers")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(speakers.len(), 2, "нет метрик по спикерам: {v}");
        assert!(
            speakers.iter().any(|speaker| speaker["name"] == "Я"),
            "имя спикера не разрешилось: {v}"
        );
        let share: f64 = speakers
            .iter()
            .filter_map(|s| s.get("share").and_then(|x| x.as_f64()))
            .sum();
        assert!((share - 1.0).abs() < 0.01, "доли не сходятся: {share}");

        let (st, v) = json(http(port, "DELETE", "/api/metrics/baseline", &[], &[]));
        assert_eq!(st, 200, "сброс базовой линии не сработал: {v}");

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
        assert_eq!(
            item.get("has_summary").and_then(|x| x.as_bool()),
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
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-length" && v == &wav_bytes.len().to_string()),
            "нет Content-Length в 200-ответе: {hs:?}"
        );
        assert!(
            !hs.iter()
                .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked")),
            "200-ответ для медиа не должен быть chunked: {hs:?}"
        );

        // HEAD-запрос для предварительной проверки размера и поддержки Range (WebKitGTK/GStreamer).
        let (st, hs, head_body) = http(port, "HEAD", &audio_target, &[], &[]);
        assert_eq!(st, 200, "HEAD /audio вернул {st}");
        assert_eq!(head_body.len(), 0, "HEAD не должен возвращать тело");
        assert!(
            hs.iter().any(|(k, v)| k == "accept-ranges" && v == "bytes"),
            "HEAD обязан сообщать Accept-Ranges: bytes"
        );
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-length" && v == &wav_bytes.len().to_string()),
            "HEAD обязан содержать Content-Length: {hs:?}"
        );

        let (st, hs, part) = http(port, "GET", &audio_target, &[("Range", "bytes=0-9")], &[]);
        assert_eq!(st, 206, "Range не поддержан");
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-range" && v.starts_with("bytes 0-9/")),
            "нет Content-Range: {hs:?}"
        );
        assert_eq!(part.len(), 10);

        let (st, hs, tail) = http(port, "GET", &audio_target, &[("Range", "bytes=-10")], &[]);
        assert_eq!(st, 206, "Суффиксный Range не вернул 206");
        assert_eq!(tail.len(), 10);
        let exp_start = wav_bytes.len() - 10;
        let exp_end = wav_bytes.len() - 1;
        assert!(
            hs.iter().any(|(k, v)| k == "content-range"
                && v == &format!("bytes {exp_start}-{exp_end}/{}", wav_bytes.len())),
            "неверный Content-Range для суффикса: {hs:?}"
        );

        let (st, hs, _) = http(
            port,
            "GET",
            &audio_target,
            &[("Range", "bytes=999999-")],
            &[],
        );
        assert_eq!(st, 416, "Неудовлетворимый диапазон должен давать 416");
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-range" && v == &format!("bytes */{}", wav_bytes.len())),
            "416 обязан содержать Content-Range: bytes */size: {hs:?}"
        );

        let (st, _, _) = http(port, "GET", "/audio/nope.mp3", &[], &[]);
        assert!(st == 404, "чужой файл должен давать 404, а не {st}");

        // Фрагмент спикера из stereo WAV: выбранный канал отдаётся моно,
        // чтобы в наушниках не звучало эхо второго канала.
        let snippet = format!("/api/media/snippet?file={fname_enc}&start=0&end=1&channel=0");
        let (st, hs, wav) = http(port, "GET", &snippet, &[], &[]);
        assert_eq!(st, 200, "snippet канала: {st}");
        assert!(
            hs.iter()
                .any(|(k, v)| k == "content-type" && v == "audio/wav"),
            "нет WAV content-type: {hs:?}"
        );
        assert_eq!(
            u16::from_le_bytes([wav[22], wav[23]]),
            1,
            "фрагмент должен быть моно"
        );

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
        assert!(rec.join("Совещание с командой.wav").exists());
        assert!(!rec.join(saved_name).exists());

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
        assert_eq!(
            v.get("explicit").and_then(|x| x.as_bool()),
            Some(true),
            "ручной выбор режима должен сохраняться"
        );

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
        assert_eq!(
            v.get("version").and_then(|x| x.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(v.get("arch").and_then(|x| x.as_str()).is_some());
        assert!(v.get("self_update").and_then(|x| x.as_bool()).is_some());
        assert_eq!(v["features"]["summary"].as_bool(), Some(true));
        assert!(v
            .get("summary_supported")
            .and_then(|x| x.as_bool())
            .is_some());
        assert_eq!(v["summarizing"].as_bool(), Some(false));

        // Summary: каталог и настройки живут без модели и runtime, а удалённые
        // маршруты не должны вернуться под новыми именами.
        let (st, v) = json(http(port, "GET", "/api/summary/presets", &[], &[]));
        assert_eq!(st, 200, "summary presets: {v}");
        assert_eq!(v["presets"].as_array().map(Vec::len), Some(7));
        let (st, v) = json(http(port, "GET", "/api/summary/settings", &[], &[]));
        assert_eq!(st, 200, "summary settings: {v}");
        assert_eq!(v["capabilities"]["classifier"].as_str(), Some("heuristic"));
        let (st, v) = json(http(port, "GET", "/api/summary", &[], &[]));
        assert_eq!((st, v["code"].as_str()), (400, Some("no_file")));
        let (st, v) = json(http(port, "GET", "/api/summary?file=nope.wav", &[], &[]));
        assert_eq!((st, v["code"].as_str()), (404, Some("no_transcript")));
        for (method, path) in [
            ("GET", "/api/summary/status"),
            ("GET", "/api/summary_stream"),
            ("POST", "/api/summary/edit"),
        ] {
            let (st, _, _) = http(port, method, path, &[], b"{}");
            assert_eq!(st, 404, "{method} {path} должен быть 404");
        }
        let too_big = vec![b'{'; 65 * 1024 + 1];
        let (st, v) = json(http(
            port,
            "POST",
            "/api/summary/classify",
            &[("Content-Type", "application/json")],
            &too_big,
        ));
        assert_eq!((st, v["code"].as_str()), (413, Some("body_too_large")));

        let (st, v) = json(http(port, "GET", "/api/transcribe/queue", &[], &[]));
        assert_eq!(st, 200);
        assert_eq!(v["running"].as_bool(), Some(false));
        assert!(v["queue"].as_array().is_some());
        let (st, v) = json(http(
            port,
            "POST",
            "/api/transcribe/queue/remove",
            &[("Content-Type", "application/json")],
            json!({"file":"nope.wav"}).to_string().as_bytes(),
        ));
        assert_eq!((st, v["ok"].as_bool()), (200, Some(false)));
        let (st, v) = json(http(
            port,
            "POST",
            "/api/transcribe/queue/clear",
            &[("Content-Type", "application/json")],
            &[],
        ));
        assert_eq!(
            (st, v["ok"].as_bool(), v["cleared"].as_u64()),
            (200, Some(true), Some(0))
        );

        let (st, v) = json(http(port, "GET", "/api/system", &[], &[]));
        assert_eq!(st, 200);
        assert_eq!(
            v.pointer("/system/version").and_then(|x| x.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );

        let (st, v) = json(http(
            port,
            "POST",
            "/api/system/check",
            &[("Content-Type", "application/json")],
            json!({"probe_audio":false,"include_optional":false})
                .to_string()
                .as_bytes(),
        ));
        assert_eq!(st, 200);
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(
            v.get("checks").and_then(|x| x.as_array()).map(Vec::len),
            Some(10)
        );

        let (st, v) = json(http(port, "GET", "/api/journal", &[], &[]));
        assert_eq!(st, 200);
        assert!(v.get("sessions").and_then(|x| x.as_array()).is_some());
        let (st, v) = json(http(port, "GET", "/api/crashes", &[], &[]));
        assert_eq!(st, 200);
        assert!(v.get("items").and_then(|x| x.as_array()).is_some());

        let (st, v) = json(http(port, "GET", "/api/update/state", &[], &[]));
        assert_eq!(st, 200);
        assert_eq!(
            v.get("current").and_then(|x| x.as_str()),
            Some(env!("CARGO_PKG_VERSION"))
        );

        // Чужая вкладка не должна управлять записью (CSRF): Origin не локальный — 403.
        let (st, _, _) = http(
            port,
            "GET",
            "/api/status",
            &[("Origin", "https://evil.example")],
            &[],
        );
        assert_eq!(st, 403);
        // DNS rebinding: чужое имя хоста не должно приниматься как локальное.
        let (st, _, _) = http(
            port,
            "GET",
            "/api/status",
            &[("Host", "evil.example:8765")],
            &[],
        );
        assert_eq!(st, 403);
        let origin = format!("http://127.0.0.1:{port}");
        let (st, _, _) = http(port, "GET", "/api/status", &[("Origin", &origin)], &[]);
        assert_eq!(st, 200);

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

        let emb: Vec<f64> = (0..mr_ml::EMBEDDING_DIM)
            .map(|i| (i as f64) / mr_ml::EMBEDDING_DIM as f64)
            .collect();
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
        // Сохранение обязано писать и центроид, и прототип: иначе матчинг
        // деградирует до одного среднего вектора и теряет разные условия записи.
        let vp: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(base.join("packs/default/voiceprints.json")).unwrap(),
        )
        .unwrap();
        let prof = vp["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "Тестовый Голос")
            .expect("профиль не сохранён");
        assert_eq!(
            prof["embedding"].as_array().unwrap().len(),
            mr_ml::EMBEDDING_DIM
        );
        assert_eq!(prof["prototypes"].as_array().unwrap().len(), 1);
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
        // После удаления голоса открытая расшифровка не должна оставаться
        // «узнанной» этим профилем.
        let transcript = rec.join("Совещание с командой.transcript.json");
        std::fs::write(
            &transcript,
            serde_json::to_vec(&json!({
                "meta": {"speakers_info": [{
                    "name": "Тестовый Голос", "matched": true,
                    "matched_name": "Тестовый Голос"
                }]},
                "utterances": []
            }))
            .unwrap(),
        )
        .unwrap();
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voiceprints/delete",
            &[("Content-Type", "application/json")],
            json!({
                "name": "Тестовый Голос",
                "audio_file": rec.join("Совещание с командой.wav").to_string_lossy()
            })
            .to_string()
            .as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true))
        );
        let cleared: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&transcript).unwrap()).unwrap();
        assert_eq!(
            cleared["meta"]["speakers_info"][0]["matched"].as_bool(),
            Some(false)
        );

        // Импорт пакета JSON-бандлом + деталка + поштучное удаление образца.
        let emb1: Vec<serde_json::Value> = (0..mr_ml::EMBEDDING_DIM)
            .map(|i| json!((i as f64 + 1.0) / mr_ml::EMBEDDING_DIM as f64))
            .collect();
        let emb2: Vec<serde_json::Value> = (0..mr_ml::EMBEDDING_DIM)
            .map(|i| json!(1.0 - (i as f64) / mr_ml::EMBEDDING_DIM as f64))
            .collect();
        let bundle = json!({
            "format": "meeting-recorder-voicepack-v2",
            "manifest": {"id": "test-pack", "name": "Тестовый Пакет"},
            "people": [{"name": "TestVoice1"}],
            "voiceprints": {"version": 3, "profiles": [{
                "name": "TestVoice1", "embedding": emb1,
                "samples_count": 2, "updated_at": "",
                "prototypes": [
                    {"id": "p1", "embedding": emb1, "duration_sec": 1.0,
                     "quality": 1.0, "created_at": "2026-01-01T00:00:00Z",
                     "snippet_relpath": null},
                    {"id": "p2", "embedding": emb2, "duration_sec": 2.0,
                     "quality": 1.0, "created_at": "2026-01-02T00:00:00Z",
                     "snippet_relpath": null},
                ],
            }]},
        });
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voicepacks/import",
            &[
                ("Content-Type", "application/octet-stream"),
                ("X-Filename", "test.voicepack.json"),
            ],
            &serde_json::to_vec(&bundle).unwrap(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "voicepacks/import: {v}"
        );
        let (_, v) = json(http(
            port,
            "GET",
            "/api/voiceprints/detail?name=TestVoice1",
            &[],
            &[],
        ));
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(
            v.get("prototypes")
                .and_then(|x| x.as_array())
                .map(|a| a.len()),
            Some(2)
        );
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voiceprints/delete_sample",
            &[("Content-Type", "application/json")],
            json!({"name": "TestVoice1", "prototype_id": "p1"})
                .to_string()
                .as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "delete_sample: {v}"
        );
        let (_, v) = json(http(
            port,
            "GET",
            "/api/voiceprints/detail?name=TestVoice1",
            &[],
            &[],
        ));
        let protos = v
            .get("prototypes")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(protos.len(), 1);
        assert_eq!(protos[0].get("id").and_then(|x| x.as_str()), Some("p2"));
        let (_, v) = json(http(port, "GET", "/api/voicepacks", &[], &[]));
        let pack_id = v
            .get("packs")
            .and_then(|x| x.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|p| p.get("name").and_then(|x| x.as_str()) == Some("Тестовый Пакет"))
            })
            .and_then(|p| p.get("id"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        assert!(!pack_id.is_empty(), "импортированный пакет не в списке");
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voicepacks/delete",
            &[("Content-Type", "application/json")],
            json!({"pack_id": pack_id}).to_string().as_bytes(),
        ));
        assert_eq!(
            (st, v.get("ok").and_then(|x| x.as_bool())),
            (200, Some(true)),
            "voicepacks/delete: {v}"
        );
        let (_, v) = json(http(port, "GET", "/api/voicepacks", &[], &[]));
        assert!(!v
            .get("packs")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().any(|p| p
                .get("id")
                .and_then(|x| x.as_str())
                .map(|id| id == pack_id)
                .unwrap_or(false)))
            .unwrap_or(false));
        let (st, v) = json(http(
            port,
            "POST",
            "/api/voicepacks/delete",
            &[("Content-Type", "application/json")],
            json!({"pack_id": "default"}).to_string().as_bytes(),
        ));
        assert_eq!(st, 400);
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));

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
        assert_eq!(
            v.get("donation")
                .and_then(|d| d.get("methods"))
                .and_then(|m| m.get(0))
                .and_then(|m| m.get("url"))
                .and_then(|u| u.as_str()),
            Some("https://tips.tips/000490393")
        );

        // Удаление внешнего файла: из списка уходит, на диске остаётся.
        let external = rec.join("важное-внешнее.wav");
        std::fs::write(&external, b"external file").unwrap();
        let (_, v) = json(http(port, "GET", "/api/recordings", &[], &[]));
        let ext_item = v
            .get("recordings")
            .and_then(|x| x.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|r| r.get("name").and_then(|n| n.as_str()) == Some("важное-внешнее.wav"))
            })
            .expect("внешний файл должен быть в списке");
        assert_eq!(ext_item.get("owned").and_then(|x| x.as_bool()), Some(false));
        let (st, v) = json(http(
            port,
            "POST",
            "/api/recordings/delete",
            &[("Content-Type", "application/json")],
            serde_json::to_vec(&json!({"path": external.to_string_lossy()}))
                .unwrap()
                .as_slice(),
        ));
        assert_eq!(st, 200);
        assert_eq!(v.get("kept").and_then(|x| x.as_bool()), Some(true));
        assert!(external.exists(), "внешний файл нельзя удалять");
        let (_, v) = json(http(port, "GET", "/api/recordings", &[], &[]));
        assert!(!v
            .get("recordings")
            .and_then(|x| x.as_array())
            .map(|a| a
                .iter()
                .any(|r| r.get("name").and_then(|n| n.as_str()) == Some("важное-внешнее.wav")))
            .unwrap_or(false));

        let owned = rec.join("own.wav");
        std::fs::write(&owned, b"own file").unwrap();
        crate::media_registry::mark_owned(&owned);
        let (st, v) = json(http(
            port,
            "POST",
            "/api/recordings/delete",
            &[("Content-Type", "application/json")],
            serde_json::to_vec(&json!({"path": owned.to_string_lossy()}))
                .unwrap()
                .as_slice(),
        ));
        assert_eq!(st, 200);
        assert_eq!(v.get("kept").and_then(|x| x.as_bool()), None);
        assert!(!owned.exists(), "свою запись удаляем");

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
                assert!(rec.join("клип тест.mkv").exists());

                let mute_mp4 = rec.join("mute_video.mp4");
                let gen_mute = std::process::Command::new("ffmpeg")
                    .args([
                        "-nostdin",
                        "-v",
                        "error",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "testsrc=duration=1:size=64x64:rate=10",
                        "-c:v",
                        "libx264",
                        "-pix_fmt",
                        "yuv420p",
                        &mute_mp4.to_string_lossy(),
                    ])
                    .status();
                if gen_mute.map(|s| s.success()).unwrap_or(false) {
                    let req_body = serde_json::json!({
                        "file": mute_mp4.to_string_lossy().to_string(),
                    })
                    .to_string();
                    let (st, v) = json(http(
                        port,
                        "POST",
                        "/api/transcribe/start",
                        &[("Content-Type", "application/json")],
                        req_body.as_bytes(),
                    ));
                    #[cfg(feature = "onnx")]
                    {
                        assert_eq!(
                            st, 400,
                            "expected 400 Bad Request for video without audio: {v}"
                        );
                        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));
                        assert_eq!(
                            v.get("error").and_then(|x| x.as_str()),
                            Some("Файл не содержит аудиодорожки")
                        );
                    }
                    #[cfg(not(feature = "onnx"))]
                    {
                        let _ = &v;
                        assert_eq!(st, 501);
                    }
                }

                let test_audio = rec.join("lecture_rec.wav");
                mr_audio::write_stereo_wav(
                    &test_audio,
                    &vec![0.1; 16000],
                    &vec![0.1; 16000],
                    16000,
                )
                .unwrap();
                let test_video_raw = rec.join("temp_src.webm");
                let gen_v = std::process::Command::new("ffmpeg")
                    .args([
                        "-nostdin",
                        "-v",
                        "error",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "testsrc=duration=1:size=64x64:rate=10",
                        "-c:v",
                        "vp9",
                        "-f",
                        "webm",
                        &test_video_raw.to_string_lossy(),
                    ])
                    .status();
                if gen_v.map(|s| s.success()).unwrap_or(false) {
                    let v_bytes = std::fs::read(&test_video_raw).unwrap();
                    let _ = std::fs::remove_file(&test_video_raw);
                    let (st, v) = json(http(
                        port,
                        "POST",
                        "/api/recording/attach_video",
                        &[
                            ("X-Audio-Path", &test_audio.to_string_lossy()),
                            ("Content-Type", "video/webm"),
                        ],
                        &v_bytes,
                    ));
                    assert_eq!(st, 200, "attach_video error: {v}");
                    assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(true));
                    assert_eq!(v.get("is_video").and_then(|x| x.as_bool()), Some(true));
                    let attached_path = v.get("path").and_then(|x| x.as_str()).unwrap();
                    assert!(
                        std::path::Path::new(attached_path).exists(),
                        "видео файл не создан"
                    );
                }
            }
        }

        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Тело экспорта больше лимита не должно сохраняться усечённым с `ok: true`.
    #[test]
    fn oversized_export_body_is_rejected() {
        let _guard = super::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base =
            std::env::temp_dir().join(format!("mr-export-{}-{}", std::process::id(), now_secs()));
        std::env::set_var("MEETING_RECORDER_DATA_DIR", &base);
        std::fs::create_dir_all(&base).unwrap();
        let cfg = serde_json::json!({"save_dir": base.to_string_lossy()});
        std::fs::write(base.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();

        let port = free_port();
        let state = Arc::new(Mutex::new(recording::RecordingState::default()));
        std::thread::spawn(move || {
            run_http_server(state, port);
        });

        let limit = 16 * 1024 * 1024usize;
        let oversized = vec![b'x'; limit + 1];
        let (st, v) = json(http(
            port,
            "POST",
            "/api/export/save_dialog?name=big.docx",
            &[("Content-Type", "application/octet-stream")],
            &oversized,
        ));
        assert_eq!(st, 413, "усечённый экспорт недопустим: {v}");
        assert_eq!(v.get("ok").and_then(|x| x.as_bool()), Some(false));

        std::env::remove_var("MEETING_RECORDER_DATA_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }
}
