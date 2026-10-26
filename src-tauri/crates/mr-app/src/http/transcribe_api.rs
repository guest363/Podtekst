//! Расшифровка: SSE-стрим событий, запуск воркера, скачивание моделей.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::media::{inside_save_dir, is_video_path, video_supported};
use super::{json_response, query_param};
use crate::config::{app_data_dir, save_dir};

pub(super) fn serve_transcript(request: tiny_http::Request, query: &str) {
    let Some(f) = query_param(query, "file") else {
        json_response(request, json!({"error": "No file specified"}), 400);
        return;
    };
    // Относительное имя резолвим от папки записей ДО проверок и чтения:
    // иначе голое имя файла читалось бы из CWD процесса.
    let p = {
        let raw = PathBuf::from(&f);
        if raw.is_absolute() {
            raw
        } else {
            save_dir().join(raw)
        }
    };
    if p.file_name().is_none() || !inside_save_dir(&p) {
        json_response(request, json!({"error": "Файл вне папки записей"}), 404);
        return;
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = p.parent().map(|d| d.to_path_buf()).unwrap_or_else(save_dir);
    let json_p = parent.join(format!("{stem}.transcript.json"));
    let md_p = parent.join(format!("{stem}.md"));
    let mut meta = serde_json::json!({});
    let mut speakers_info = serde_json::json!([]);
    let mut utterances = serde_json::json!([]);
    if let Ok(text) = std::fs::read_to_string(&json_p) {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
            meta = data.get("meta").cloned().unwrap_or(json!({}));
            speakers_info = meta.get("speakers_info").cloned().unwrap_or(json!([]));
            // Полный список реплик без лимита: обрезка «первые 200» теряла
            // хвост длинных встреч в UI.
            utterances = data.get("utterances").cloned().unwrap_or(json!([]));
        }
    }
    let md_text = std::fs::read_to_string(&md_p).unwrap_or_default();
    json_response(
        request,
        json!({
            "filename": p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "meta": meta,
            "speakers_info": speakers_info,
            "utterances": utterances,
            "markdown": md_text,
        }),
        200,
    );
}

/// SSE-стрим событий транскрибации/моделей (совместим с EventSource в Web UI).
///
/// Возобновление по стандартному заголовку `Last-Event-ID` (браузер шлёт его сам
/// после обрыва связи): клиент продолжает с места разрыва. Новое подключение без
/// курсора получает только новые события — проигрывание всей истории повторяло
/// пользователю устаревшие ошибки и чужие «модель готова».
pub(super) fn serve_transcribe_stream(request: tiny_http::Request) {
    #[cfg(not(feature = "onnx"))]
    {
        let _ = request.respond(tiny_http::Response::from_string(
            "data: {\"type\":\"error\",\"message\":\"соберите с --features onnx\"}\n\n",
        ));
    }
    // Ответ пишем сами: tiny_http копит тело в BufWriter на 1 КБ и отдаёт его
    // клиенту только по заполнении — SSE-события опаздывали на десятки секунд,
    // и прогресс расшифровки выглядел зависшим. Здесь буфер сбрасывается
    // после каждого пакета.
    #[cfg(feature = "onnx")]
    {
        use std::io::Write;
        let mut since = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Last-Event-ID"))
            .and_then(|h| h.value.as_str().trim().parse::<usize>().ok())
            .unwrap_or_else(|| {
                // Без курсора — только будущее: история могла относиться к прошлым запускам.
                crate::transcribe::take_new_events(usize::MAX).1
            });
        let mut out = request.into_writer();
        if out
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\n\
                  Cache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
            )
            .and_then(|_| out.flush())
            .is_err()
        {
            return;
        }
        loop {
            let (fresh, total, running) = crate::transcribe::take_new_events(since);
            let mut buf = String::new();
            for (i, e) in fresh.iter().enumerate() {
                // Абсолютный id события: браузер подставит его в Last-Event-ID при реконнекте.
                buf.push_str(&format!(
                    "id: {}\ndata: {}\n\n",
                    since + i,
                    serde_json::to_string(e).unwrap_or_default()
                ));
            }
            since = total;
            if (!buf.is_empty() && out.write_all(buf.as_bytes()).is_err()) || out.flush().is_err() {
                return;
            }
            if !running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

#[cfg(feature = "onnx")]
pub(super) fn handle_transcribe_start(request: tiny_http::Request, body: serde_json::Value) {
    let audio_file = body
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let engine = body
        .get("engine")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string();
    // Язык нужен только Whisper; «auto» (по умолчанию) — авторегулирование модели.
    let language = body
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .trim()
        .to_lowercase();
    let my_name = body
        .get("my_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Я")
        .to_string();
    if audio_file.is_empty() || !PathBuf::from(&audio_file).exists() {
        json_response(
            request,
            json!({"ok": false, "error": "Аудиофайл не найден. Проверьте путь и повторите."}),
            400,
        );
        return;
    }
    // Транскрибируем только записи из папки записей: результат (md/json)
    // пишется рядом с исходником — произвольный путь означал бы запись на диск.
    if !inside_save_dir(Path::new(&audio_file)) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл вне папки записей"}),
            400,
        );
        return;
    }
    if !video_supported() && is_video_path(Path::new(&audio_file)) {
        json_response(
            request,
            json!({"ok": false, "error": "На Android видео не поддерживается. Импортируйте аудиофайл."}),
            400,
        );
        return;
    }
    if crate::transcribe::is_transcribe_running() {
        json_response(
            request,
            json!({"ok": false, "error": "Расшифровка уже выполняется"}),
            409,
        );
        return;
    }
    let opts = crate::transcribe::TranscribeOptions {
        speaker_left: my_name,
        speaker_right: "Собеседник".to_string(),
        engine,
        language,
        min_speakers: 0,
        max_speakers: 0,
    };
    if crate::transcribe::spawn_transcription_worker(PathBuf::from(audio_file), opts) {
        json_response(
            request,
            json!({"ok": true, "message": "Расшифровка запущена"}),
            200,
        );
    } else {
        json_response(
            request,
            json!({"ok": false, "error": "Расшифровка уже выполняется"}),
            409,
        );
    }
}

#[cfg(not(feature = "onnx"))]
pub(super) fn handle_transcribe_start(request: tiny_http::Request, _body: serde_json::Value) {
    json_response(
        request,
        json!({"ok": false, "error": "Расшифровка недоступна: ядро собрано без --features onnx"}),
        501,
    );
}

pub(super) fn handle_models_update(request: tiny_http::Request, body: serde_json::Value) {
    let model_id = body
        .get("model_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if model_id.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "model_id не указан"}),
            400,
        );
        return;
    }
    let reply_id = model_id.clone();
    // Стартовое событие (и флаг «модель качается») — синхронно до ответа:
    // иначе EventSource, подключённый сразу после 200, мог поймать обрыв
    // «ничего не выполняется» или пропустить моментальное завершение.
    #[cfg(feature = "onnx")]
    crate::transcribe::start_model_download();
    #[cfg(feature = "onnx")]
    crate::transcribe::emit_model_event(serde_json::json!({
        "type": "stage", "stage": "model-download", "pct": 1,
        "message": format!("Скачивание {model_id}…"), "model_id": model_id,
    }));
    std::thread::spawn(move || {
        let app_data = app_data_dir();
        let manifest = mr_models::load_bundled_manifest();
        let res = mr_models::download_or_update_model(&app_data, &manifest, &model_id, &|ev| {
            #[cfg(feature = "onnx")]
            crate::transcribe::emit_model_event(serde_json::json!({
                "type": "progress", "stage": "model-download", "pct": ev.pct,
                "model_id": model_id, "downloaded_bytes": ev.downloaded_bytes,
                "total_bytes": ev.total_bytes, "message": ev.message,
            }));
        });
        #[cfg(feature = "onnx")]
        match res {
            Ok(()) => {
                crate::transcribe::emit_model_event(
                    serde_json::json!({"type": "model-done", "ok": true, "model_id": model_id}),
                );
                crate::transcribe::finish_model_events();
            }
            Err(e) => {
                crate::transcribe::emit_model_event(
                    serde_json::json!({"type": "model-done", "ok": false, "model_id": model_id, "error": e.to_string()}),
                );
                crate::transcribe::finish_model_events();
            }
        }
        #[cfg(not(feature = "onnx"))]
        let _ = res;
    });
    json_response(
        request,
        json!({"ok": true, "message": format!("Скачивание модели {reply_id} началось. Прогресс – в карточке модели.")}),
        200,
    );
}
