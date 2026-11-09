use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

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
    body: &[u8],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut last_err = String::new();
    for _ in 0..200 {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut s) => {
                let _ = s.set_read_timeout(Some(Duration::from_secs(120)));
                let req = format!(
                    "{method} {target} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
                     Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
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
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    panic!("сервер не поднялся: {last_err}");
}

fn json_call(port: u16, method: &str, target: &str, body: Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(&body).unwrap();
    let (status, _, raw) = http(port, method, target, &bytes);
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn wait_until<F: FnMut() -> bool>(limit: Duration, mut f: F) -> bool {
    let started = Instant::now();
    while started.elapsed() < limit {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn vm_hwm_mb(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = text.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

struct Cleanup {
    base: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        mr_app_lib::shutdown_summary_for_test();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

// Без env тест печатает причину и проходит; SUMMARY_REQUIRE_REAL=1
// делает пропуск ошибкой.
#[test]
fn summary_real_smoke() {
    let runtime = std::env::var("MR_SUMMARY_RUNTIME").ok();
    let model = std::env::var("MR_SUMMARY_MODEL").ok();
    let transcript = std::env::var("MR_TEST_TRANSCRIPT").ok();
    let require = std::env::var("SUMMARY_REQUIRE_REAL").ok().as_deref() == Some("1");
    let (Some(runtime), Some(model), Some(transcript)) = (runtime, model, transcript) else {
        let reason = "нужны MR_SUMMARY_RUNTIME, MR_SUMMARY_MODEL, MR_TEST_TRANSCRIPT";
        if require {
            panic!("SUMMARY_REQUIRE_REAL=1, но {reason}");
        }
        eprintln!("summary_real: skip — {reason}");
        return;
    };
    if !PathBuf::from(&runtime).exists() {
        panic!("runtime не найден: {runtime}");
    }
    if !PathBuf::from(&model).is_file() {
        panic!("модель не найдена: {model}");
    }
    let source = PathBuf::from(&transcript);
    if !source.is_file() {
        panic!("транскрипт не найден: {transcript}");
    }
    let base = std::env::temp_dir().join(format!(
        "mr-summary-real-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let rec = base.join("rec");
    std::fs::create_dir_all(&rec).unwrap();
    let config = json!({"save_dir": rec.to_string_lossy(), "auto_transcribe": false});
    std::fs::write(
        base.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let file = "real-sample.wav";
    let transcript_bytes = std::fs::copy(&source, rec.join("real-sample.transcript.json"))
        .map_err(|e| panic!("копирование транскрипта: {e}"))
        .unwrap();
    std::fs::write(rec.join("real-sample.md"), "# synthetic\n").unwrap();
    assert!(transcript_bytes > 0);

    std::env::set_var("MEETING_RECORDER_DATA_DIR", &base);
    std::env::set_var("PODTEKST_SUMMARY_ENABLED", "1");
    std::env::set_var("MR_SUMMARY_RUNTIME", &runtime);
    std::env::set_var("MR_SUMMARY_MODEL", &model);
    std::env::remove_var("MR_SUMMARY_FAKE_BACKEND");

    let port = free_port();
    let _cleanup = Cleanup { base: base.clone() };
    mr_app_lib::spawn_test_http_server(port);
    let (st, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
    assert_eq!(st, 200, "GET: {v}");
    let rev = v["source_rev"].as_str().unwrap().to_string();
    assert_eq!(
        v["capabilities"]["runtime"], "sidecar",
        "runtime не найден: {v}"
    );
    assert_eq!(v["block_reason"], Value::Null, "preflight блокирует: {v}");

    let started = Instant::now();
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal", "source_rev": rev}),
    );
    assert_eq!(st, 202, "generate: {v}");
    let job = v["job_id"].as_str().unwrap().to_string();
    let done = wait_until(Duration::from_secs(300), || {
        let (_, s) = json_call(port, "GET", "/api/status", json!({}));
        s["summarizing"].as_bool() == Some(false)
    });
    assert!(done, "генерация не завершилась за 300 с");
    let elapsed = started.elapsed();
    let (st, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
    assert_eq!(st, 200);
    assert_eq!(v["state"], "fresh", "нет fresh-артефакта: {v}");
    let summary = &v["summary"];
    assert!(summary["id"].is_string());
    assert!(!summary["sections"].as_array().unwrap().is_empty());
    assert!(!summary["markdown"].as_str().unwrap().is_empty());
    assert_eq!(summary["source_rev"], rev);
    let json_path = rec.join("real-sample.summary.json");
    let artifact: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(artifact["source"]["rev"], rev);
    assert_eq!(
        artifact["generator"]["model_sha256"],
        "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"
    );
    assert!(artifact["generator"]["prompt_version"]
        .as_str()
        .unwrap()
        .starts_with("meeting-"));
    let pid_file = base.join("summary/sidecar.pid.json");
    let peak = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|r| r["pid"].as_u64())
        .and_then(|pid| vm_hwm_mb(pid as u32));
    let duration_ms = artifact["generator"]["duration_ms"].as_u64().unwrap_or(0);
    let timings = &artifact["generator"]["timings_ms"];
    println!(
        "summary_real: total={}ms wall={}ms timings={} peak_hwm_mb={:?} items={}",
        duration_ms,
        elapsed.as_millis(),
        timings,
        peak,
        artifact["summary"]["items_total"]
    );
    if let Some(peak) = peak {
        assert!(peak <= 6656, "пик {peak} MiB выше бюджета 6,5 ГиБ");
    }

    // Cancel <= 2 c: артефакт предыдущего job остаётся целым.
    let (st, v) = json_call(
        port,
        "POST",
        "/api/summary/generate",
        json!({"file": file, "preset_id": "meeting", "detail": "normal",
               "source_rev": rev, "force": true}),
    );
    assert_eq!(st, 202, "cancel generate: {v}");
    let job2 = v["job_id"].as_str().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(500));
    let cancel_started = Instant::now();
    let (st, v) = json_call(port, "POST", "/api/summary/cancel", json!({"job_id": job2}));
    assert_eq!(
        (st, v["cancelled"].as_bool()),
        (200, Some(true)),
        "cancel: {v}"
    );
    let stopped = wait_until(Duration::from_secs(5), || {
        let (_, s) = json_call(port, "GET", "/api/status", json!({}));
        s["summarizing"].as_bool() == Some(false)
    });
    let cancel_ms = cancel_started.elapsed().as_millis();
    assert!(stopped, "cancel не остановил job");
    assert!(cancel_ms <= 2000, "cancel {cancel_ms} ms > 2 c");
    let (_, v) = json_call(port, "GET", &format!("/api/summary?file={file}"), json!({}));
    assert_eq!(v["state"], "fresh", "cancel испортил прежний артефакт");
    println!("summary_real: cancel={cancel_ms}ms job={job}");
}
