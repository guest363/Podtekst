use serde_json::{json, Value};

use super::json_response;

pub(crate) fn handle_journal(request: tiny_http::Request, limit: usize) {
    let sessions = crate::journal::list(limit.clamp(1, 100));
    json_response(
        request,
        json!({
            "ok": true,
            "sessions": sessions,
            "total": sessions.len(),
            "truncated": false,
        }),
        200,
    );
}

pub(crate) fn handle_crashes(request: tiny_http::Request) {
    let items: Vec<Value> = crate::crash::list().into_iter().map(flatten).collect();
    let unacknowledged = items
        .iter()
        .filter(|item| item["acknowledged"] == false)
        .count();
    let last_at = items
        .first()
        .and_then(|item| item.get("created_at"))
        .cloned()
        .unwrap_or(Value::Null);
    json_response(
        request,
        json!({
            "ok": true,
            "items": items,
            "unacknowledged": unacknowledged,
            "session_count": 0,
            "last_at": last_at,
        }),
        200,
    );
}

pub(crate) fn handle_crash_ack(request: tiny_http::Request, body: &Value) {
    if body.get("all").and_then(Value::as_bool) == Some(true) {
        crate::crash::ack_all();
    } else if let Some(id) = body.get("id").and_then(Value::as_str) {
        crate::crash::ack(id);
    }
    json_response(request, json!({"ok": true}), 200);
}

fn flatten(value: Value) -> Value {
    let backtrace_empty = value
        .get("backtrace")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    json!({
        "id": value.get("id").cloned().unwrap_or(Value::Null),
        "created_at": value.get("created_at").cloned().unwrap_or(Value::Null),
        "kind": value.get("kind").cloned().unwrap_or(Value::Null),
        "synthetic": value.get("synthetic").cloned().unwrap_or(json!(false)),
        "app_version": value.pointer("/app/version").cloned().unwrap_or(Value::Null),
        "thread": value.get("thread").cloned().unwrap_or(Value::Null),
        "message": value.pointer("/panic/message").cloned().unwrap_or(Value::Null),
        "location": value.pointer("/panic/location").cloned().unwrap_or(Value::Null),
        "has_backtrace": !backtrace_empty,
        "acknowledged": value.get("acked").cloned().unwrap_or(json!(false)),
    })
}
