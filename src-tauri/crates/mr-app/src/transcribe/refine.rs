use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{document, report, Utterance};
use crate::config::save_dir;
use crate::fsutil::write_atomic;
use crate::http::media::inside_save_dir;

const PASSPORT_SYSTEM: &str = "Ты стенографист. По началу записи определи ключевых участников (имена и роли, если они звучат) и ключевые термины или сущности.";

const REFINE_SYSTEM: &str = r#"Ты эксперт-стенографист. Отреставрируй реплики, записанные на удалённый микрофон в помещении.
Контекст встречи:
{passport}

Правила:
1. Исправь фонетические искажения слов по смыслу (названия учреждений, термины, фамилии, числительные). Не выдумывай новых фактов и сохраняй каждое сказанное слово как можно ближе к оригиналу.
2. Если внутри одной реплики говорят разные участники, раздели её на отдельные реплики со своими метками времени.
3. Сохраняй существующие метки спикеров; используй роль (например, «Судья» или «Представитель») только если она прямо следует из контекста.
4. Верни строго валидный JSON-массив без комментариев: [{"start": 0.0, "end": 0.0, "speaker": "...", "text": "..."}]"#;

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

    let previous_info = data["meta"]["speakers_info"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let intro_text: String = original_utts
        .iter()
        .take(15)
        .map(|u| format!("{}: {}", u.speaker, u.text))
        .collect::<Vec<_>>()
        .join("\n");

    let passport_user = format!("Начало встречи:\n{intro_text}");

    const BATCH_SIZE: usize = 20;
    let total_batches = original_utts.len().div_ceil(BATCH_SIZE);
    let (refined_utts, failed_batches) = crate::summary::run_prompts_session(|run| {
        let passport = run(PASSPORT_SYSTEM, &passport_user, 256, 0.2)
            .unwrap_or_else(|_| "деловая или официальная встреча".to_string());
        let refine_system = REFINE_SYSTEM.replace("{passport}", passport.trim());
        let mut failed = 0usize;
        let mut refined: Vec<Utterance> = Vec::with_capacity(original_utts.len());

        for chunk in original_utts.chunks(BATCH_SIZE) {
            let chunk_json = serde_json::to_string_pretty(&chunk.iter().map(|u| {
                json!({
                    "start": u.start,
                    "end": u.end,
                    "speaker": u.speaker,
                    "text": u.text
                })
            }).collect::<Vec<_>>()).unwrap_or_default();

            let batch = run(&refine_system, &chunk_json, 2048, 0.2)
                .ok()
                .and_then(|response| parse_json_array(&response))
                .and_then(|items| map_refined(chunk, items));
            match batch {
                Some(items) if !items.is_empty() => refined.extend(items),
                _ => {
                    failed += 1;
                    refined.extend_from_slice(chunk);
                }
            }
        }

        if failed == total_batches {
            return Err("Модель не смогла восстановить ни одной реплики. Проверьте, что рантайм Qwen установлен, и повторите попытку.".to_string());
        }
        Ok((refined, failed))
    })
    .map_err(|error| (503, error))?;

    let utts_val = serde_json::to_value(&refined_utts).map_err(|e| (500, e.to_string()))?;
    data["utterances"] = utts_val.clone();

    let mut unique_speakers: Vec<String> = refined_utts.iter().map(|u| u.speaker.clone()).collect();
    unique_speakers.sort();
    unique_speakers.dedup();

    let mut speakers_info: Vec<Value> = Vec::with_capacity(unique_speakers.len());
    for spk_name in &unique_speakers {
        let matching: Vec<&Utterance> = refined_utts
            .iter()
            .filter(|u| &u.speaker == spk_name)
            .collect();
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
        // Имя могло сохраниться: не теряем привязку к голосовому отпечатку.
        let old = previous_info
            .iter()
            .find(|p| p["name"].as_str() == Some(spk_name.as_str()));
        let mut info = old
            .filter(|p| p.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"name": spk_name}));
        if let Some(object) = info.as_object_mut() {
            object.insert("name".to_string(), json!(spk_name));
            object.insert("utterances_count".to_string(), json!(count));
            object.insert("total_speech_sec".to_string(), json!(speech));
            object.insert("samples".to_string(), json!(samples));
        }
        speakers_info.push(info);
    }

    let meta = data
        .get_mut("meta")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| (400, "В расшифровке нет метаданных.".to_string()))?;
    meta.insert("speakers".to_string(), json!(unique_speakers));
    meta.insert("speakers_info".to_string(), json!(speakers_info));
    if let Some(engine) = meta
        .get("engine")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        if !engine.contains("Qwen") {
            meta.insert(
                "engine".to_string(),
                json!(format!("{engine} + Qwen-реставрация")),
            );
        }
    }

    if let Some(stats) = data.get_mut("stats").and_then(Value::as_object_mut) {
        let words = refined_utts
            .iter()
            .map(|u| u.text.split_whitespace().count())
            .sum::<usize>();
        stats.insert("segments".to_string(), json!(refined_utts.len()));
        stats.insert("words".to_string(), json!(words));
        stats.insert(
            "speech_sec".to_string(),
            json!((document::unique_speech_sec(&refined_utts) * 10.0).round() / 10.0),
        );
    }

    let markdown = report::build_markdown(&data["meta"], &refined_utts);
    let text = report::build_text(&refined_utts);
    let rev = serde_json::from_value::<Vec<mr_summarize::Utterance>>(utts_val)
        .ok()
        .map(|items| mr_summarize::artifact::source_rev(&items));

    let json_str = serde_json::to_string_pretty(&data).map_err(|e| (500, e.to_string()))?;

    if std::fs::read_to_string(&json_path).ok().as_deref() != Some(source.as_str()) {
        return Err((
            409,
            "Файл расшифровки изменился во время реставрации. Откройте его заново.".to_string(),
        ));
    }
    let backup = json_path.with_file_name(format!(
        "{}.bak",
        json_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    write_atomic(&backup, source.as_bytes())
        .map_err(|e| (500, format!("Не удалось сохранить резервную копию: {e}")))?;

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
        "failed_batches": failed_batches,
    }))
}

fn parse_json_array(raw: &str) -> Option<Vec<RefinedUtt>> {
    let start = raw.find('[')?;
    let mut deserializer = serde_json::Deserializer::from_str(&raw[start..]);
    Vec::<RefinedUtt>::deserialize(&mut deserializer).ok()
}

fn map_refined(chunk: &[Utterance], items: Vec<RefinedUtt>) -> Option<Vec<Utterance>> {
    let start = chunk.iter().map(|u| u.start).fold(f64::INFINITY, f64::min);
    let end = chunk
        .iter()
        .map(|u| u.end)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let text = item.text.trim();
        let speaker = item.speaker.trim();
        if !item.start.is_finite()
            || !item.end.is_finite()
            || item.start < start - 0.5
            || item.end > end + 0.5
            || item.end < item.start
            || text.is_empty()
            || speaker.is_empty()
        {
            return None;
        }
        let clamped_start = item.start.clamp(start, end);
        let clamped_end = item.end.clamp(clamped_start, end);
        out.push(Utterance {
            start: clamped_start,
            end: clamped_end,
            speaker: speaker.to_string(),
            channel: channel_at(chunk, clamped_start),
            text: text.to_string(),
            words: Vec::new(),
            diarized: true,
            silhouette: 0.0,
            ctc_used: false,
        });
    }
    Some(out)
}

fn channel_at(chunk: &[Utterance], at: f64) -> Option<i32> {
    chunk
        .iter()
        .find(|u| at >= u.start && at < u.end)
        .or_else(|| {
            chunk
                .iter()
                .min_by(|a, b| (a.start - at).abs().total_cmp(&(b.start - at).abs()))
        })
        .and_then(|u| u.channel)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utt(start: f64, end: f64, channel: Option<i32>) -> Utterance {
        Utterance {
            start,
            end,
            speaker: "Спикер 1".to_string(),
            channel,
            text: "привет".to_string(),
            words: Vec::new(),
            diarized: true,
            silhouette: 0.0,
            ctc_used: false,
        }
    }

    fn item(start: f64, end: f64, speaker: &str, text: &str) -> RefinedUtt {
        RefinedUtt {
            start,
            end,
            speaker: speaker.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn parse_array_ignores_trailing_prose() {
        let parsed = parse_json_array(
            "Вот результат:\n[{\"start\":0.0,\"end\":1.0,\"speaker\":\"A\",\"text\":\"x\"}]\nГотово.",
        )
        .expect("массив должен разобраться");
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn map_keeps_channel_and_rejects_bad_items() {
        let chunk = vec![utt(0.0, 2.0, Some(1))];
        let mapped = map_refined(&chunk, vec![item(0.2, 1.0, "Спикер 1", "тест")])
            .expect("валидная реплика");
        assert_eq!(mapped[0].channel, Some(1));

        assert!(map_refined(&chunk, vec![item(1.0, 3.0, "Спикер 1", "тест")]).is_none());
        assert!(map_refined(&chunk, vec![item(0.2, 1.0, "", "тест")]).is_none());
        assert!(map_refined(&chunk, vec![item(f64::NAN, 1.0, "Спикер 1", "тест")]).is_none());
    }
}
