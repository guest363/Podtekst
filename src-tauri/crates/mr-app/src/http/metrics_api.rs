use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
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
    #[serde(default)]
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
    #[serde(default)]
    matched_pack_id: Option<String>,
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

    let (names, persons) = identities(&doc);
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
    let source = transcript.display().to_string();
    let record = query_param(query, "record").as_deref() == Some("1");
    let key = cache_key(&transcript, audio.as_deref());
    if let Some(hit) = cache_get(&key) {
        let mut metrics = hit;
        crate::metrics::apply(&mut metrics, &source, &persons, record);
        respond(request, metrics);
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

    let started = std::time::Instant::now();
    let mut metrics = mr_metrics::analyze(&turns, duration_sec, audio.as_deref(), channels, &names);
    cache_put(key, metrics.clone());
    crate::metrics::apply(&mut metrics, &source, &persons, record);
    log::debug!(
        target: "metrics",
        file = crate::logging::file_token(&media),
        turns = turns.len(),
        channels,
        ms = started.elapsed().as_millis() as u64;
        "ev=metrics.compute"
    );

    respond(request, metrics);
}

pub(super) fn serve_reset_baseline(request: tiny_http::Request) {
    match crate::metrics::reset() {
        Ok(()) => json_response(request, serde_json::json!({"ok": true}), 200),
        Err(error) => {
            log::warn!(target: "metrics", "ev=baseline.reset error={error}");
            json_response(
                request,
                serde_json::json!({"error": "Не удалось сбросить базовую линию"}),
                500,
            );
        }
    }
}

fn identities(doc: &Doc) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut names = HashMap::new();
    let mut persons = HashMap::new();
    for speaker in &doc.meta.speakers_info {
        let display = speaker
            .matched_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&speaker.name)
            .to_string();
        let person = speaker
            .matched_pack_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&display)
            .to_string();
        for alias in [&speaker.id, &speaker.name, &display] {
            if alias.is_empty() {
                continue;
            }
            names
                .entry(alias.clone())
                .or_insert_with(|| display.clone());
            persons.insert(alias.clone(), person.clone());
        }
    }
    (names, persons)
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

fn cache_key(path: &Path, audio: Option<&Path>) -> String {
    let mut key = file_key(path);
    key.push('|');
    match audio {
        Some(audio) => key.push_str(&file_key(audio)),
        None => key.push_str("noaudio"),
    }
    key
}

fn file_key(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}|{len}|{mtime}", path.display())
}

fn cache_get(key: &str) -> Option<Metrics> {
    let hit = cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(key)
        .cloned();
    if hit.is_some() {
        let mut order = order().lock().unwrap_or_else(|error| error.into_inner());
        if let Some(position) = order.iter().position(|item| item == key) {
            order.remove(position);
        }
        order.push_back(key.to_string());
    }
    hit
}

fn cache_put(key: String, metrics: Metrics) {
    cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(key.clone(), metrics);
    let mut order = order().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(position) = order.iter().position(|item| item == &key) {
        order.remove(position);
    }
    order.push_back(key);
    while order.len() > 8 {
        if let Some(old) = order.pop_front() {
            cache()
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&old);
        }
    }
}

fn cache() -> &'static Mutex<HashMap<String, Metrics>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Metrics>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn order() -> &'static Mutex<VecDeque<String>> {
    static ORDER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    ORDER.get_or_init(|| Mutex::new(VecDeque::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_resolve_display_names_and_pack() {
        let doc = Doc {
            meta: Meta {
                audio: String::new(),
                speakers_info: vec![SpeakerJson {
                    id: "spk-A".to_string(),
                    name: "Спикер A".to_string(),
                    matched_name: Some("Я".to_string()),
                    matched_pack_id: Some("pack-1".to_string()),
                }],
            },
            utterances: Vec::new(),
        };
        let (names, persons) = identities(&doc);
        assert_eq!(names.get("Спикер A"), Some(&"Я".to_string()));
        assert_eq!(names.get("spk-A"), Some(&"Я".to_string()));
        assert_eq!(persons.get("Спикер A"), Some(&"pack-1".to_string()));
        assert_eq!(persons.get("Я"), Some(&"pack-1".to_string()));
    }
}
