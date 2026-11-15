use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use mr_metrics::Metrics;
use serde::Deserialize;

use super::media::inside_save_dir;
use super::{json_response, query_param};
use crate::config::save_dir;

#[derive(Debug, Default, Deserialize)]
struct Doc {
    #[serde(default)]
    meta: Meta,
    utterances: Vec<UtteranceJson>,
}

#[derive(Debug, Default, Deserialize)]
struct Meta {
    #[serde(default)]
    audio: String,
    #[serde(default)]
    speakers_info: Vec<SpeakerJson>,
}

#[derive(Debug, Default, Deserialize)]
struct SpeakerJson {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    matched_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UtteranceJson {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    speaker: String,
    #[serde(default)]
    channel: Option<i64>,
    #[serde(default)]
    text: String,
}

pub(super) fn serve_metrics(request: tiny_http::Request, query: &str) {
    let Some(file) = query_param(query, "file") else {
        json_response(
            request,
            serde_json::json!({"error": "No file specified"}),
            400,
        );
        return;
    };
    let media = {
        let raw = PathBuf::from(&file);
        if raw.is_absolute() {
            raw
        } else {
            save_dir().join(raw)
        }
    };
    if media.file_name().is_none() || !inside_save_dir(&media) {
        json_response(
            request,
            serde_json::json!({"error": "Файл вне папки записей"}),
            404,
        );
        return;
    }
    let stem = media
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = media.parent().map(PathBuf::from).unwrap_or_else(save_dir);
    let transcript = parent.join(format!("{stem}.transcript.json"));
    let Ok(text) = std::fs::read_to_string(&transcript) else {
        json_response(
            request,
            serde_json::json!({"error": "Нет расшифровки для этой записи"}),
            404,
        );
        return;
    };
    let Ok(doc) = serde_json::from_str::<Doc>(&text) else {
        json_response(
            request,
            serde_json::json!({"error": "Битый файл расшифровки"}),
            422,
        );
        return;
    };
    let key = cache_key(&transcript);
    if let Some(hit) = cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&key)
        .cloned()
    {
        respond(request, hit);
        return;
    }

    let turns: Vec<mr_metrics::Turn> = doc
        .utterances
        .iter()
        .map(|item| mr_metrics::Turn {
            start: item.start,
            end: item.end,
            speaker: item.speaker.clone(),
            channel: item.channel.and_then(|value| usize::try_from(value).ok()),
            text: item.text.clone(),
        })
        .collect();
    let names: HashMap<String, String> = doc
        .meta
        .speakers_info
        .iter()
        .filter(|speaker| !speaker.id.is_empty())
        .map(|speaker| {
            let display = speaker
                .matched_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&speaker.name);
            (speaker.id.clone(), display.to_string())
        })
        .collect();

    let audio_path = if doc.meta.audio.is_empty() {
        Some(media.clone())
    } else {
        let candidate = parent.join(&doc.meta.audio);
        Some(candidate).filter(|path| path.exists() && inside_save_dir(path))
    };
    let (audio, channels, duration_sec) = audio_path
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        })
        .and_then(|path| {
            hound::WavReader::open(&path).ok().map(|reader| {
                let spec = reader.spec();
                let duration = reader.duration() as f64 / spec.sample_rate as f64;
                (path, spec.channels as usize, duration)
            })
        })
        .map_or((None, 0, 0.0), |(path, channels, duration)| {
            (Some(path), channels, duration)
        });

    let started = std::time::Instant::now();
    let mut metrics = mr_metrics::analyze(&turns, duration_sec, audio.as_deref(), channels, &names);
    crate::metrics::apply(&mut metrics, &stem);
    log::debug!(
        target: "metrics",
        file = crate::logging::file_token(&media),
        turns = turns.len(),
        channels,
        ms = started.elapsed().as_millis() as u64;
        "ev=metrics.compute"
    );

    cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key.clone(), metrics.clone());
    let mut order = order().lock().unwrap_or_else(|error| error.into_inner());
    order.push_back(key);
    while order.len() > 8 {
        if let Some(old) = order.pop_front() {
            cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&old);
        }
    }

    respond(request, metrics);
}

fn respond(request: tiny_http::Request, metrics: Metrics) {
    match serde_json::to_value(&metrics) {
        Ok(value) => json_response(request, value, 200),
        Err(_) => json_response(
            request,
            serde_json::json!({"error": "Не удалось собрать метрики"}),
            500,
        ),
    }
}

fn cache_key(path: &PathBuf) -> String {
    let meta = std::fs::metadata(path).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}|{len}|{mtime}", path.display())
}

fn cache() -> &'static Mutex<HashMap<String, Metrics>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Metrics>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn order() -> &'static Mutex<VecDeque<String>> {
    static ORDER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    ORDER.get_or_init(|| Mutex::new(VecDeque::new()))
}
