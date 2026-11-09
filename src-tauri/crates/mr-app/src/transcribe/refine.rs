use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{report, Utterance};
use crate::config::save_dir;
use crate::fsutil::write_atomic;
use crate::http::media::inside_save_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefinedUtt {
    start: f64,
    end: f64,
    speaker: String,
    text: String,
}

pub fn refine_transcript_file(file: &str) -> Result<Value, (u16, String)> {
    if !crate::summary::model_ready() {
        return Err((
            400,
            "Модель Qwen не установлена. Скачайте её во вкладке «Модели ИИ».".to_string(),
        ));
    }

    let (json_path, md_path, txt_path) = resolve_paths(file)?;
    let source = std::fs::read_to_string(&json_path)
        .map_err(|_| (404, "Файл расшифровки не найден.".to_string()))?;
    let mut data: Value = serde_json::from_str(&source)
        .map_err(|_| (400, "Файл .transcript.json повреждён.".to_string()))?;

    let original_utts: Vec<Utterance> = serde_json::from_value(data["utterances"].clone())
        .map_err(|e| (400, format!("Некорректные реплики: {e}")))?;

    if original_utts.is_empty() {
        return Err((400, "В расшифровке нет реплик для реставрации.".to_string()));
    }

    let intro_text: String = original_utts
        .iter()
        .take(15)
        .map(|u| format!("{}: {}", u.speaker, u.text))
        .collect::<Vec<_>>()
        .join("\n");

    let passport_prompt = format!(
        "<|im_start|>system\nТы стенографист. Проанализируй начало записи и извлеки ключевых участников и сущности (суд, дело, банк, суммы).\n<|im_end|>\n<|im_start|>user\nНачало встречи:\n{}\n\nКратко перечисли участников (имена и роли: судья, истец, ответчик, представитель) и ключевые термины/сущности.<|im_end|>\n<|im_start|>assistant\n",
        intro_text
    );

    let passport = crate::summary::run_raw_prompt(&passport_prompt, 256, 0.2)
        .unwrap_or_else(|_| "Судебное заседание или деловая встреча.".to_string());

    const BATCH_SIZE: usize = 12;
    let mut refined_utts: Vec<Utterance> = Vec::with_capacity(original_utts.len());

    for chunk in original_utts.chunks(BATCH_SIZE) {
        let chunk_json = serde_json::to_string_pretty(&chunk.iter().map(|u| {
            json!({
                "start": u.start,
                "end": u.end,
                "speaker": u.speaker,
                "text": u.text
            })
        }).collect::<Vec<_>>()).unwrap_or_default();

        let refine_prompt = format!(
            "<|im_start|>system\nТы эксперт-стенографист. Твоя задача — отреставрировать реплики, записанные на удалённый микрофон в помещении.\nКонтекст встречи:\n{}\n\nПравила:\n1. Исправь фонетические искажения слов по смыслу контекста встречи (названия учреждений, термины, фамилии, числительные). НЕ выдумывай новых фактов, сохраняй каждое сказанное слово максимально близко к оригиналу!\n2. Если внутри одной реплики говорят разные участники (например, один задаёт вопрос, а другой отвечает «Ваша честь»), раздели на две отдельные реплики со своими метками времени.\n3. Укажи правильного спикера для каждой реплики (Судья, Истец, Представитель истца, Представитель ответчика и т.д.).\n4. Верни СТРОГО валидный JSON-массив без комментариев: [ {{\"start\": ..., \"end\": ..., \"speaker\": \"...\", \"text\": \"...\"}} ]\n<|im_end|>\n<|im_start|>user\nРеплики для реставрации:\n{}\n<|im_end|>\n<|im_start|>assistant\n[",
            passport.trim(),
            chunk_json
        );

        let mut batch_refined = false;
        if let Ok(response) = crate::summary::run_raw_prompt(&refine_prompt, 2048, 0.2) {
            let full_json = format!("[{response}");
            let parsed = parse_json_array(&full_json);
            if let Some(items) = parsed {
                if !items.is_empty() {
                    for item in items {
                        refined_utts.push(Utterance {
                            start: item.start,
                            end: item.end,
                            speaker: item.speaker,
                            channel: None,
                            text: item.text,
                            words: Vec::new(),
                            diarized: true,
                            silhouette: 0.0,
                            ctc_used: false,
                        });
                    }
                    batch_refined = true;
                }
            }
        }

        if !batch_refined {
            refined_utts.extend_from_slice(chunk);
        }
    }

    let utts_val = serde_json::to_value(&refined_utts).map_err(|e| (500, e.to_string()))?;
    data["utterances"] = utts_val.clone();

    let mut unique_speakers: Vec<String> = refined_utts.iter().map(|u| u.speaker.clone()).collect();
    unique_speakers.sort();
    unique_speakers.dedup();
    data["meta"]["speakers"] = json!(unique_speakers);

    let mut speakers_info: Vec<Value> = Vec::new();
    for spk_name in &unique_speakers {
        let matching: Vec<&Utterance> = refined_utts.iter().filter(|u| &u.speaker == spk_name).collect();
        let count = matching.len();
        let speech: f64 = matching.iter().map(|u| (u.end - u.start).max(0.0)).sum();
        let samples: Vec<Value> = matching
            .iter()
            .take(3)
            .map(|u| {
                json!({
                    "start": u.start,
                    "end": u.end,
                    "dur": (u.end - u.start).max(0.0),
                    "text": u.text,
                    "full_text": u.text,
                    "channel": u.channel,
                })
            })
            .collect();
        speakers_info.push(json!({
            "name": spk_name,
            "utterances_count": count,
            "total_speech_sec": speech,
            "samples": samples,
            "matched": false,
        }));
    }
    data["meta"]["speakers_info"] = json!(speakers_info);

    let markdown = report::build_markdown(&data["meta"], &refined_utts);
    let text = report::build_text(&refined_utts);
    let rev = serde_json::from_value::<Vec<mr_summarize::Utterance>>(utts_val)
        .ok()
        .map(|items| mr_summarize::artifact::source_rev(&items));

    let json_str = serde_json::to_string_pretty(&data).map_err(|e| (500, e.to_string()))?;

    for (path, contents) in [
        (&json_path, json_str),
        (&md_path, markdown.clone()),
        (&txt_path, text),
    ] {
        write_atomic(path, contents.as_bytes())
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

fn parse_json_array(raw: &str) -> Option<Vec<RefinedUtt>> {
    let text = raw.trim();
    let start_idx = text.find('[')?;
    let end_idx = text.rfind(']')?;
    if end_idx <= start_idx {
        return None;
    }
    let slice = &text[start_idx..=end_idx];
    serde_json::from_str::<Vec<RefinedUtt>>(slice).ok()
}

fn resolve_paths(file: &str) -> Result<(PathBuf, PathBuf, PathBuf), (u16, String)> {
    let raw = PathBuf::from(file.trim());
    if raw.as_os_str().is_empty() {
        return Err((400, "Имя файла не указано.".to_string()));
    }
    let audio = if raw.is_absolute() {
        raw
    } else {
        save_dir().join(raw)
    };
    if audio.file_name().is_none() || !inside_save_dir(&audio) {
        return Err((404, "Файл вне папки записей".to_string()));
    }
    let parent = audio.parent().unwrap_or_else(|| Path::new("."));
    let file_name = audio.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = if let Some(stripped) = file_name.strip_suffix(".transcript.json") {
        stripped.to_string()
    } else if let Some(stripped) = file_name.strip_suffix(".md") {
        stripped.to_string()
    } else {
        audio
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    if stem.is_empty() {
        return Err((400, "Некорректное имя файла".to_string()));
    }
    let json_path = parent.join(format!("{stem}.transcript.json"));
    let md_path = parent.join(format!("{stem}.md"));
    let txt_path = parent.join(format!("{stem}.txt"));
    Ok((json_path, md_path, txt_path))
}
