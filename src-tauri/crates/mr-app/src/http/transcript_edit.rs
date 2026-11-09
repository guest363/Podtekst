use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::json_response;
use super::media::inside_save_dir;
use crate::config::save_dir;
use crate::transcribe::{report, Utterance};

pub(super) fn handle_transcript_edit(request: tiny_http::Request, body: Value) {
    match edit_transcript(&body) {
        Ok(result) => json_response(request, result, 200),
        Err((status, error)) => {
            json_response(request, json!({"ok": false, "error": error}), status)
        }
    }
}

pub(super) fn handle_transcript_refine(request: tiny_http::Request, body: Value) {
    let file = body.get("file").and_then(|v| v.as_str()).unwrap_or("").trim();
    if file.is_empty() {
        json_response(request, json!({"ok": false, "error": "Файл не указан"}), 400);
        return;
    }
    match crate::transcribe::refine::refine_transcript_file(file) {
        Ok(result) => json_response(request, result, 200),
        Err((status, error)) => {
            json_response(request, json!({"ok": false, "error": error}), status)
        }
    }
}

fn edit_transcript(body: &Value) -> Result<Value, (u16, String)> {
    let (json_path, md_path, txt_path) = transcript_paths(body)?;
    let source = std::fs::read_to_string(&json_path).map_err(|error| {
        log::warn!(
            target: "json",
            path = crate::logging::file_token(&json_path),
            error = error.to_string();
            "ev=json.load"
        );
        (404, "Расшифровка не найдена".to_string())
    })?;
    let mut data: Value = serde_json::from_str(&source).map_err(|error| {
        log::warn!(
            target: "json",
            path = crate::logging::file_token(&json_path),
            error = error.to_string();
            "ev=json.load"
        );
        (400, "Файл .transcript.json повреждён".to_string())
    })?;
    let utts_before = data["utterances"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);
    apply_edit(&mut data, body).map_err(|e| (400, e))?;
    log::info!(
        target: "json",
        file = crate::logging::file_token(&json_path),
        op = if body.get("old_speaker").is_some() { "rename" } else { "edit" },
        index = body.get("index").and_then(|v| v.as_u64()).map(|v| v as i64).unwrap_or(-1),
        speaker_id = crate::logging::speaker_token(
            body.get("speaker").and_then(|v| v.as_str()).unwrap_or("")
        ),
        text_chars = body.get("text").and_then(|v| v.as_str()).map(|t| t.chars().count()).unwrap_or(0),
        utts_before = utts_before,
        utts_after = data["utterances"].as_array().map(|items| items.len()).unwrap_or(0);
        "ev=json.edit"
    );
    if cfg!(debug_assertions) {
        if let Ok(doc) =
            serde_json::from_value::<crate::transcribe::document::TranscriptDoc>(data.clone())
        {
            for violation in crate::transcribe::document::validate(&doc).iter().take(20) {
                log::warn!(
                    target: "json",
                    i = violation.index.map(|value| value as i64).unwrap_or(-1),
                    field = violation.field,
                    value = violation.value.clone(),
                    rule = violation.rule;
                    "ev=json.check"
                );
            }
        }
    }

    let utterances: Vec<Utterance> = serde_json::from_value(data["utterances"].clone())
        .map_err(|e| (400, format!("Некорректные реплики: {e}")))?;
    let markdown = report::build_markdown(&data["meta"], &utterances);
    let text = report::build_text(&utterances);
    let rev = serde_json::from_value::<Vec<mr_summarize::Utterance>>(data["utterances"].clone())
        .ok()
        .map(|items| mr_summarize::artifact::source_rev(&items));
    let json = serde_json::to_string_pretty(&data).map_err(|e| (500, e.to_string()))?;
    for (path, contents) in [
        (&json_path, json),
        (&md_path, markdown.clone()),
        (&txt_path, text),
    ] {
        crate::fsutil::write_atomic(path, contents.as_bytes())
            .map_err(|e| (500, format!("Не удалось записать {}: {e}", path.display())))?;
    }

    Ok(json!({
        "ok": true,
        "utterances": data["utterances"],
        "speakers_info": data["meta"]["speakers_info"],
        "markdown": markdown,
        "rev": rev,
    }))
}

fn transcript_paths(body: &Value) -> Result<(PathBuf, PathBuf, PathBuf), (u16, String)> {
    let file = body
        .get("file")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if file.is_empty() {
        return Err((400, "Файл не указан".to_string()));
    }
    let raw = PathBuf::from(file);
    let audio = if raw.is_absolute() {
        raw
    } else {
        save_dir().join(raw)
    };
    if audio.file_name().is_none() || !inside_save_dir(&audio) {
        return Err((404, "Файл вне папки записей".to_string()));
    }
    let stem = audio
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = audio.parent().unwrap_or_else(|| Path::new("."));
    Ok((
        parent.join(format!("{stem}.transcript.json")),
        parent.join(format!("{stem}.md")),
        parent.join(format!("{stem}.txt")),
    ))
}

fn apply_edit(data: &mut Value, body: &Value) -> Result<(), String> {
    let new_speaker = body
        .get("speaker")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if new_speaker.chars().count() > 200 {
        return Err("Имя спикера слишком длинное".to_string());
    }
    let renaming = body.get("old_speaker").is_some();
    if renaming {
        let old = body["old_speaker"].as_str().unwrap_or("").trim();
        if old.is_empty() || new_speaker.is_empty() {
            return Err("Имя спикера не может быть пустым".to_string());
        }
        let utterances = data["utterances"]
            .as_array_mut()
            .ok_or_else(|| "В расшифровке нет реплик".to_string())?;
        let mut found = false;
        for utterance in utterances {
            if utterance["speaker"].as_str() == Some(old) {
                utterance["speaker"] = json!(new_speaker);
                found = true;
            }
        }
        if !found {
            return Err("Спикер не найден в репликах".to_string());
        }
    } else {
        let index = body
            .get("index")
            .and_then(|v| v.as_u64())
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| "Некорректный индекс реплики".to_string())?;
        let text = match body.get("text") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| "Текст реплики должен быть строкой".to_string())?,
            ),
        };
        if text.is_none() && new_speaker.is_empty() {
            return Err("Нет изменений".to_string());
        }
        let utterance = data["utterances"]
            .as_array_mut()
            .and_then(|items| items.get_mut(index))
            .ok_or_else(|| "Реплика не найдена".to_string())?;
        if let Some(text) = text {
            utterance["text"] = json!(text);
            // Уверенность привязана к старому тексту: чистим слова,
            // иначе UI покажет чипы чужой расшифровки.
            utterance["words"] = json!([]);
            utterance["ctc_used"] = json!(false);
        }
        if !new_speaker.is_empty() {
            utterance["speaker"] = json!(new_speaker);
        }
    }

    let utterances_doc = data["utterances"].clone();
    let utterances = utterances_doc
        .as_array()
        .ok_or_else(|| "В расшифровке нет реплик".to_string())?;
    let mut names = Vec::new();
    for utterance in utterances {
        let name = utterance["speaker"].as_str().unwrap_or("Спикер");
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let words = utterances
        .iter()
        .map(|u| u["text"].as_str().unwrap_or("").split_whitespace().count())
        .sum::<usize>();

    let meta = data["meta"]
        .as_object_mut()
        .ok_or_else(|| "В расшифровке нет метаданных".to_string())?;
    meta.insert("speakers".to_string(), json!(names));
    if !meta.get("speakers_info").is_some_and(Value::is_array) {
        meta.insert("speakers_info".to_string(), json!([]));
    }
    let speakers_info = meta["speakers_info"]
        .as_array_mut()
        .expect("массив создан выше");
    if renaming {
        let old = body["old_speaker"].as_str().unwrap_or("").trim();
        let unmatch = body
            .get("unmatch")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        for info in speakers_info.iter_mut() {
            let hit = info["name"].as_str() == Some(old)
                || info["matched_name"].as_str() == Some(old)
                || info["name"].as_str() == Some(new_speaker);
            if info["name"].as_str() == Some(old) {
                info["name"] = json!(new_speaker);
            }
            if info["matched_name"].as_str() == Some(old) {
                info["matched_name"] = json!(new_speaker);
            }
            if unmatch && hit {
                info["matched"] = json!(false);
                info["matched_name"] = serde_json::Value::Null;
                info["matched_pack_id"] = serde_json::Value::Null;
                info["matched_pack_name"] = serde_json::Value::Null;
            }
        }
    }
    if !new_speaker.is_empty()
        && !speakers_info
            .iter()
            .any(|info| info["name"].as_str() == Some(new_speaker))
    {
        speakers_info
            .push(json!({"name": new_speaker, "utterances_count": 0, "total_speech_sec": 0}));
    }
    for info in speakers_info {
        let name = info["name"].as_str().unwrap_or("");
        let matching: Vec<&Value> = utterances
            .iter()
            .filter(|u| u["speaker"].as_str() == Some(name))
            .collect();
        let count = matching.len();
        let speech: f64 = matching
            .iter()
            .map(|u| {
                let start = u["start"].as_f64().unwrap_or(0.0);
                (u["end"].as_f64().unwrap_or(start) - start).max(0.0)
            })
            .sum();
        info["utterances_count"] = json!(count);
        info["total_speech_sec"] = json!(speech);

        // У откреплённого спикера генерируем сэмплы из его реплик для прослушивания и биометрии
        let has_samples = info
            .get("samples")
            .and_then(|s| s.as_array())
            .is_some_and(|a| !a.is_empty())
            || info.get("sample").is_some();
        if !has_samples && !matching.is_empty() {
            let samples: Vec<Value> = matching
                .iter()
                .take(3)
                .map(|u| {
                    let start = u["start"].as_f64().unwrap_or(0.0);
                    let end = u["end"].as_f64().unwrap_or(start);
                    let dur = (end - start).max(0.0);
                    let text = u["text"].as_str().unwrap_or("");
                    let ch = u.get("channel").and_then(|c| c.as_i64());
                    json!({
                        "start": start,
                        "end": end,
                        "dur": dur,
                        "text": text,
                        "full_text": text,
                        "channel": ch,
                    })
                })
                .collect();
            if !samples.is_empty() {
                if info.get("sample").is_none() {
                    info["sample"] = samples[0].clone();
                }
                info["samples"] = json!(samples);
            }
        }
    }
    data["stats"]["segments"] = json!(utterances.len());
    data["stats"]["words"] = json!(words);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_text_speaker_and_global_name() {
        let mut data = json!({
            "meta": {"speakers": ["Спикер 1", "Спикер 2"], "speakers_info": [
                {"name": "Спикер 1"}, {"name": "Спикер 2"}
            ]},
            "stats": {},
            "utterances": [
                {"start": 0.0, "end": 2.0, "speaker": "Спикер 1", "text": "Первая"},
                {"start": 2.0, "end": 5.0, "speaker": "Спикер 2", "text": "Вторая"}
            ]
        });
        apply_edit(
            &mut data,
            &json!({"index": 0, "text": "Новый\nтекст", "speaker": "Спикер 2"}),
        )
        .unwrap();
        assert_eq!(data["utterances"][0]["text"], "Новый\nтекст");
        assert_eq!(data["meta"]["speakers"], json!(["Спикер 2"]));
        assert_eq!(data["meta"]["speakers_info"][1]["utterances_count"], 2);

        apply_edit(
            &mut data,
            &json!({"old_speaker": "Спикер 2", "speaker": "Ведущий"}),
        )
        .unwrap();
        assert!(data["utterances"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["speaker"] == "Ведущий"));
        assert_eq!(data["stats"]["words"], 3);
        assert_eq!(data["meta"]["speakers_info"][1]["total_speech_sec"], 5.0);

        data["meta"]["speakers_info"][1]["matched"] = json!(true);
        data["meta"]["speakers_info"][1]["matched_name"] = json!("Ведущий");
        apply_edit(
            &mut data,
            &json!({"old_speaker": "Ведущий", "speaker": "Ведущий", "unmatch": true}),
        )
        .unwrap();
        assert_eq!(data["meta"]["speakers_info"][1]["matched"], json!(false));
        assert!(data["meta"]["speakers_info"][1]["matched_name"].is_null());
    }

    #[test]
    fn text_edit_drops_stale_confidence() {
        let mut data = json!({
            "meta": {"speakers": ["Спикер"], "speakers_info": []},
            "stats": {},
            "utterances": [
                {"start": 0.0, "end": 2.0, "speaker": "Спикер", "text": "раз два",
                 "words": [{"w": "раз", "conf": 0.9, "needs_review": false}],
                 "ctc_used": true}
            ]
        });
        apply_edit(&mut data, &json!({"index": 0, "text": "раз два три"})).unwrap();
        assert_eq!(data["utterances"][0]["text"], "раз два три");
        assert_eq!(data["utterances"][0]["words"], json!([]));
        assert_eq!(data["utterances"][0]["ctc_used"], json!(false));
    }
}
