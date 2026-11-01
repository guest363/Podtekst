use std::path::{Path, PathBuf};

use serde_json::json;

use super::json_response;
use super::media::{
    header_value, inside_save_dir, is_media_ext, is_video_path, safe_file_name, video_supported,
};
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
    let was_owned = crate::media_registry::load_registry().is_owned(&old_path);
    if std::fs::rename(&old_path, &new_path).is_err() {
        json_response(
            request,
            json!({"ok": false, "error": "Не удалось переименовать файл"}),
            500,
        );
        return;
    }
    crate::media_registry::path_renamed(&old_path, &new_path);
    // Старые записи узнаём по имени файла: после переименования шаблон уже
    // не совпадёт, поэтому фиксируем путь в реестре явно.
    if was_owned {
        crate::media_registry::mark_owned(&new_path);
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
    // Файл, который приложение не создавало (положили в папку вручную или
    // принесли извне), не удаляем: убираем только запись из списка.
    if !crate::media_registry::load_registry().is_owned(&p) {
        crate::media_registry::hide(&p);
        json_response(
            request,
            json!({
                "ok": true,
                "kept": true,
                "deleted": [],
                "name": p.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
            }),
            200,
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
    crate::media_registry::unmark_owned(&p);
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
    if !video_supported() && is_video_path(Path::new(&fname)) {
        json_response(
            request,
            json!({"ok": false, "error": "На Android видео не поддерживается. Выберите аудиофайл (MP3, WAV, M4A, OGG, FLAC)."}),
            400,
        );
        return;
    }
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
    crate::media_registry::mark_imported(&dest);
    json_response(
        request,
        json!({"ok": true, "path": dest.to_string_lossy(), "name": dest.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}),
        200,
    );
}
pub(super) fn handle_extract_audio(request: tiny_http::Request, body: serde_json::Value) {
    if !video_supported() {
        json_response(
            request,
            json!({"ok": false, "error": "Извлечение звука из видео недоступно на Android"}),
            501,
        );
        return;
    }
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
        Ok(o) if o.status.success() && out.exists() => {
            crate::media_registry::mark_owned(&out);
            json_response(
                request,
                json!({"ok": true, "path": out.to_string_lossy(), "name": out.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()}),
                200,
            )
        }
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

pub(super) fn handle_attach_video(mut request: tiny_http::Request) {
    const MAX_UPLOAD: u64 = 4 * 1024 * 1024 * 1024;
    let audio_path_raw = header_value(&request, "X-Audio-Path").unwrap_or_default();
    if audio_path_raw.trim().is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Не указан путь к аудиофайлу"}),
            400,
        );
        return;
    }
    let audio_path = PathBuf::from(audio_path_raw.trim());
    let audio_path = if audio_path.is_absolute() {
        audio_path
    } else {
        save_dir().join(audio_path)
    };
    if !audio_path.is_file() || !inside_save_dir(&audio_path) {
        json_response(
            request,
            json!({"ok": false, "error": "Аудиозапись не найдена в папке записей"}),
            404,
        );
        return;
    }

    let ctype = header_value(&request, "Content-Type").unwrap_or_default();
    let is_mp4 = ctype.contains("mp4");
    let video_ext = if is_mp4 { "mp4" } else { "webm" };

    let dir = audio_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir);
    let stem = audio_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "record".to_string());
    let temp_video = dir.join(format!(".{stem}.video_in.{video_ext}"));

    let mut out = match std::fs::File::create(&temp_video) {
        Ok(f) => f,
        Err(e) => {
            json_response(
                request,
                json!({"ok": false, "error": format!("Не удалось создать временный файл: {e}")}),
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
                    let _ = std::fs::remove_file(&temp_video);
                    json_response(
                        request,
                        json!({"ok": false, "error": "Видео больше 4 ГБ"}),
                        413,
                    );
                    return;
                }
                if std::io::Write::write_all(&mut out, &buf[..n]).is_err() {
                    drop(out);
                    let _ = std::fs::remove_file(&temp_video);
                    json_response(
                        request,
                        json!({"ok": false, "error": "Не удалось записать видео на диск"}),
                        500,
                    );
                    return;
                }
            }
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&temp_video);
                json_response(
                    request,
                    json!({"ok": false, "error": format!("Ошибка чтения потока: {e}")}),
                    500,
                );
                return;
            }
        }
    }
    drop(out);

    if total < 250 {
        let _ = std::fs::remove_file(&temp_video);
        eprintln!("[ATTACH_VIDEO] Видеопоток слишком мал ({total} байт), отклонён как пустой");
        json_response(
            request,
            json!({"ok": false, "error": "Видеопоток пуст или не содержит видеокадров"}),
            400,
        );
        return;
    }

    eprintln!("[ATTACH_VIDEO] Получен видеопоток: {total} байт для аудио {audio_path:?}");

    let final_video = crate::recording::unique_media_path(&dir, &format!("{stem}.{video_ext}"));
    let ffmpeg_available = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !ffmpeg_available {
        let companion =
            crate::recording::unique_media_path(&dir, &format!("{stem}.video.{video_ext}"));
        let _ = std::fs::rename(&temp_video, &companion);
        crate::media_registry::mark_owned(&companion);
        crate::recording::maybe_autotranscribe(&audio_path.to_string_lossy());
        json_response(
            request,
            json!({
                "ok": true,
                "path": companion.to_string_lossy(),
                "filename": companion.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
                "is_video": true,
                "warning": "ffmpeg_not_found",
                "standalone_audio": audio_path.to_string_lossy()
            }),
            200,
        );
        return;
    }

    let run_ffmpeg = |extra_args: &[&str]| -> std::io::Result<std::process::Output> {
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-i")
            .arg(&temp_video)
            .arg("-i")
            .arg(&audio_path)
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("1:a:0");
        cmd.args(extra_args);
        cmd.arg(&final_video);
        cmd.output()
    };

    let copy_args = if is_mp4 {
        vec!["-c:v", "copy", "-c:a", "aac", "-shortest"]
    } else {
        vec!["-c:v", "copy", "-c:a", "libopus", "-shortest"]
    };

    let mut success = false;
    let mut last_err = String::new();

    match run_ffmpeg(&copy_args) {
        Ok(o) if o.status.success() && final_video.exists() => {
            success = true;
        }
        Ok(o) => {
            last_err = String::from_utf8_lossy(&o.stderr)
                .chars()
                .take(300)
                .collect::<String>();
            eprintln!("ffmpeg copy mux failed: {}", last_err.trim());
        }
        Err(e) => {
            last_err = e.to_string();
            eprintln!("ffmpeg execution error: {e}");
        }
    }

    if !success {
        let transcode_args = if is_mp4 {
            vec![
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "23",
                "-c:a",
                "aac",
                "-shortest",
            ]
        } else {
            vec![
                "-c:v",
                "libvpx",
                "-crf",
                "30",
                "-b:v",
                "2M",
                "-c:a",
                "libopus",
                "-shortest",
            ]
        };
        match run_ffmpeg(&transcode_args) {
            Ok(o) if o.status.success() && final_video.exists() => {
                success = true;
            }
            Ok(o) => {
                last_err = String::from_utf8_lossy(&o.stderr)
                    .chars()
                    .take(300)
                    .collect::<String>();
                eprintln!("ffmpeg transcode mux failed: {}", last_err.trim());
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }

    let _ = std::fs::remove_file(&temp_video);

    if success {
        eprintln!("[ATTACH_VIDEO] Склейка успешна -> {final_video:?}");
        let _ = std::fs::remove_file(&audio_path);
        crate::media_registry::unmark_owned(&audio_path);
        crate::media_registry::mark_owned(&final_video);
        crate::recording::maybe_autotranscribe(&final_video.to_string_lossy());
        json_response(
            request,
            json!({
                "ok": true,
                "path": final_video.to_string_lossy(),
                "filename": final_video.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
                "is_video": true
            }),
            200,
        );
    } else {
        eprintln!("[ATTACH_VIDEO] Ошибка склейки ffmpeg: {last_err}");
        json_response(
            request,
            json!({
                "ok": false,
                "error": format!("ffmpeg: {}", last_err.trim()),
                "audio_path": audio_path.to_string_lossy()
            }),
            500,
        );
    }
}
