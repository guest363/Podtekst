//! Загрузка, переименование и извлечение аудио из видео.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::json_response;
use super::media::{header_value, inside_save_dir, is_media_ext, safe_file_name};
use crate::config::save_dir;

pub(super) fn handle_recording_rename(
    request: tiny_http::Request,
    body: serde_json::Value,
    blocked: bool,
) {
    if blocked {
        json_response(
            request,
            json!({"ok": false, "error": "Сначала остановите запись"}),
            409,
        );
        return;
    }
    let old = body
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if old.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Не указан файл"}),
            400,
        );
        return;
    }
    let old_path = PathBuf::from(&old);
    if !old_path.is_file() || !inside_save_dir(&old_path) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл не найден в папке записей"}),
            404,
        );
        return;
    }
    let raw = body.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let clean: String = raw
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
        .collect();
    let stem = clean.trim().trim_matches('.').trim().to_string();
    if stem.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Имя не может быть пустым"}),
            400,
        );
        return;
    }
    let ext = old_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "wav".to_string());
    let lower = stem.to_lowercase();
    let new_stem = match lower.rsplit_once('.') {
        Some((base, tail)) if is_media_ext(tail) => base.to_string(),
        _ => stem.clone(),
    };
    if new_stem.trim().is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Имя не может быть пустым"}),
            400,
        );
        return;
    }
    let new_name = format!("{new_stem}.{ext}");
    let dir = old_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir);
    let new_path = dir.join(&new_name);
    if new_path == old_path {
        json_response(
            request,
            json!({"ok": true, "path": old, "name": old_path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}),
            200,
        );
        return;
    }
    if new_path.exists() {
        json_response(
            request,
            json!({"ok": false, "error": "Файл с таким именем уже есть"}),
            409,
        );
        return;
    }
    if std::fs::rename(&old_path, &new_path).is_err() {
        json_response(
            request,
            json!({"ok": false, "error": "Не удалось переименовать файл"}),
            500,
        );
        return;
    }
    let old_stem = old_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for suffix in [".transcript.json", ".md", ".txt"] {
        let from = dir.join(format!("{old_stem}{suffix}"));
        if from.exists() {
            let _ = std::fs::rename(&from, dir.join(format!("{new_stem}{suffix}")));
        }
    }
    json_response(
        request,
        json!({"ok": true, "path": new_path.to_string_lossy(), "name": new_name}),
        200,
    );
}

/// POST /api/recordings/delete: удаляет медиафайл и его расшифровки
/// (`.transcript.json`, `.md`, `.txt`) рядом с исходником.
pub(super) fn handle_recording_delete(
    request: tiny_http::Request,
    body: serde_json::Value,
    blocked: bool,
) {
    if blocked {
        json_response(
            request,
            json!({"ok": false, "error": "Сначала остановите запись"}),
            409,
        );
        return;
    }
    let path = body
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if path.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Не указан файл"}),
            400,
        );
        return;
    }
    let p = PathBuf::from(&path);
    if !p.is_file() || !inside_save_dir(&p) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл не найден в папке записей"}),
            404,
        );
        return;
    }
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_else(save_dir);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut deleted = vec![p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()];
    let _ = std::fs::remove_file(&p);
    for suffix in [".transcript.json", ".md", ".txt"] {
        let side = dir.join(format!("{stem}{suffix}"));
        if side.exists() && std::fs::remove_file(&side).is_ok() {
            deleted.push(
                side.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
        }
    }
    json_response(request, json!({"ok": true, "deleted": deleted}), 200);
}

/// POST /api/upload: сырое тело + заголовок X-Filename.
/// Пишет потоком в `.part` с переименованием; лимит 4 ГБ против OOM.
pub(super) fn handle_upload(mut request: tiny_http::Request) {
    const MAX_UPLOAD: u64 = 4 * 1024 * 1024 * 1024;
    if let Some(cl) = header_value(&request, "Content-Length") {
        if cl
            .trim()
            .parse::<u64>()
            .map(|n| n > MAX_UPLOAD)
            .unwrap_or(false)
        {
            json_response(
                request,
                json!({"ok": false, "error": "Файл больше 4 ГБ. Разбейте его на части."}),
                413,
            );
            return;
        }
    }
    let raw = header_value(&request, "X-Filename").unwrap_or_else(|| "uploaded_media".to_string());
    let fname = safe_file_name(raw.trim());
    let dir = save_dir();
    let _ = std::fs::create_dir_all(&dir);
    let dest = crate::recording::unique_media_path(&dir, &fname);
    let part_name = format!(
        ".{}.part",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("upload")
    );
    let part = dir.join(part_name);
    let mut out = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(e) => {
            json_response(
                request,
                json!({"ok": false, "error": format!("Не удалось сохранить файл: {e}")}),
                500,
            );
            return;
        }
    };
    let mut reader = request.as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        match std::io::Read::read(&mut reader, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > MAX_UPLOAD {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    json_response(
                        request,
                        json!({"ok": false, "error": "Файл больше 4 ГБ. Разбейте его на части."}),
                        413,
                    );
                    return;
                }
                if std::io::Write::write_all(&mut out, &buf[..n]).is_err() {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    json_response(
                        request,
                        json!({"ok": false, "error": "Не удалось записать файл на диск"}),
                        500,
                    );
                    return;
                }
            }
            Err(_) => {
                drop(out);
                let _ = std::fs::remove_file(&part);
                json_response(
                    request,
                    json!({"ok": false, "error": "Не удалось прочитать загруженный файл"}),
                    400,
                );
                return;
            }
        }
    }
    drop(out);
    if total == 0 {
        let _ = std::fs::remove_file(&part);
        json_response(
            request,
            json!({"ok": false, "error": "Файл пуст. Загрузите непустой файл."}),
            400,
        );
        return;
    }
    if std::fs::rename(&part, &dest).is_err() {
        let _ = std::fs::remove_file(&part);
        json_response(
            request,
            json!({"ok": false, "error": "Не удалось сохранить файл"}),
            500,
        );
        return;
    }
    json_response(
        request,
        json!({"ok": true, "path": dest.to_string_lossy(), "name": dest.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}),
        200,
    );
}
pub(super) fn handle_extract_audio(request: tiny_http::Request, body: serde_json::Value) {
    let media = body
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let codec = body
        .get("codec")
        .and_then(|v| v.as_str())
        .unwrap_or("mp3")
        .to_string();
    if media.is_empty() || !Path::new(&media).exists() {
        json_response(
            request,
            json!({"ok": false, "error": "Видеофайл не найден. Проверьте путь и повторите."}),
            400,
        );
        return;
    }
    if !inside_save_dir(Path::new(&media)) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл вне папки записей"}),
            400,
        );
        return;
    }
    let src = PathBuf::from(&media);
    let out = src.parent().unwrap_or(Path::new(".")).join(format!(
        "{}.{}",
        src.file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
            .as_ref(),
        if codec == "wav" { "wav" } else { "mp3" }
    ));
    let args: Vec<String> = if codec == "wav" {
        vec![
            "-y".into(),
            "-i".into(),
            media.clone(),
            "-vn".into(),
            "-ac".into(),
            "1".into(),
            "-ar".into(),
            "16000".into(),
            out.to_string_lossy().to_string(),
        ]
    } else {
        vec![
            "-y".into(),
            "-i".into(),
            media.clone(),
            "-vn".into(),
            "-codec:a".into(),
            "libmp3lame".into(),
            "-q:a".into(),
            "4".into(),
            out.to_string_lossy().to_string(),
        ]
    };
    match std::process::Command::new("ffmpeg").args(&args).output() {
        Ok(o) if o.status.success() && out.exists() => json_response(
            request,
            json!({"ok": true, "path": out.to_string_lossy(), "name": out.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}),
            200,
        ),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr)
                .chars()
                .take(300)
                .collect::<String>();
            json_response(
                request,
                json!({"ok": false, "error": format!("ffmpeg: {}", err.trim())}),
                500,
            )
        }
        Err(e) => json_response(
            request,
            json!({"ok": false, "error": format!("ffmpeg не найден: {e}")}),
            500,
        ),
    }
}
