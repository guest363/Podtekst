//! Контрактные тесты summary HTTP/SSE на fake-backend (без модели и runtime).
//!
//! Один последовательный сценарий: env-переменные и общий сервер нельзя
//! безопасно делить между параллельными тестами одного бинарника.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

static LOCK: Mutex<()> = Mutex::new(());
const CANARY: &str = "CANARY_0123456789abcdef0123456789abcdef";

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
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut s) => {
                let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(15)));
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
    panic!("сервер не поднялся: {last_err}");
}

fn json_call(port: u16, method: &str, target: &str, body: Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(&body).unwrap();
    let (status, _, raw) = http(
        port,
        method,
        target,
        &[("Content-Type", "application/json")],
        &bytes,
    );
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn transcript_json(texts: &[&str]) -> Value {
    let utterances: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            json!({
                "start": i as f64 * 2.0,
                "end": i as f64 * 2.0 + 1.8,
                "speaker": if i % 2 == 0 { "Спикер 1" } else { "Спикер 2" },
                "text": text,
            })
        })
        .collect();
    json!({
        "meta": {"speakers": ["Спикер 1", "Спикер 2"], "speakers_info": []},
        "utterances": utterances,
    })
}

fn wait_idle(port: u16, timeout: std::time::Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let (_, v) = json_call(port, "GET", "/api/status", json!({}));
        if v.get("summarizing").and_then(Value::as_bool) == Some(false) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

fn summary_artifact(dir: &std::path::Path, stem: &str) -> Value {
    let bytes = std::fs::read(dir.join(format!("{stem}.summary.json"))).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn wait_state(port: u16, file: &str, expected: &str, timeout: std::time::Duration) -> Value {
    let started = std::time::Instant::now();
    loop {
        let (_, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
        if v.get("state").and_then(Value::as_str) == Some(expected) {
            return v;
        }
        if started.elapsed() > timeout {
            panic!("state для {file} не стал {expected}: {v}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn summary_contract_suite() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = std::env::temp_dir().join(format!(
        "mr-summary-contract-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let rec = base.join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let config = json!({
        "save_dir": rec.to_string_lossy(),
        "auto_transcribe": false,
    });
    std::fs::write(
        base.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    std::env::set_var("MEETING_RECORDER_DATA_DIR", &base);
    std::env::set_var("PODTEKST_SUMMARY_ENABLED", "1");
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");
    std::env::remove_var("SUMMARY_CLASSIFIER_MODE");
    std::env::remove_var("MR_SUMMARY_MODEL");

    let port = free_port();
    mr_app_lib::spawn_test_http_server(port);

    // Синтетический транскрипт: canary нужен только для проверки SSE-утечек.
    let texts = [
        format!("{CANARY} Обсудили срок запуска и договорились проверить отчёт до среды."),
        "Решили подготовить план и отправить его коллегам завтра.".to_string(),
        "Вопрос по бюджету остался открытым.".to_string(),
        "Согласовали дату следующей встречи и ответственного за задачи.".to_string(),
    ];
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let transcript = transcript_json(&refs);
    let file = "contract-sample.wav";
    std::fs::write(
        rec.join("contract-sample.transcript.json"),
        serde_json::to_vec_pretty(&transcript).unwrap(),
    )
    .unwrap();
    let md_before = "# Синтетическая расшифровка\n\n- реплики\n";
    std::fs::write(rec.join("contract-sample.md"), md_before).unwrap();
    let wav = base.join("src.wav");
    mr_audio::write_stereo_wav(&wav, &vec![0.1; 1600], &vec![0.1; 1600], 16000).unwrap();
    let wav_bytes = std::fs::read(&wav).unwrap();
    let (st, _, raw) = http(
        port,
        "POST",
        "/api/upload",
        &[("X-Filename", "contract-sample.wav")],
        &wav_bytes,
    );
    assert_eq!(st, 200, "upload: {raw:?}");

    // 1. Каталог: 7 шаблонов, universal последний.
    let (st, v) = json_call(port, "GET", "/api/summary/presets", json!({}));
    assert_eq!(st, 200);
    let presets = v["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 7);
    assert_eq!(presets[6]["id"], "universal");

    // 2. GET: missing + recommendation, без модели/runtime (fake).
    let (st, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
    assert_eq!(st, 200, "GET summary: {v}");
    assert_eq!(v["state"], "missing");
    let rev = v["source_rev"].as_str().unwrap().to_string();
    assert_eq!(rev.len(), 64);
    assert!(v["classification"].is_object(), "нет рекомендации: {v}");
    assert_eq!(v["settings"]["auto_suggest"], true);
    assert_eq!(v["capabilities"]["classifier"], "heuristic");
    assert_eq!(v["limits"]["min_free_ram_mb"], 6656);

    // 3. classify: sync 200 + force пересчитывает.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/classify",
        json!({"file": file, "force": true}),
    );
    assert_eq!(st, 200, "classify: {v}");
    assert_eq!(v["source_rev"], rev);
    assert!(v["classification"]["spec_version"].is_string());
    assert_eq!(v["cache_hit"], false);

    // 4. stale_source preflight.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal",
               "source_rev": "0".repeat(64)}),
    );
    assert_eq!(st, 409);
    assert_eq!(v["code"], "stale_source");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal"}),
    );
    assert_eq!((st, v["code"].as_str()), (400, Some("source_rev_required")));
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "nope", "detail": "normal", "source_rev": rev}),
    );
    assert_eq!((st, v["code"].as_str()), (400, Some("invalid_preset")));
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "huge", "source_rev": rev}),
    );
    assert_eq!((st, v["code"].as_str()), (400, Some("invalid_detail")));

    // 5. Успешная генерация + артефакты.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal", "source_rev": rev}),
    );
    assert_eq!(st, 202, "generate: {v}");
    let first_job = v["job_id"].as_str().unwrap().to_string();
    // 6. Второй job при активном -> 409 summary_busy. Fake быстрый, поэтому
    // проверяем в slow-режиме ниже; здесь только форма ответа.
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    let v = wait_state(port, file, "fresh", std::time::Duration::from_secs(10));
    let summary_id = v["summary"]["id"].as_str().unwrap().to_string();
    assert!(!summary_id.is_empty());
    assert!(!v["summary"]["sections"].as_array().unwrap().is_empty());
    assert!(!v["summary"]["markdown"].as_str().unwrap().is_empty());
    assert_eq!(v["summary"]["job_id"], summary_id);
    let json_path = rec.join("contract-sample.summary.json");
    let md_path = rec.join("contract-sample.summary.md");
    assert!(json_path.is_file() && md_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&json_path, &md_path] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600,
                "summary-файл должен быть 0600"
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(rec.join("contract-sample.md")).unwrap(),
        md_before,
        "transcript .md изменён"
    );

    // 7. Cached vs force.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal", "source_rev": rev}),
    );
    assert_eq!(
        (st, v["cached"].as_bool()),
        (200, Some(true)),
        "cached: {v}"
    );
    assert_eq!(v["summary_id"], summary_id);
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev, "force": true}),
    );
    assert_eq!(st, 202, "force: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    let v = wait_state(port, file, "fresh", std::time::Duration::from_secs(10));
    assert_ne!(v["summary"]["id"].as_str(), Some(summary_id.as_str()));

    // 8. has_summary additive, has_transcript прежний.
    let (_, v) = json_call(port, "GET", "/api/recordings", json!({}));
    let item = v["recordings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == file)
        .expect("записи нет");
    assert_eq!(item["has_summary"], true);
    assert_eq!(item["has_transcript"], true);

    // 9. Rename двигает оба summary; speaker edit делает stale.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/recordings/rename",
        json!({"path": item["path"], "name": "contract-renamed"}),
    );
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)), "rename: {v}");
    let renamed = "contract-renamed.wav";
    assert!(rec.join("contract-renamed.summary.json").is_file());
    assert!(rec.join("contract-renamed.summary.md").is_file());
    assert!(!json_path.exists());
    let (st, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert_eq!(
        (st, v["state"].as_str()),
        (200, Some("fresh")),
        "rename→fresh"
    );
    let (st, v) = json_call(
        port,
        "POST",
        "/api/transcript/edit",
        json!({"file": renamed, "old_speaker": "Спикер 2", "speaker": "Участник"}),
    );
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)), "edit: {v}");
    assert!(v["rev"].as_str().is_some());
    let (_, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert_eq!(v["state"], "stale", "speaker rename не пометил stale");

    // 10. Body limit 64 KiB.
    let big = vec![b'{'; 70 * 1024];
    let (st, _, raw) = http(
        port,
        "POST",
        "/api/summary/classify",
        &[("Content-Type", "application/json")],
        &big,
    );
    assert_eq!(st, 413);
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["code"], "body_too_large");

    // 11. Удалённые маршруты -> 404, не новые эндпоинты.
    for (method, path) in [
        ("GET", "/api/summary/status"),
        ("GET", "/api/summary_stream"),
        ("POST", "/api/summary/edit"),
    ] {
        let (st, _, _) = http(port, method, path, &[], b"{}");
        assert_eq!(st, 404, "{method} {path} должен быть 404");
    }

    // 12. manual_only (D15 fallback): рекомендации нет, settings/classify 409.
    std::env::set_var("SUMMARY_CLASSIFIER_MODE", "manual_only");
    let (_, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert!(
        v["classification"].is_null(),
        "manual_only дал рекомендацию"
    );
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/classify",
        json!({"file": renamed}),
    );
    assert_eq!(
        (st, v["code"].as_str()),
        (409, Some("classifier_manual_only"))
    );
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/settings",
        json!({"auto_suggest": true}),
    );
    assert_eq!(
        (st, v["code"].as_str()),
        (409, Some("classifier_manual_only"))
    );
    std::env::remove_var("SUMMARY_CLASSIFIER_MODE");

    // 12b. auto_suggest=false: классификация не вычисляется и не отдаётся.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/settings",
        json!({"auto_suggest": false}),
    );
    assert_eq!(
        (st, v["ok"].as_bool()),
        (200, Some(true)),
        "settings off: {v}"
    );
    let (_, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert!(
        v["classification"].is_null(),
        "auto_suggest=false дал рекомендацию"
    );
    let (st, _) = json_call(
        port,
        "POST",
        "/api/summary/settings",
        json!({"auto_suggest": true}),
    );
    assert_eq!(st, 200);

    // 13. stream-before-POST: этап и терминал на одной SSE-шине, без текста.
    let (mut sse, total_before) = open_sse(port);
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "slow");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": v_rev(port, renamed), "force": true}),
    );
    assert_eq!(st, 202, "slow generate: {v}");
    let active_job = v["job_id"].as_str().unwrap().to_string();
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": v_rev(port, renamed), "force": true}),
    );
    assert_eq!((st, v["code"].as_str()), (409, Some("summary_busy")));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/cancel",
        json!({"job_id": active_job}),
    );
    assert_eq!(
        (st, v["cancelled"].as_bool()),
        (200, Some(true)),
        "cancel: {v}"
    );
    let capture =
        read_sse_until_terminal(&mut sse, total_before, std::time::Duration::from_secs(8));
    assert!(capture.contains("summary-stage"), "нет stage: {capture}");
    assert!(
        capture.contains("summary-error"),
        "нет терминала: {capture}"
    );
    assert!(
        capture.contains("cancelled"),
        "терминал не cancelled: {capture}"
    );
    assert!(!capture.contains(CANARY), "canary утёк в SSE");
    for line in capture.lines().filter(|l| l.starts_with("data: ")) {
        let event: Value = serde_json::from_str(&line[6..]).unwrap();
        if event["type"].as_str().unwrap_or("").starts_with("summary-") {
            for forbidden in ["text", "delta", "token", "markdown", "prompt", "message"] {
                assert!(
                    event.get(forbidden).is_none(),
                    "запрещённое поле {forbidden}: {event}"
                );
            }
        }
    }
    let pcts: Vec<u64> = capture
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.strip_prefix("data: ")?).ok())
        .filter(|e| e["type"] == "summary-stage")
        .filter_map(|e| e["pct"].as_u64())
        .collect();
    assert!(
        pcts.windows(2).all(|w| w[0] <= w[1]),
        "pct не монотонен: {pcts:?}"
    );
    assert!(wait_idle(port, std::time::Duration::from_secs(5)));
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    // 13b. Неизвестный job_id -> 404 unknown_job.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/cancel",
        json!({"job_id": "00000000-0000-4000-8000-000000000000"}),
    );
    assert_eq!((st, v["code"].as_str()), (404, Some("unknown_job")));

    // 13c. Терминальный stale_source: правка во время job, предыдущий цел.
    let (_, before) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    let before_id = before["summary"]["id"].as_str().unwrap().to_string();
    let (mut sse, cursor) = open_sse(port);
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "slow");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": v_rev(port, renamed), "force": true}),
    );
    assert_eq!(st, 202, "stale job: {v}");
    std::thread::sleep(std::time::Duration::from_millis(150));
    let (st, v) = json_call(
        port,
        "POST",
        "/api/transcript/edit",
        json!({"file": renamed, "index": 0, "text": "Изменённая реплика для проверки устаревания."}),
    );
    assert_eq!(
        (st, v["ok"].as_bool()),
        (200, Some(true)),
        "edit during job: {v}"
    );
    let capture = read_sse_until_terminal(&mut sse, cursor, std::time::Duration::from_secs(8));
    assert!(
        capture.contains("stale_source"),
        "нет терминального stale_source: {capture}"
    );
    assert!(wait_idle(port, std::time::Duration::from_secs(5)));
    let (_, after) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert_eq!(after["state"], "stale");
    assert_eq!(
        after["summary"]["id"].as_str(),
        Some(before_id.as_str()),
        "stale job заменил предыдущий артефакт"
    );
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    // 14. Модель отсутствует и fake выключен -> 503 model_missing.
    std::env::remove_var("MR_SUMMARY_FAKE_BACKEND");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": v_rev(port, renamed), "force": true}),
    );
    assert_eq!(
        (st, v["code"].as_str()),
        (503, Some("model_missing")),
        "model_missing: {v}"
    );
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    // 15. newer-major: default refusal; allow_downgrade делает backup.
    let json_path = rec.join("contract-renamed.summary.json");
    let v2 = std::fs::read_to_string(&json_path)
        .unwrap()
        .replace("podtekst.summary/v1", "podtekst.summary/v2");
    std::fs::write(&json_path, v2).unwrap();
    let (_, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert_eq!(v["state"], "unsupported");
    let rev_now = v_rev(port, renamed);
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev_now, "force": true}),
    );
    assert_eq!((st, v["code"].as_str()), (422, Some("schema_unsupported")));
    assert!(
        std::fs::read_to_string(&json_path)
            .unwrap()
            .contains("podtekst.summary/v2"),
        "unsupported-файл изменён без разрешения"
    );
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev_now, "force": true, "allow_downgrade": true}),
    );
    assert_eq!(st, 202, "allow_downgrade: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    let backups: Vec<String> = std::fs::read_dir(&rec)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".summary.json.newer."))
        .collect();
    assert_eq!(backups.len(), 1, "нет backup newer: {backups:?}");

    // 16. System: summary не core, отдельный блок.
    let (_, v) = json_call(port, "GET", "/api/system", json!({}));
    let models = &v["system"]["models"];
    let items = models["items"].as_array().unwrap();
    let summary_item = items
        .iter()
        .find(|m| m["kind"] == "summary")
        .expect("summary нет в каталоге");
    assert_eq!(summary_item["ram_mb"], 5530);
    assert_eq!(summary_item["required_free_mb"], 6656);
    assert!(models["core_total"].as_u64().unwrap() >= 1);
    assert!(!models["missing_core"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id.as_str().unwrap_or("").contains("sum-")));
    assert_eq!(models["summary"]["supported"], true);
    let (_, v) = json_call(port, "GET", "/api/status", json!({}));
    assert_eq!(v["features"]["summary"], true);
    assert_eq!(v["summary_classifier"], "heuristic");
    assert_eq!(v["summarizing"], false);

    // 17. Удаление удаляет оба summary (owned-файл из fake-записи).
    std::env::set_var("MR_FAKE_CAPTURE", "1");
    let (st, v) = json_call(port, "POST", "/api/record/start", json!({"mode": "mix"}));
    assert_eq!(
        (st, v["ok"].as_bool()),
        (200, Some(true)),
        "record start: {v}"
    );
    let started_file = v["file"].as_str().unwrap().to_string();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let (st, v) = json_call(port, "POST", "/api/record/stop", json!({}));
    assert_eq!(
        (st, v["ok"].as_bool()),
        (200, Some(true)),
        "record stop: {v}"
    );
    std::env::remove_var("MR_FAKE_CAPTURE");
    // Остановка финализирует имя (добавляет время окончания) — берём из ответа.
    let recorded = v["file"].as_str().unwrap_or(&started_file).to_string();
    let recorded_path = PathBuf::from(&recorded);
    let stem = recorded_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let recorded_transcript = serde_json::to_vec_pretty(&transcript_json(&[
        "Проверка удаления итогов вместе с записью.",
        "Обсудили срок и задачи на следующей неделе.",
    ]))
    .unwrap();
    std::fs::write(
        rec.join(format!("{stem}.transcript.json")),
        recorded_transcript,
    )
    .unwrap();
    std::fs::write(rec.join(format!("{stem}.md")), "# t\n").unwrap();
    let rev_recorded = v_rev(port, &recorded_path.file_name().unwrap().to_string_lossy());
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": recorded, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev_recorded}),
    );
    assert_eq!(st, 202, "recorded generate: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    wait_state(
        port,
        &recorded_path.file_name().unwrap().to_string_lossy(),
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let rjson = rec.join(format!("{stem}.summary.json"));
    let rmd = rec.join(format!("{stem}.summary.md"));
    assert!(rjson.is_file() && rmd.is_file());
    let (st, v) = json_call(
        port,
        "POST",
        "/api/recordings/delete",
        json!({"path": recorded}),
    );
    assert_eq!(st, 200, "delete: {v}");
    assert!(
        !rjson.exists() && !rmd.exists(),
        "summary не удалён с записью"
    );
    assert!(!recorded_path.exists());

    // 18. Corrupt при regenerate -> бэкап, новый артефакт.
    let json_path = rec.join("contract-renamed.summary.json");
    std::fs::write(&json_path, "{broken").unwrap();
    let (_, v) = json_call(
        port,
        "GET",
        &format!("/api/summary?file={renamed}"),
        json!({}),
    );
    assert_eq!(v["state"], "corrupt");
    let rev_now = v_rev(port, renamed);
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": renamed, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev_now, "force": true}),
    );
    assert_eq!(st, 202, "corrupt regenerate: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    wait_state(port, renamed, "fresh", std::time::Duration::from_secs(10));
    let corrupt = std::fs::read_dir(&rec)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(".summary.json.corrupt.")
        })
        .count();
    assert_eq!(corrupt, 1, "нет corrupt-бэкапа");

    // 19. too_short/no_text.
    std::fs::write(
        rec.join("short.transcript.json"),
        serde_json::to_vec(&json!({"utterances": []})).unwrap(),
    )
    .unwrap();
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/classify",
        json!({"file": "short.wav"}),
    );
    assert_eq!((st, v["code"].as_str()), (422, Some("no_text")));
    std::fs::write(
        rec.join("tiny.transcript.json"),
        serde_json::to_vec(&transcript_json(&["Привет."])).unwrap(),
    )
    .unwrap();
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/classify",
        json!({"file": "tiny.wav"}),
    );
    assert_eq!((st, v["code"].as_str()), (422, Some("too_short")));

    // 20. Sensitive: server-enforced confirmation, флаг в артефакте.
    std::fs::write(
        rec.join("legal.transcript.json"),
        serde_json::to_vec(&transcript_json(&refs)).unwrap(),
    )
    .unwrap();
    let legal_rev = v_rev(port, "legal.wav");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "legal.wav", "preset_id": "legal", "detail": "normal",
               "source_rev": legal_rev}),
    );
    assert_eq!(
        (st, v["code"].as_str()),
        (400, Some("confirmation_required")),
        "legal без подтверждения: {v}"
    );
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "legal.wav", "preset_id": "legal", "detail": "short",
               "source_rev": legal_rev, "confirm_sensitive": true}),
    );
    assert_eq!(st, 202, "legal confirmed: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    wait_state(
        port,
        "legal.wav",
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let artifact: Value =
        serde_json::from_slice(&std::fs::read(rec.join("legal.summary.json")).unwrap()).unwrap();
    assert_eq!(artifact["preset"]["confirmed_sensitive"], true);
    assert_eq!(artifact["preset"]["id"], "legal");

    // 21. Map-reduce: реальный reduce-проход, а не merge.
    let long: Vec<String> = (0..200)
        .map(|i| format!("Обсудили пункт {i}: решили подготовить отчёт и проверить срок до среды."))
        .collect();
    let long_refs: Vec<&str> = long.iter().map(String::as_str).collect();
    std::fs::write(
        rec.join("long.transcript.json"),
        serde_json::to_vec(&transcript_json(&long_refs)).unwrap(),
    )
    .unwrap();
    let long_rev = v_rev(port, "long.wav");
    let (mut sse, cursor) = open_sse(port);
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "long.wav", "preset_id": "meeting", "detail": "normal",
               "source_rev": long_rev}),
    );
    assert_eq!(st, 202, "map-reduce: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(30)));
    let capture = read_sse_until_terminal(&mut sse, cursor, std::time::Duration::from_secs(5));
    assert!(
        capture.contains("summary-done"),
        "map-reduce завершился не done: {capture}"
    );
    wait_state(
        port,
        "long.wav",
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let artifact = summary_artifact(&rec, "long");
    assert_eq!(artifact["provenance"]["mode"], "map_reduce");
    assert_eq!(artifact["provenance"]["reduce_levels"], 1);
    assert!(artifact["provenance"]["chunks"].as_u64().unwrap() >= 2);
    assert!(!artifact["summary"]["sections"]
        .as_array()
        .unwrap()
        .is_empty());

    // 22. Невалидный reduce -> заземлённый merge, job не теряется.
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "reduce_invalid");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "long.wav", "preset_id": "meeting", "detail": "normal",
               "source_rev": long_rev, "force": true}),
    );
    assert_eq!(st, 202, "reduce invalid: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(30)));
    wait_state(
        port,
        "long.wav",
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let artifact = summary_artifact(&rec, "long");
    assert_eq!(artifact["provenance"]["mode"], "map_reduce");
    assert_eq!(
        artifact["provenance"]["retry_reason"], "reduce_invalid",
        "{}",
        artifact["provenance"]
    );
    assert!(artifact["provenance"]["parser_recovered"].as_u64().unwrap() >= 1);
    assert!(!artifact["summary"]["sections"]
        .as_array()
        .unwrap()
        .is_empty());
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    // 23. output_thin: первый ответ пустой, repair-повтор даёт пункты.
    let thin_texts: Vec<String> = (0..20)
        .map(|i| format!("Обсудили пункт номер {i} и проверили отчёт по запуску проекта."))
        .collect();
    let thin_refs: Vec<&str> = thin_texts.iter().map(String::as_str).collect();
    std::fs::write(
        rec.join("thin.transcript.json"),
        serde_json::to_vec(&transcript_json(&thin_refs)).unwrap(),
    )
    .unwrap();
    let thin_rev = v_rev(port, "thin.wav");
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "thin_once");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "thin.wav", "preset_id": "meeting", "detail": "normal",
               "source_rev": thin_rev}),
    );
    assert_eq!(st, 202, "thin_once: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    wait_state(
        port,
        "thin.wav",
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let artifact = summary_artifact(&rec, "thin");
    assert!(artifact["provenance"]["parser_retries"].as_u64().unwrap() >= 1);
    assert!(artifact["provenance"]["retry_reason"]
        .as_str()
        .unwrap_or("")
        .contains("output_thin"));
    assert!(artifact["summary"]["items_total"].as_u64().unwrap() >= 2);
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    // 24. decision_missed: первый ответ без решений, repair возвращает cue.
    let decision_texts = [
        "Решили перенести запуск на следующую неделю.",
        "Подготовить отчёт поручили коллегам.",
        "Вопрос по бюджету остался открытым.",
    ];
    std::fs::write(
        rec.join("decision.transcript.json"),
        serde_json::to_vec(&transcript_json(&decision_texts)).unwrap(),
    )
    .unwrap();
    let decision_rev = v_rev(port, "decision.wav");
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "no_decisions");
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": "decision.wav", "preset_id": "meeting", "detail": "normal",
               "source_rev": decision_rev}),
    );
    assert_eq!(st, 202, "no_decisions: {v}");
    assert!(wait_idle(port, std::time::Duration::from_secs(20)));
    wait_state(
        port,
        "decision.wav",
        "fresh",
        std::time::Duration::from_secs(10),
    );
    let artifact = summary_artifact(&rec, "decision");
    assert!(artifact["provenance"]["parser_retries"].as_u64().unwrap() >= 1);
    assert!(artifact["provenance"]["retry_reason"]
        .as_str()
        .unwrap_or("")
        .contains("decision_missed"));
    assert!(artifact["summary"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == "decisions" && !s["items"].as_array().unwrap().is_empty()));
    std::env::set_var("MR_SUMMARY_FAKE_BACKEND", "1");

    assert!(!first_job.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

fn v_rev(port: u16, file: &str) -> String {
    let (_, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
    v["source_rev"].as_str().unwrap_or("").to_string()
}

fn open_sse(port: u16) -> (TcpStream, usize) {
    let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    s.write_all(
        b"GET /api/transcribe_stream HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    // Дожидаемся заголовков: поток уже открыт, job ещё не стартовал.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut byte).unwrap_or(0) == 0 {
            panic!("SSE закрылся до заголовков");
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head);
    let cursor = text
        .lines()
        .find_map(|l| l.strip_prefix("id: ").and_then(|v| v.trim().parse().ok()))
        .unwrap_or(0);
    (s, cursor)
}

fn read_sse_until_terminal(
    s: &mut TcpStream,
    _cursor: usize,
    limit: std::time::Duration,
) -> String {
    let started = std::time::Instant::now();
    let mut out = String::new();
    let mut buf = [0u8; 4096];
    while started.elapsed() < limit {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                if out.contains("summary-error") || out.contains("summary-done") {
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    out
}
