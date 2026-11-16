use std::path::{Path, PathBuf};

use serde_json::json;

use super::super::lists::{ensure_person_in_pack, resolve_target_pack};
use super::super::transcript_names::{clear_matched_in_transcript, rename_speaker_in_files};
use super::store::{delete_voiceprint_files, voiceprint_files};
use super::{store_voiceprint, PrototypeSample};
use crate::config::save_dir;
use crate::http::json_response;
use crate::http::media::{inside_save_dir, resolve_media_path};
use crate::voiceprints::{l2_normalize, EMBEDDING_DIM};

pub(crate) fn handle_voiceprint_save(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let pack_id = body
        .get("pack_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    // Без onnx эмбеддинг из аудио не извлекается, поэтому mut не нужен.
    #[cfg_attr(not(feature = "onnx"), allow(unused_mut))]
    let mut emb: Vec<f64> = body
        .get("embedding")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
        .unwrap_or_default();
    let audio_file = body
        .get("audio_file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let old_name = body
        .get("old_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Границы образца для сниппета «как звучит»: берутся из карточки спикера.
    let sample_start = body.get("sample_start").and_then(|v| v.as_f64());
    let sample_end = body.get("sample_end").and_then(|v| v.as_f64());
    let sample_channel = body
        .get("sample_channel")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let mut extracted_quality: Option<f64> = None;
    #[cfg(feature = "onnx")]
    if emb.is_empty() && !audio_file.is_empty() {
        if let (Some(start), Some(end)) = (sample_start, sample_end) {
            let audio_p = Path::new(&audio_file);
            let full_p = if audio_p.is_absolute() {
                audio_p.to_path_buf()
            } else {
                save_dir().join(audio_p)
            };
            if inside_save_dir(&full_p) {
                let channel_i32 = sample_channel.map(|c| c as i32);
                match crate::transcribe::extract_utterance_embedding(
                    &full_p,
                    start,
                    end,
                    channel_i32,
                ) {
                    Ok((e, _dur, q)) => {
                        emb = e;
                        extracted_quality = Some(q);
                    }
                    Err(error) => {
                        json_response(request, json!({"ok": false, "error": error}), 422);
                        return;
                    }
                }
            }
        }
    }
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника"}),
            400,
        );
        return;
    }
    if !emb.is_empty() && emb.len() != EMBEDDING_DIM {
        json_response(
            request,
            json!({"ok": false, "error": "Неверная размерность голосового отпечатка"}),
            400,
        );
        return;
    }
    let chosen_pack = resolve_target_pack(&name, pack_id.as_deref());
    if !emb.is_empty() {
        let norm = l2_normalize(&emb);
        if norm.iter().all(|x| x.abs() < 1e-12) {
            json_response(
                request,
                json!({"ok": false, "error": "Пустой голосовой отпечаток"}),
                400,
            );
            return;
        }
        if let Err(error) = store_voiceprint(
            &name,
            Some(&chosen_pack),
            norm,
            PrototypeSample {
                audio_file: &audio_file,
                start: sample_start,
                end: sample_end,
                channel: sample_channel,
                quality: extracted_quality,
            },
        ) {
            json_response(request, json!({"ok": false, "error": error}), 422);
            return;
        }
    }

    ensure_person_in_pack(&name, Some("Участник"), &chosen_pack);
    let mut state = None;
    if !audio_file.is_empty() {
        // Правим расшифровки только внутри папки записей (запись в произвольную
        // директорию через чужое имя файла недопустима). Относительное имя
        // сначала разрешаем от `save_dir`, иначе файл не найдётся.
        let audio_path = resolve_media_path(&audio_file);
        if inside_save_dir(&audio_path) {
            state = rename_speaker_in_files(
                &audio_path,
                &name,
                if old_name.is_empty() {
                    None
                } else {
                    Some(old_name.as_str())
                },
            );
        }
    }
    // UI обновляет текст из этого ответа, без повторного GET: иначе на медленной
    // цепочке (loadPeople → reload) расшифровка оставалась старой.
    let mut payload =
        json!({"ok": true, "voiceprint_saved": !emb.is_empty(), "pack_id": chosen_pack});
    if let (Some(dst), Some(src)) = (
        payload.as_object_mut(),
        state.and_then(|v| v.as_object().cloned()),
    ) {
        for (key, value) in src {
            dst.insert(key, value);
        }
    }
    json_response(request, payload, 200);
}

#[cfg(not(feature = "onnx"))]
pub(crate) fn handle_voiceprint_learn_sample(
    request: tiny_http::Request,
    _body: serde_json::Value,
) {
    json_response(
        request,
        json!({"ok": false, "error": "Обучение голоса недоступно: сборка без onnx"}),
        501,
    );
}

#[cfg(feature = "onnx")]
pub(crate) fn handle_voiceprint_learn_sample(request: tiny_http::Request, body: serde_json::Value) {
    let file = body
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let index = body
        .get("index")
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok());
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if file.is_empty() || name.is_empty() || index.is_none() {
        json_response(
            request,
            json!({"ok": false, "error": "Нужны file, index и name"}),
            400,
        );
        return;
    }
    if name.chars().count() > 200 {
        json_response(
            request,
            json!({"ok": false, "error": "Имя слишком длинное"}),
            400,
        );
        return;
    }
    if crate::transcribe::is_transcribe_running() {
        json_response(
            request,
            json!({"ok": false, "error": "Дождитесь окончания расшифровки"}),
            409,
        );
        return;
    }
    let raw = PathBuf::from(&file);
    let audio = if raw.is_absolute() {
        raw
    } else {
        save_dir().join(raw)
    };
    if audio.file_name().is_none() || !inside_save_dir(&audio) {
        json_response(
            request,
            json!({"ok": false, "error": "Файл вне папки записей"}),
            404,
        );
        return;
    }
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let json_p = audio
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(save_dir)
        .join(format!("{stem}.transcript.json"));
    let data: serde_json::Value = match std::fs::read_to_string(&json_p) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                log::warn!(
                    target: "json",
                    path = crate::logging::file_token(&json_p),
                    error = error.to_string();
                    "ev=json.load"
                );
                json_response(
                    request,
                    json!({"ok": false, "error": "Расшифровка не найдена"}),
                    404,
                );
                return;
            }
        },
        Err(error) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(&json_p),
                error = error.to_string();
                "ev=json.load"
            );
            json_response(
                request,
                json!({"ok": false, "error": "Расшифровка не найдена"}),
                404,
            );
            return;
        }
    };
    let u = data
        .get("utterances")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(index.unwrap_or(usize::MAX)));
    let (start, end, channel) = match u {
        Some(u) => (
            u.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0),
            u.get("end")
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| u.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0)),
            u.get("channel").and_then(|v| v.as_i64()).map(|c| c as i32),
        ),
        None => {
            json_response(
                request,
                json!({"ok": false, "error": "Реплика не найдена"}),
                404,
            );
            return;
        }
    };
    let (emb, _clean_dur, quality) =
        match crate::transcribe::extract_utterance_embedding(&audio, start, end, channel) {
            Ok(res) => res,
            Err(e) => {
                json_response(request, json!({"ok": false, "error": e}), 422);
                return;
            }
        };
    if emb.len() != EMBEDDING_DIM {
        json_response(
            request,
            json!({"ok": false, "error": "Неверная размерность голосового отпечатка"}),
            500,
        );
        return;
    }
    let norm = l2_normalize(&emb);
    if norm.iter().all(|x| x.abs() < 1e-12) {
        json_response(
            request,
            json!({"ok": false, "error": "Пустой голосовой отпечаток"}),
            400,
        );
        return;
    }
    let pack_id = body
        .get("pack_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string());
    let chosen_pack = resolve_target_pack(&name, pack_id.as_deref());
    let audio_lossy = audio.to_string_lossy();
    if let Err(error) = store_voiceprint(
        &name,
        Some(&chosen_pack),
        norm,
        PrototypeSample {
            audio_file: &audio_lossy,
            start: Some(start),
            end: Some(end),
            channel: channel.map(|c| c.max(0) as usize),
            quality: Some(quality),
        },
    ) {
        json_response(request, json!({"ok": false, "error": error}), 422);
        return;
    }
    ensure_person_in_pack(&name, Some("Участник"), &chosen_pack);
    json_response(
        request,
        json!({"ok": true, "voiceprint_saved": !emb.is_empty(), "pack_id": chosen_pack}),
        200,
    );
}

pub(crate) fn handle_voiceprint_delete(request: tiny_http::Request, body: serde_json::Value) {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let audio_file = body.get("audio_file").and_then(|v| v.as_str());
    if name.is_empty() {
        json_response(
            request,
            json!({"ok": false, "error": "Укажите имя участника"}),
            400,
        );
        return;
    }
    let found = delete_voiceprint_internal(name, audio_file);
    json_response(request, json!({"ok": found}), 200);
}

pub(crate) fn delete_voiceprint_internal(name: &str, audio_file: Option<&str>) -> bool {
    let found = delete_voiceprint_files(name, voiceprint_files());
    if found {
        if let Some(af) = audio_file.filter(|s| !s.is_empty()) {
            clear_matched_in_transcript(Path::new(af), name.trim());
        }
    }
    found
}
