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

fn edit_transcript(body: &Value) -> Result<Value, (u16, String)> {
    let (json_path, md_path, txt_path) = transcript_paths(body)?;
    let source = std::fs::read_to_string(&json_path)
        .map_err(|_| (404, "Расшифровка не найдена".to_string()))?;
    let mut data: Value = serde_json::from_str(&source)
        .map_err(|_| (400, "Файл .transcript.json повреждён".to_string()))?;
    apply_edit(&mut data, body).map_err(|e| (400, e))?;

    let utterances: Vec<Utterance> = serde_json::from_value(data["utterances"].clone())
        .map_err(|e| (400, format!("Некорректные реплики: {e}")))?;
    let markdown = report::build_markdown(&data["meta"], &utterances);
    let text = report::build_text(&utterances);
    let json = serde_json::to_string_pretty(&data).map_err(|e| (500, e.to_string()))?;
    for (path, contents) in [
        (&json_path, json),
        (&md_path, markdown.clone()),
        (&txt_path, text),
    ] {
        atomic_write(path, &contents).map_err(|e| (500, e))?;
    }

    Ok(json!({
        "ok": true,
        "utterances": data["utterances"],
        "speakers_info": data["meta"]["speakers_info"],
        "markdown": markdown,
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
        for info in speakers_info.iter_mut() {
            if info["name"].as_str() == Some(old) {
                info["name"] = json!(new_speaker);
            }
            if info["matched_name"].as_str() == Some(old) {
                info["matched_name"] = json!(new_speaker);
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
        let matching = utterances
            .iter()
            .filter(|u| u["speaker"].as_str() == Some(name));
        let mut count = 0;
        let mut speech = 0.0;
        for utterance in matching {
            count += 1;
            let start = utterance["start"].as_f64().unwrap_or(0.0);
            speech += (utterance["end"].as_f64().unwrap_or(start) - start).max(0.0);
        }
        info["utterances_count"] = json!(count);
        info["total_speech_sec"] = json!(speech);
    }
    data["stats"]["segments"] = json!(utterances.len());
    data["stats"]["words"] = json!(words);
    Ok(())
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, contents)
        .map_err(|e| format!("Не удалось записать {}: {e}", path.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Не удалось сохранить {}: {e}", path.display()));
    }
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
    }
}
