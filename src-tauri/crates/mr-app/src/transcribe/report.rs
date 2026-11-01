use super::{Sample, SpeakerInfo, Utterance};

pub(crate) fn human_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn format_ts(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

pub(crate) fn speaker_to_json(s: &SpeakerInfo) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "channel": s.channel,
        "matched": s.matched,
        "matched_name": s.matched_name,
        "similarity": s.similarity,
        "silhouette": s.silhouette,
        "candidates": s.candidates.iter().map(|(n, v)| serde_json::json!({"name": n, "similarity": v})).collect::<Vec<_>>(),
        "embedding": s.embedding,
        "sample": s.samples.first().map(sample_to_json),
        "samples": s.samples.iter().map(sample_to_json).collect::<Vec<_>>(),
        "utterances_count": s.utterances_count,
        "total_speech_sec": s.total_speech_sec,
    })
}

fn sample_to_json(s: &Sample) -> serde_json::Value {
    serde_json::json!({
        "start": s.start, "end": s.end, "dur": s.dur,
        "text": s.text, "full_text": s.full_text,
        "channel": s.channel,
    })
}

pub(crate) fn build_markdown(meta: &serde_json::Value, utterances: &[Utterance]) -> String {
    let mut lines = vec![];
    lines.push(format!(
        "# {}",
        meta.get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Расшифровка встречи")
    ));
    lines.push(String::new());
    lines.push(format!(
        "**Источник:** `{}` ({})",
        meta.get("audio").and_then(|v| v.as_str()).unwrap_or("?"),
        meta.get("duration").and_then(|v| v.as_str()).unwrap_or("?")
    ));
    lines.push(format!(
        "**Движок:** {} (работает на этом компьютере)",
        meta.get("engine").and_then(|v| v.as_str()).unwrap_or("?")
    ));
    lines.push(format!(
        "**Метод спикеров:** {}",
        meta.get("speaker_method")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    ));
    if let Some(sp) = meta.get("speakers").and_then(|v| v.as_array()) {
        if !sp.is_empty() {
            let names: Vec<String> = sp
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            lines.push(format!("**Спикеры:** {}", names.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push("---".to_string());
    lines.push(String::new());
    for u in utterances {
        lines.push(format!(
            "**[{}] {}:** {}",
            format_ts(u.start),
            u.speaker,
            u.text
        ));
        lines.push(String::new());
    }
    let mut s = lines.join("\n");
    while s.ends_with('\n') {
        s.pop();
    }
    s + "\n"
}

pub(crate) fn build_text(utterances: &[Utterance]) -> String {
    utterances
        .iter()
        .map(|u| format!("[{}] {}: {}", format_ts(u.start), u.speaker, u.text))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn record_stats(audio_duration_sec: f64) {
    let mut should_emit = false;
    let mut emit_count = 0u64;
    let mut emit_saved = 0.0f64;
    crate::config::update_config(|cfg| {
        let count = cfg
            .get("transcribed_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            + 1;
        let saved = cfg
            .get("total_saved_minutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            + (audio_duration_sec / 60.0 * 3.0).max(2.0);
        let rounded_saved = (saved * 10.0).round() / 10.0;
        cfg["transcribed_count"] = count.into();
        cfg["total_saved_minutes"] = rounded_saved.into();
        let dismissed = cfg
            .get("donate_prompt_dismissed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !dismissed {
            should_emit = true;
            emit_count = count;
            emit_saved = rounded_saved;
        }
    });
    if should_emit {
        super::events::emit(serde_json::json!({
            "type": "stats",
            "transcribed_count": emit_count,
            "total_saved_minutes": emit_saved,
            "donate_prompt_dismissed": false,
            "should_prompt_donate": true,
        }));
    }
}
