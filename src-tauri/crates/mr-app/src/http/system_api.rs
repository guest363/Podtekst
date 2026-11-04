use serde_json::json;

use super::json_response;

pub(crate) fn handle_system(
    request: tiny_http::Request,
    recording: bool,
    elapsed: u64,
    mode: &str,
) {
    json_response(
        request,
        json!({
            "ok": true,
            "system": crate::system::snapshot(recording, elapsed, mode),
        }),
        200,
    );
}

pub(crate) fn handle_system_check(
    request: tiny_http::Request,
    body: serde_json::Value,
    recording: bool,
) {
    let probe_audio = body
        .get("probe_audio")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let include_optional = body
        .get("include_optional")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    json_response(
        request,
        crate::system::check(probe_audio, recording, include_optional),
        200,
    );
}

pub(crate) fn handle_system_open(request: tiny_http::Request, body: serde_json::Value) {
    let target = body.get("target").and_then(|x| x.as_str()).unwrap_or("");
    match crate::system::open_target(target) {
        Ok(()) => json_response(request, json!({"ok": true}), 200),
        Err(e) => json_response(request, json!({"ok": false, "error": e}), 400),
    }
}
