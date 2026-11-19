use serde_json::json;

use super::media::{inside_save_dir, resolve_media_path};
#[cfg(feature = "onnx")]
use super::media::{is_video_path, video_supported};
use super::{json_response, query_param};
use crate::config::{app_data_dir, save_dir};

pub(super) fn serve_transcript(request: tiny_http::Request, query: &str) {
    let Some(f) = query_param(query, "file") else {
        json_response(request, json!({"error": "No file specified"}), 400);
        return;
    };
    // Относительное имя резолвим от папки записей ДО проверок и чтения:
    // иначе голое имя файла читалось бы из CWD процесса.
    let p = resolve_media_path(&f);
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
    #[cfg(feature = "onnx")]
    let mut rev = None;
    #[cfg(not(feature = "onnx"))]
    let rev = None;
    #[cfg(feature = "onnx")]
    match crate::transcribe::document::read_transcript(&json_p) {
        Ok(doc) => {
            let summary_utts: Vec<mr_summarize::Utterance> = doc
                .utterances
                .iter()
                .map(|u| mr_summarize::Utterance {
                    start: u.start,
                    end: u.end,
                    speaker: u.speaker.clone(),
                    text: u.text.clone(),
                })
                .collect();
            rev = Some(mr_summarize::artifact::source_rev(&summary_utts));
            meta = serde_json::to_value(&doc.meta).unwrap_or_else(|_| json!({}));
            speakers_info = meta.get("speakers_info").cloned().unwrap_or(json!([]));
            // Полный список реплик без лимита: обрезка «первые 200» теряла
            // хвост длинных встреч в UI.
            utterances = serde_json::to_value(&doc.utterances).unwrap_or_else(|_| json!([]));
        }
        Err(crate::transcribe::document::TranscriptError::NotFound(_)) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&json_p),
                reason = "not_found";
                "ev=json.load"
            );
        }
        Err(_) => {}
    }
    // Сборка без onnx не содержит конвейера и его формата: UI всё равно
    // должен показывать уже готовые расшифровки.
    #[cfg(not(feature = "onnx"))]
    if let Ok(text) = std::fs::read_to_string(&json_p) {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(data) => {
                meta = data.get("meta").cloned().unwrap_or(json!({}));
                speakers_info = meta.get("speakers_info").cloned().unwrap_or(json!([]));
                utterances = data.get("utterances").cloned().unwrap_or(json!([]));
            }
            Err(error) => log::warn!(
                target: "json",
                path = crate::logging::file_token(&json_p),
                error = error.to_string();
                "ev=json.load"
            ),
        }
    }
    let md_text = std::fs::read_to_string(&md_p).unwrap_or_default();
    // Без onnx расшифровка читается как Value: повторный разбор нужен только
    // для source_rev, в обычной сборке rev уже посчитан по типизированному doc.
    #[cfg(not(feature = "onnx"))]
    let rev = rev.or_else(|| {
        serde_json::from_value::<Vec<mr_summarize::Utterance>>(utterances.clone())
            .ok()
            .map(|items| mr_summarize::artifact::source_rev(&items))
    });
    log::debug!(
        target: "json",
        file = crate::logging::file_token(&p),
        utts = utterances.as_array().map(|items| items.len()).unwrap_or(0),
        bytes = std::fs::metadata(&json_p).map(|m| m.len()).unwrap_or(0),
        meta_keys = meta.as_object().map(|map| map.len()).unwrap_or(0),
        rev = rev.clone().unwrap_or_default();
        "ev=json.response"
    );
    json_response(
        request,
        json!({
            "filename": p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "meta": meta,
            "speakers_info": speakers_info,
            "utterances": utterances,
            "markdown": md_text,
            "rev": rev,
        }),
        200,
    );
}

/// SSE-стрим событий транскрибации/моделей (совместим с EventSource в Web UI).
///
/// Возобновление по стандартному заголовку `Last-Event-ID` (браузер шлёт его сам
/// после обрыва связи): клиент продолжает с места разрыва; без курсора клиент
/// получает только новые события.
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
            // Клиент уже получил событие с этим id — продолжаем со следующего.
            .map(|id| id.saturating_add(1))
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
        // Без паузы первый же опрос «не running» закрывал поток раньше, чем
        // summary успевал выставить SUMMARY_ACTIVE.
        let mut idle_ticks = 0u8;
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
            let wrote = !buf.is_empty();
            if (wrote && out.write_all(buf.as_bytes()).is_err()) || out.flush().is_err() {
                return;
            }
            if !running {
                idle_ticks += 1;
                if idle_ticks >= 2 {
                    break;
                }
            } else {
                idle_ticks = 0;
            }
            if wrote {
                idle_ticks = 0;
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
    // Резолвим путь ДО проверок: иначе `exists()` смотрел бы в CWD процесса,
    // а конвейер писал бы артефакты рядом с файлом вне папки записей.
    let audio = resolve_media_path(&audio_file);
    if audio_file.is_empty() || !audio.is_file() {
        json_response(
            request,
            json!({"ok": false, "error": "Аудиофайл не найден. Проверьте путь и повторите."}),
            400,
        );
        return;
    }
    // Транскрибируем только записи из папки записей: результат (md/json)
    // пишется рядом с исходником — произвольный путь означал бы запись на диск.
    if !inside_save_dir(&audio) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл вне папки записей"}),
            400,
        );
        return;
    }
    if !video_supported() && is_video_path(&audio) {
        json_response(
            request,
            json!({"ok": false, "error": "На Android видео не поддерживается. Импортируйте аудиофайл."}),
            400,
        );
        return;
    }
    if !mr_audio::has_audio_track(&audio) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл не содержит аудиодорожки"}),
            400,
        );
        return;
    }
    let min_speakers = body
        .get("min_speakers")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0);
    let max_speakers = body
        .get("max_speakers")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0);
    if let Err(error) = crate::transcribe::validate_speaker_bounds(min_speakers, max_speakers) {
        json_response(request, json!({"ok": false, "error": error}), 400);
        return;
    }
    let opts = crate::transcribe::TranscribeOptions {
        speaker_left: my_name,
        speaker_right: "Собеседник".to_string(),
        engine,
        language,
        min_speakers,
        max_speakers,
        hotwords: hotwords_from_body(&body),
        ctc_enabled: body
            .get("ctc_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(crate::config::configured_ctc_enabled),
    };
    let (started, position) = crate::transcribe::enqueue_transcription(audio, opts);
    let already_running = !started && position == 1;
    json_response(
        request,
        json!({
            "ok": true,
            "started": started,
            "queued": !started && !already_running,
            "position": position,
            "message": if started {
                "Расшифровка запущена"
            } else if already_running {
                "Запись уже расшифровывается"
            } else {
                "Добавлено в очередь"
            }
        }),
        200,
    );
}

#[cfg(feature = "onnx")]
pub(super) fn handle_queue_get(request: tiny_http::Request) {
    let status = crate::transcribe::get_queue_status();
    json_response(request, status, 200);
}

#[cfg(feature = "onnx")]
pub(super) fn handle_queue_remove(request: tiny_http::Request, body: serde_json::Value) {
    let file = body.get("file").and_then(|v| v.as_str()).unwrap_or("");
    let ok = crate::transcribe::remove_from_queue(file);
    json_response(request, json!({"ok": ok}), 200);
}

#[cfg(feature = "onnx")]
pub(super) fn handle_queue_clear(request: tiny_http::Request) {
    let count = crate::transcribe::clear_queue();
    json_response(request, json!({"ok": true, "cleared": count}), 200);
}

#[cfg(not(feature = "onnx"))]
pub(super) fn handle_transcribe_start(request: tiny_http::Request, _body: serde_json::Value) {
    json_response(
        request,
        json!({"ok": false, "error": "Расшифровка недоступна: ядро собрано без --features onnx"}),
        501,
    );
}

pub(super) fn serve_hotwords(request: tiny_http::Request) {
    json_response(
        request,
        json!({
            "ok": true,
            "hotwords": crate::config::configured_hotwords(),
            "ctc_enabled": crate::config::configured_ctc_enabled(),
            "limit": crate::hotwords::MAX_HOTWORDS,
        }),
        200,
    );
}

pub(super) fn handle_hotwords_save(request: tiny_http::Request, body: serde_json::Value) {
    let raw: Vec<String> = body
        .get("hotwords")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(crate::config::configured_hotwords);
    if raw.len() > crate::hotwords::MAX_HOTWORDS {
        json_response(
            request,
            json!({"ok": false, "error": "Словарь ограничен 500 записями"}),
            400,
        );
        return;
    }
    let ctc_enabled = body.get("ctc_enabled").and_then(|v| v.as_bool());
    let clean = crate::hotwords::sanitize(&raw);
    crate::config::save_hotwords_cfg(clean.clone(), ctc_enabled);
    json_response(
        request,
        json!({
            "ok": true,
            "hotwords": clean,
            "ctc_enabled": ctc_enabled.unwrap_or_else(crate::config::configured_ctc_enabled),
        }),
        200,
    );
}

#[cfg(feature = "onnx")]
fn hotwords_from_body(body: &serde_json::Value) -> Vec<String> {
    match body.get("hotwords").and_then(|v| v.as_array()) {
        None => crate::config::configured_hotwords(),
        Some(a) => crate::hotwords::sanitize(
            &a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        ),
    }
}

/// Установка подменяет файлы моделей: работа, которая их читает, должна быть
/// завершена, иначе расшифровка увидит набор из двух версий.
fn install_blocker() -> Option<(&'static str, &'static str)> {
    #[cfg(feature = "onnx")]
    if crate::transcribe::has_pending_work() {
        return Some((
            "busy_transcription",
            "Идёт расшифровка. Дождитесь окончания и повторите.",
        ));
    }
    if crate::summary::is_active() {
        return Some((
            "busy_summary",
            "Идут итоги. Дождитесь окончания и повторите.",
        ));
    }
    None
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
    let manifest = mr_models::load_bundled_manifest();
    let Some(entry) = manifest.models.iter().find(|m| m.id == model_id) else {
        json_response(
            request,
            json!({"ok": false, "error": "Модель не найдена в манифесте", "error_code": "unknown_model"}),
            400,
        );
        return;
    };
    let kind = entry.kind.clone();
    if let Some((code, message)) = install_blocker() {
        json_response(
            request,
            json!({"ok": false, "error": message, "error_code": code, "retryable": true}),
            409,
        );
        return;
    }
    let total: u64 = entry.files.iter().map(|f| f.size_bytes).sum();
    let models_root = mr_models::models_dir(&app_data_dir());
    if let Err(mr_models::ModelsError::NoSpace {
        need_bytes,
        free_bytes,
    }) = mr_models::check_space(&models_root, total)
    {
        json_response(
            request,
            json!({
                "ok": false,
                "code": "disk_low",
                "error": "Не хватает места на диске. Освободите место и повторите.",
                "need_bytes": need_bytes,
                "free_bytes": free_bytes,
            }),
            422,
        );
        return;
    }
    // Windows не даёт заменить загруженный .gguf: гасим sidecar до публикации.
    // Ровно для модели итогов — ASR-модели sidecar не читает.
    if kind == "summary" {
        crate::summary::shutdown();
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
            #[cfg(not(feature = "onnx"))]
            let _ = &ev;
            #[cfg(feature = "onnx")]
            crate::transcribe::emit_model_event(serde_json::json!({
                "type": "progress", "stage": "model-download", "pct": ev.pct,
                "model_id": model_id, "downloaded_bytes": ev.downloaded_bytes,
                "total_bytes": ev.total_bytes, "message": ev.message,
            }));
        });
        if res.is_ok() {
            // Модель итогов не должна вытеснять кэш ASR: иначе каждая расшифровка
            // после сводки считается заново.
            if kind == "summary" {
                crate::summary::invalidate_model_cache();
            } else {
                #[cfg(feature = "onnx")]
                crate::transcribe::invalidate_models_cache();
            }
        }
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
