use std::io::Read;

use serde_json::{json, Value};

use super::{json_response, media, query_param};
use crate::summary::{self, ApiError, GenerateInput, StartOutcome};

const MAX_SUMMARY_BODY: u64 = 64 * 1024;

fn error_response(request: tiny_http::Request, error: ApiError) {
    json_response(
        request,
        json!({"ok": false, "code": error.code, "error": error.message}),
        error.status,
    );
}

fn read_json(request: &mut tiny_http::Request) -> Result<Value, ApiError> {
    // У итогов тело — только текст расшифровки, 64 КиБ хватает; общий лимит
    // 16 МиБ ради них не поднимаем.
    if let Some(length) = media::header_value(request, "Content-Length") {
        if length
            .trim()
            .parse::<u64>()
            .map(|n| n > MAX_SUMMARY_BODY)
            .unwrap_or(false)
        {
            return Err(ApiError::new(
                413,
                "body_too_large",
                "Слишком большой запрос. Повторите.",
            ));
        }
    }
    let mut buf = Vec::new();
    let _ = request
        .as_reader()
        .take(MAX_SUMMARY_BODY + 1)
        .read_to_end(&mut buf);
    if buf.len() as u64 > MAX_SUMMARY_BODY {
        return Err(ApiError::new(
            413,
            "body_too_large",
            "Слишком большой запрос. Повторите.",
        ));
    }
    if buf.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&buf)
        .map_err(|_| ApiError::new(400, "invalid_request", "Некорректный запрос итогов."))
}

/// Android: 501, никогда 404 — клиент отличает неподдерживаемую функцию от битого URL.
pub(super) fn unsupported(request: tiny_http::Request) {
    json_response(
        request,
        json!({
            "ok": false,
            "code": "summary_unsupported",
            "error": "Итоги на этом устройстве не поддерживаются.",
        }),
        501,
    );
}

pub(super) fn dispatch(
    mut request: tiny_http::Request,
    method: &tiny_http::Method,
    path: &str,
    query: &str,
    recording: bool,
) {
    if !summary::supported() {
        unsupported(request);
        return;
    }
    let get = matches!(method, tiny_http::Method::Get);
    let post = matches!(method, tiny_http::Method::Post);
    let _: Result<(), ApiError> = match (method, path) {
        (tiny_http::Method::Get, "/api/summary") => {
            let Some(file) = query_param(query, "file") else {
                error_response(request, ApiError::new(400, "no_file", "Не указан файл."));
                return;
            };
            match summary::get_view(&file, recording) {
                Ok(view) => json_response(request, merge_ok(view), 200),
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/classify") => {
            match read_json(&mut request) {
                Ok(body) => {
                    let file = body.get("file").and_then(Value::as_str).unwrap_or("");
                    let force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
                    match summary::classify_view(file, force) {
                        Ok(view) => {
                            json_response(request, merge_ok(view), 200);
                        }
                        Err(error) => error_response(request, error),
                    }
                }
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/generate") => {
            match read_json(&mut request) {
                Ok(body) => {
                    let settings = summary::config::load();
                    let input = GenerateInput {
                        file: body
                            .get("file")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        preset_id: body
                            .get("preset_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        detail: body
                            .get("detail")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| settings.detail.as_str().to_string()),
                        source_rev: body
                            .get("source_rev")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        force: body.get("force").and_then(Value::as_bool).unwrap_or(false),
                        confirm_sensitive: body
                            .get("confirm_sensitive")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        allow_downgrade: body
                            .get("allow_downgrade")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    };
                    match summary::generate(&input, recording) {
                        Ok(StartOutcome::Started { job_id }) => {
                            json_response(
                                request,
                                json!({"ok": true, "job_id": job_id, "started": true}),
                                202,
                            );
                        }
                        Ok(StartOutcome::Cached { summary_id }) => {
                            json_response(
                                request,
                                json!({"ok": true, "cached": true, "summary_id": summary_id}),
                                200,
                            );
                        }
                        Err(error) => error_response(request, error),
                    }
                }
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/cancel") => {
            match read_json(&mut request) {
                Ok(body) => {
                    let job_id = body.get("job_id").and_then(Value::as_str).unwrap_or("");
                    if summary::cancel(job_id) {
                        json_response(request, json!({"ok": true, "cancelled": true}), 200);
                    } else {
                        error_response(
                            request,
                            ApiError::new(404, "unknown_job", "Задача не найдена."),
                        );
                    }
                }
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Get, "/api/summary/presets") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            let view = summary::presets_json();
            json_response(
                request,
                json!({
                    "ok": true,
                    "catalog_version": view["catalog_version"],
                    "presets": view["presets"],
                    "system_prompt_base": view["system_prompt_base"],
                }),
                200,
            );
            Ok(())
        }
        (tiny_http::Method::Get, "/api/summary/gpu") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            json_response(request, merge_ok(summary::gpu_status()), 200);
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/gpu/install") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            match summary::gpu_install() {
                Ok(()) => {
                    json_response(request, json!({"ok": true, "started": true}), 202);
                }
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/gpu/cancel") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            json_response(
                request,
                json!({"ok": true, "cancelled": summary::gpu_cancel()}),
                200,
            );
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/gpu/remove") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            match summary::gpu_remove() {
                Ok(view) => json_response(request, merge_ok(view), 200),
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        (tiny_http::Method::Get, "/api/summary/settings") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            json_response(request, merge_ok(summary::settings_view()), 200);
            Ok(())
        }
        (tiny_http::Method::Post, "/api/summary/settings") => {
            if let Err(error) = summary::ensure_enabled() {
                error_response(request, error);
                return;
            }
            match read_json(&mut request) {
                Ok(body) => match summary::update_settings(&body) {
                    Ok(view) => json_response(request, merge_ok(view), 200),
                    Err(error) => error_response(request, error),
                },
                Err(error) => error_response(request, error),
            }
            Ok(())
        }
        _ if path == "/api/summary" || path.starts_with("/api/summary/") => {
            if get || post {
                error_response(
                    request,
                    ApiError::new(404, "not_found", "Маршрут итогов не найден."),
                );
            } else {
                error_response(
                    request,
                    ApiError::new(405, "method_not_allowed", "Метод не поддерживается."),
                );
            }
            Ok(())
        }
        _ => {
            error_response(
                request,
                ApiError::new(404, "not_found", "Маршрут итогов не найден."),
            );
            Ok(())
        }
    };
}

fn merge_ok(view: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("ok".to_string(), Value::Bool(true));
    if let Value::Object(fields) = view {
        map.extend(fields);
    }
    Value::Object(map)
}
