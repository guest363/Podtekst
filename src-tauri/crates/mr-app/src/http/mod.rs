pub(crate) mod export_dialog;
pub(crate) mod files;
pub(crate) mod media;
pub(crate) mod people;
pub(crate) mod transcribe_api;
pub(crate) mod transcript_edit;
pub(crate) mod update;

use std::io::Read;
use std::sync::{Arc, Mutex};

use mr_audio::get_audio_devices_info;
use mr_packs as packs;
use serde_json::json;

use self::export_dialog::handle_save_dialog;
use self::files::{
    handle_attach_video, handle_extract_audio, handle_recording_delete, handle_recording_rename,
    handle_upload,
};
use self::media::{local_request_allowed, recordings_list, serve_audio_file, serve_media_snippet};
use self::people::{
    handle_voiceprint_delete, handle_voiceprint_save, load_all_voiceprints, load_external_people,
    people_json_files, read_people_list, remove_person, save_external_people, update_people,
};
use self::transcribe_api::{
    handle_models_update, handle_transcribe_start, serve_transcribe_stream, serve_transcript,
};
use self::transcript_edit::handle_transcript_edit;
use self::update::{handle_update_apply, handle_update_check, handle_update_progress};
use crate::config::config_lock;
use crate::config::{
    app_data_dir, configured_auto_transcribe, configured_record_mode, save_auto_transcribe,
    save_dir, save_record_mode, static_dir,
};
use crate::recording::now_secs;
use crate::recording::{start_recording_inner, stop_recording_inner, RecordingState};

const DONATE_TIPS_URL: &str = "https://tips.tips/000490393";

fn donation_methods() -> serde_json::Value {
    if DONATE_TIPS_URL.is_empty() {
        return json!([]);
    }
    json!([{
        "title": "Чаевые автору",
        "desc": "СБП или банковская карта",
        "url": DONATE_TIPS_URL
    }])
}

fn json_response(request: tiny_http::Request, value: serde_json::Value, status: u16) {
    let body = serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string());
    let resp = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(hdr("Content-Type: application/json; charset=utf-8"));
    let _ = request.respond(resp);
}

/// Парсинг строки заголовка без паники: невалидная строка даёт нейтральный
/// заголовок вместо падения единственного потока сервера.
fn hdr(line: &str) -> tiny_http::Header {
    line.parse::<tiny_http::Header>().unwrap_or_else(|_| {
        "X-Unknown: ?"
            .parse::<tiny_http::Header>()
            .expect("константный заголовок валиден")
    })
}

/// Тело статики: сначала диск (dev), иначе вшитая в бинарник копия.
/// Нужно для запуска не из корня репо (ярлык, трей, автозапуск) — иначе окно показывает 404.
/// Разрешён только корень static/ и js/ без `..`.
pub(crate) fn static_body(rel: &str) -> Option<(&'static str, Vec<u8>)> {
    let rel = percent_decode(rel.trim_start_matches('/'));
    if rel.is_empty() || rel.contains("..") {
        return None;
    }
    let kind: &'static str = if rel.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if rel.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if rel == "index.html" {
        "text/html; charset=utf-8"
    } else if rel == "favicon.svg" {
        "image/svg+xml"
    } else if rel == "icon.png" {
        "image/png"
    } else {
        return None;
    };
    const FALLBACK_INDEX: &[u8] = include_bytes!("../../../../../ui/web/static/index.html");
    const FALLBACK_FAVICON: &[u8] = include_bytes!("../../../../../ui/web/static/favicon.svg");
    const FALLBACK_ICON: &[u8] = include_bytes!("../../../../../ui/web/static/icon.png");
    const FALLBACK_CSS: &[(&str, &[u8])] = &[(
        "css/app.css",
        include_bytes!("../../../../../ui/web/static/css/app.css"),
    )];
    const FALLBACK_JS: &[(&str, &[u8])] = &[(
        "js/app.js",
        include_bytes!("../../../../../ui/web/static/js/app.js"),
    )];
    let body = std::fs::read(static_dir().join(&rel)).ok().or_else(|| {
        if rel == "index.html" {
            Some(FALLBACK_INDEX.to_vec())
        } else if rel == "favicon.svg" {
            Some(FALLBACK_FAVICON.to_vec())
        } else if rel == "icon.png" {
            Some(FALLBACK_ICON.to_vec())
        } else {
            FALLBACK_CSS
                .iter()
                .chain(FALLBACK_JS.iter())
                .find(|(name, _)| *name == rel)
                .map(|(_, bytes)| bytes.to_vec())
        }
    });
    body.map(|b| (kind, b))
}

fn serve_static(request: tiny_http::Request, rel: &str) {
    match static_body(rel) {
        Some((kind, body)) => {
            let resp = tiny_http::Response::from_data(body)
                .with_header(hdr(&format!("Content-Type: {kind}")));
            let _ = request.respond(resp);
        }
        None => {
            let resp = tiny_http::Response::from_string("Not Found").with_status_code(404);
            let _ = request.respond(resp);
        }
    }
}

const MAX_JSON_BODY: u64 = 16 * 1024 * 1024;

fn read_body_to_json(request: &mut tiny_http::Request) -> serde_json::Value {
    let mut buf = Vec::new();
    let _ = request
        .as_reader()
        .take(MAX_JSON_BODY)
        .read_to_end(&mut buf);
    serde_json::from_slice(&buf).unwrap_or(json!({}))
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(&h), Some(&l)) = (b.get(i + 1), b.get(i + 2)) {
                let hex = |c: u8| (c as char).to_digit(16);
                if let (Some(h), Some(l)) = (hex(h), hex(l)) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

pub(crate) fn run_http_server(state: Arc<Mutex<RecordingState>>, port: u16) {
    let server = match tiny_http::Server::http(format!("127.0.0.1:{port}")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Не удалось занять порт {port}: {e}");
            return;
        }
    };
    people::migrate_voiceprints_v3();
    people::sync_voiceprint_people();
    println!("→ Rust-ядро слушает http://127.0.0.1:{port}");
    for request in server.incoming_requests() {
        // Каждый запрос — в своём потоке: SSE-стрим расшифровки, загрузка
        // гигабайтного файла и медленный клиент больше не замораживают
        // весь API (включая /api/status, который поллит UI).
        let state = state.clone();
        std::thread::spawn(move || {
            let mut request = request;
            let url = request.url().to_string();
            let method = request.method().clone();
            let (path, query) = match url.split_once('?') {
                Some((p, q)) => (p.to_string(), q.to_string()),
                None => (url.clone(), String::new()),
            };
            if !local_request_allowed(&request, port) {
                let resp = tiny_http::Response::from_string("Forbidden").with_status_code(403);
                let _ = request.respond(resp);
                return;
            }
            // CORS preflight: своих браузер не спрашивает, чужим отвечаем отказом.
            if method == tiny_http::Method::Options {
                let _ = request.respond(tiny_http::Response::empty(204));
                return;
            }
            if (method == tiny_http::Method::Get || method == tiny_http::Method::Head)
                && path.starts_with("/audio/")
            {
                serve_audio_file(request, &path);
                return;
            }
            match (method, path.as_str()) {
                (tiny_http::Method::Get, "/") | (tiny_http::Method::Get, "/index.html") => {
                    serve_static(request, "index.html")
                }
                // Любой файл из ui/web/static (css/*, js/*): имена проверяет static_body.
                (tiny_http::Method::Get, p)
                    if !p.starts_with("/api/") && !p.starts_with("/audio/") =>
                {
                    serve_static(request, p)
                }
                (tiny_http::Method::Get, "/api/status") => {
                    let st = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let elapsed = if st.is_recording {
                        now_secs().saturating_sub(st.start_time)
                    } else {
                        0
                    };
                    let dev = get_audio_devices_info();
                    let levels = st
                        .capture
                        .as_ref()
                        .and_then(|c| c.lock().ok())
                        .map(|s| (s.mic_level(), s.system_level()))
                        .unwrap_or((0.0, 0.0));
                    let mic_level = levels.0;
                    let sys_level = levels.1;
                    #[cfg(feature = "onnx")]
                    let transcribing = crate::transcribe::is_transcribe_running();
                    #[cfg(feature = "onnx")]
                    let transcribing_file = crate::transcribe::transcribing_file();
                    #[cfg(not(feature = "onnx"))]
                    let transcribing = false;
                    #[cfg(not(feature = "onnx"))]
                    let transcribing_file = String::new();
                    json_response(
                        request,
                        json!({
                            "recording": st.is_recording,
                            "transcribing": transcribing,
                            "transcribing_file": transcribing_file,
                            "elapsed": elapsed,
                            "current_file": st.current_file,
                            "mode": st.mode,
                            "device": dev,
                            "platform": std::env::consts::OS,
                            "recordings_dir": save_dir().to_string_lossy(),
                            "mic_level": mic_level,
                            "sys_level": sys_level,
                            "screen_recording": st.screen_session.is_some(),
                            "screen_supported": crate::screen::is_screen_capture_supported(),
                            "core": "rust",
                        }),
                        200,
                    );
                }
                (tiny_http::Method::Get, "/api/update/check") => {
                    handle_update_check(
                        request,
                        query_param(&query, "force").as_deref() == Some("1"),
                    );
                }
                (tiny_http::Method::Post, "/api/update/apply") => {
                    handle_update_apply(request);
                }
                (tiny_http::Method::Get, "/api/update/progress") => {
                    handle_update_progress(request);
                }
                (tiny_http::Method::Get, "/api/recordings") => {
                    json_response(request, json!({"recordings": recordings_list()}), 200);
                }
                (tiny_http::Method::Post, "/api/recordings/rename") => {
                    let body = read_body_to_json(&mut request);
                    let path = body.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    let blocked = {
                        let st = state.lock().unwrap_or_else(|e| e.into_inner());
                        st.is_recording && st.current_file.as_deref() == Some(path)
                    };
                    handle_recording_rename(request, body, blocked);
                }
                (tiny_http::Method::Post, "/api/recordings/delete") => {
                    let body = read_body_to_json(&mut request);
                    let path = body.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    let blocked = {
                        let st = state.lock().unwrap_or_else(|e| e.into_inner());
                        st.is_recording && st.current_file.as_deref() == Some(path)
                    };
                    handle_recording_delete(request, body, blocked);
                }
                (tiny_http::Method::Get, "/api/record/mode") => {
                    json_response(
                        request,
                        json!({"ok": true, "mode": configured_record_mode()}),
                        200,
                    );
                }
                (tiny_http::Method::Post, "/api/record/mode") => {
                    let body = read_body_to_json(&mut request);
                    match body.get("mode").and_then(|x| x.as_str()) {
                        Some(m @ ("split" | "mix")) => {
                            save_record_mode(m);
                            json_response(request, json!({"ok": true, "mode": m}), 200);
                        }
                        _ => json_response(
                            request,
                            json!({"ok": false, "error": "Режим: split или mix"}),
                            400,
                        ),
                    }
                }
                (tiny_http::Method::Post, "/api/log") => {
                    let mut s = String::new();
                    let _ = request.as_reader().read_to_string(&mut s);
                    eprintln!("[UI_LOG] {s}");
                    json_response(request, json!({"ok": true}), 200);
                }
                (tiny_http::Method::Get, "/api/transcribe/auto") => {
                    json_response(
                        request,
                        json!({"ok": true, "enabled": configured_auto_transcribe()}),
                        200,
                    );
                }
                (tiny_http::Method::Post, "/api/transcribe/auto") => {
                    let body = read_body_to_json(&mut request);
                    match body.get("enabled").and_then(|x| x.as_bool()) {
                        Some(enabled) => {
                            save_auto_transcribe(enabled);
                            json_response(request, json!({"ok": true, "enabled": enabled}), 200);
                        }
                        None => json_response(
                            request,
                            json!({"ok": false, "error": "Нужно поле enabled"}),
                            400,
                        ),
                    }
                }
                (tiny_http::Method::Get, "/api/people") => {
                    let mut people: Vec<serde_json::Value> = vec![];
                    let mut seen = std::collections::HashSet::new();
                    for pf in people_json_files() {
                        let domain = pf
                            .parent()
                            .and_then(|d| d.file_name())
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        for mut p in read_people_list(&pf).unwrap_or_default() {
                            let name = p
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                                continue;
                            }
                            if p.get("domain").is_none() {
                                p["domain"] = serde_json::json!(domain);
                            }
                            if p.get("is_external").is_none() {
                                p["is_external"] = serde_json::json!(false);
                            }
                            people.push(p);
                        }
                    }
                    for ext in load_external_people() {
                        let name = ext
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if name.is_empty() || !seen.insert(name.to_lowercase()) {
                            continue;
                        }
                        people.push(json!({"name": ext.get("name"), "role": ext.get("role").and_then(|x| x.as_str()).unwrap_or("Гость"), "is_external": true}));
                    }
                    let vp_list = load_all_voiceprints();
                    let mut ext_to_sync = Vec::new();
                    for vp in &vp_list {
                        let name = vp
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if name.is_empty() || !seen.insert(name.to_lowercase()) {
                            continue;
                        }
                        let item = json!({"name": name, "role": "Участник", "is_external": true});
                        people.push(item.clone());
                        ext_to_sync.push(item);
                    }
                    if !ext_to_sync.is_empty() {
                        let mut ext = load_external_people();
                        for item in ext_to_sync {
                            ext.push(item);
                        }
                        save_external_people(&ext);
                    }
                    let vp_names: Vec<String> = vp_list
                        .iter()
                        .filter_map(|p| {
                            p.get("name")
                                .and_then(|x| x.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect();
                    json_response(
                        request,
                        json!({"people": people, "external": load_external_people(), "voiceprints": vp_names}),
                        200,
                    );
                }
                (tiny_http::Method::Get, "/api/voiceprints") => {
                    let summary: Vec<serde_json::Value> = load_all_voiceprints().iter().map(|p| json!({
                    "name": p.get("name"),
                    "samples_count": p.get("samples_count").and_then(|x| x.as_u64()).unwrap_or(1),
                    "updated_at": p.get("updated_at"),
                })).collect();
                    json_response(request, json!({"ok": true, "voiceprints": summary}), 200);
                }
                (tiny_http::Method::Get, "/api/voicepacks") => {
                    let app = app_data_dir();
                    match packs::list_packs(&app) {
                        Ok(list) => json_response(request, json!({"ok": true, "packs": list}), 200),
                        Err(e) => json_response(
                            request,
                            json!({"ok": false, "error": e.to_string()}),
                            500,
                        ),
                    }
                }
                (tiny_http::Method::Get, "/api/models") => {
                    let app = app_data_dir();
                    let manifest = mr_models::load_bundled_manifest();
                    let status = mr_models::get_models_status(&app, &manifest);
                    json_response(request, json!({"ok": true, "models": status}), 200);
                }
                (tiny_http::Method::Get, "/api/support/info") => {
                    let cfg_path = app_data_dir().join("config.json");
                    let cfg: serde_json::Value = std::fs::read_to_string(&cfg_path)
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or(json!({}));
                    json_response(
                        request,
                        json!({
                            "ok": true,
                            "donation": {"title": "Поддержать развитие Open Source проекта", "methods": donation_methods()},
                            "stats": {
                                "transcribed_count": cfg.get("transcribed_count").and_then(|v| v.as_u64()).unwrap_or(0),
                                "total_saved_minutes": cfg.get("total_saved_minutes").and_then(|v| v.as_f64()).unwrap_or(0.0),
                                "donate_prompt_dismissed": cfg.get("donate_prompt_dismissed").and_then(|v| v.as_bool()).unwrap_or(false),
                            },
                        }),
                        200,
                    );
                }
                (tiny_http::Method::Post, "/api/support/dismiss") => {
                    let cfg_path = app_data_dir().join("config.json");
                    let mut cfg: serde_json::Value = std::fs::read_to_string(&cfg_path)
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or(json!({}));
                    cfg["donate_prompt_dismissed"] = json!(true);
                    config_lock(|| {
                        let _ = std::fs::create_dir_all(app_data_dir());
                        let _ = std::fs::write(
                            &cfg_path,
                            serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                        );
                    });
                    json_response(request, json!({"ok": true}), 200);
                }
                (tiny_http::Method::Post, "/api/people/add_external") => {
                    let body = read_body_to_json(&mut request);
                    let name = body
                        .get("name")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let role = body
                        .get("role")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Гость")
                        .trim()
                        .to_string();
                    if name.is_empty() {
                        json_response(
                            request,
                            json!({"ok": false, "error": "Имя не может быть пустым"}),
                            400,
                        );
                    } else {
                        let mut list = load_external_people();
                        if !list.iter().any(|p| {
                            p.get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_lowercase()
                                == name.to_lowercase()
                        }) {
                            list.push(json!({"name": name, "role": role, "is_external": true}));
                            save_external_people(&list);
                        }
                        json_response(request, json!({"ok": true}), 200);
                    }
                }
                (tiny_http::Method::Post, "/api/people/remove_external") => {
                    let body = read_body_to_json(&mut request);
                    let name = body
                        .get("name")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_lowercase();
                    let mut list = load_external_people();
                    let before = list.len();
                    list.retain(|p| {
                        p.get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_lowercase()
                            != name
                    });
                    if list.len() != before {
                        save_external_people(&list);
                    }
                    json_response(request, json!({"ok": true}), 200);
                }
                (tiny_http::Method::Post, "/api/people/update") => {
                    let body = read_body_to_json(&mut request);
                    let old = body
                        .get("old_name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let name = body
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let role = body
                        .get("role")
                        .and_then(|x| x.as_str())
                        .map(|s| s.trim().to_string());
                    if old.is_empty() || name.is_empty() {
                        json_response(
                            request,
                            json!({"ok": false, "error": "Имя не может быть пустым"}),
                            400,
                        );
                    } else {
                        let ok = update_people(&old, &name, role.as_deref());
                        json_response(
                            request,
                            json!({"ok": ok, "error": if ok { "" } else { "Участник не найден" } }),
                            if ok { 200 } else { 404 },
                        );
                    }
                }
                (tiny_http::Method::Post, "/api/people/remove") => {
                    let body = read_body_to_json(&mut request);
                    let name = body
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if name.is_empty() {
                        json_response(
                            request,
                            json!({"ok": false, "error": "Имя не может быть пустым"}),
                            400,
                        );
                    } else {
                        let ok = remove_person(&name);
                        json_response(
                            request,
                            json!({"ok": ok, "error": if ok { "" } else { "Участник не найден" } }),
                            if ok { 200 } else { 404 },
                        );
                    }
                }
                (tiny_http::Method::Post, "/api/voiceprints/save") => {
                    let body = read_body_to_json(&mut request);
                    handle_voiceprint_save(request, body);
                }
                (tiny_http::Method::Post, "/api/voiceprints/delete") => {
                    let body = read_body_to_json(&mut request);
                    handle_voiceprint_delete(request, body);
                }
                (tiny_http::Method::Post, "/api/voicepacks/toggle") => {
                    let body = read_body_to_json(&mut request);
                    let pack_id = body
                        .get("pack_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let enabled = body
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    match packs::set_pack_enabled(&app_data_dir(), &pack_id, enabled) {
                        Ok(_) => json_response(request, json!({"ok": true}), 200),
                        Err(e) => json_response(
                            request,
                            json!({"ok": false, "error": e.to_string()}),
                            400,
                        ),
                    }
                }
                (tiny_http::Method::Post, "/api/voicepacks/create") => {
                    let body = read_body_to_json(&mut request);
                    let name = body
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Новый пакет")
                        .trim()
                        .to_string();
                    match packs::create_pack(&app_data_dir(), &name, "") {
                        Ok(info) => json_response(request, json!({"ok": true, "pack": info}), 200),
                        Err(e) => json_response(
                            request,
                            json!({"ok": false, "error": e.to_string()}),
                            500,
                        ),
                    }
                }
                (tiny_http::Method::Post, "/api/voicepacks/export") => {
                    let body = read_body_to_json(&mut request);
                    let pack_id = body
                        .get("pack_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let out = save_dir().join(format!("{pack_id}.voicepack.zip"));
                    match packs::export_pack(&app_data_dir(), &pack_id, &out, false) {
                        Ok(p) => json_response(
                            request,
                            json!({"ok": true, "path": p.to_string_lossy()}),
                            200,
                        ),
                        Err(e) => json_response(
                            request,
                            json!({"ok": false, "error": e.to_string()}),
                            500,
                        ),
                    }
                }
                (tiny_http::Method::Post, "/api/media/extract_audio") => {
                    let body = read_body_to_json(&mut request);
                    handle_extract_audio(request, body);
                }
                (tiny_http::Method::Get, "/api/media/snippet") => {
                    serve_media_snippet(request, &query);
                }
                (tiny_http::Method::Post, "/api/upload") => {
                    handle_upload(request);
                }
                (tiny_http::Method::Post, "/api/record/start") => {
                    #[cfg(feature = "onnx")]
                    let transcribing = crate::transcribe::is_transcribe_running();
                    #[cfg(not(feature = "onnx"))]
                    let transcribing = false;
                    if transcribing {
                        json_response(
                            request,
                            json!({"ok": false, "error": "Дождитесь окончания расшифровки"}),
                            409,
                        );
                    } else {
                        let body = read_body_to_json(&mut request);
                        let mode = body.get("mode").and_then(|m| m.as_str()).unwrap_or("split");
                        let screen = body
                            .get("screen")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                        let res = start_recording_inner(&mut st, mode, screen);
                        if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                            save_record_mode(mode);
                        }
                        json_response(request, res, 200);
                    }
                }
                (tiny_http::Method::Post, "/api/record/stop") => {
                    let body = read_body_to_json(&mut request);
                    let defer = body
                        .get("defer_autotranscribe")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                    let res = stop_recording_inner(&mut st, defer);
                    json_response(request, res, 200);
                }
                (tiny_http::Method::Post, "/api/recording/attach_video") => {
                    handle_attach_video(request);
                }
                (tiny_http::Method::Get, "/api/transcript") => {
                    serve_transcript(request, &query);
                }
                (tiny_http::Method::Post, "/api/transcript/edit") => {
                    let body = read_body_to_json(&mut request);
                    handle_transcript_edit(request, body);
                }
                (tiny_http::Method::Post, "/api/export/save_dialog") => {
                    handle_save_dialog(request, &query);
                }
                (tiny_http::Method::Get, "/api/transcribe_stream") => {
                    serve_transcribe_stream(request);
                }
                (tiny_http::Method::Post, "/api/transcribe/start") => {
                    let recording = {
                        let st = state.lock().unwrap_or_else(|e| e.into_inner());
                        st.is_recording
                    };
                    if recording {
                        json_response(
                            request,
                            json!({"ok": false, "error": "Сначала остановите запись"}),
                            409,
                        );
                    } else {
                        let body = read_body_to_json(&mut request);
                        handle_transcribe_start(request, body);
                    }
                }
                (tiny_http::Method::Post, "/api/models/update") => {
                    let body = read_body_to_json(&mut request);
                    handle_models_update(request, body);
                }
                _ => {
                    let resp = tiny_http::Response::from_string("Not Found").with_status_code(404);
                    let _ = request.respond(resp);
                }
            }
        });
    }
}

// Полные реализации подключаются за cargo-фичей `tauri-ui` (Tauri v2 плагины).
// Здесь — кроссплатформенные файловые реализации, достаточные для прод-использования
// без WebView: одиночный экземпляр через TCP-сокет, автозапуск через .desktop/LaunchAgent.
