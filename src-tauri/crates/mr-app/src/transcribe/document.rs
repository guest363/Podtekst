//! Канонический `.transcript.json`: одно описание формата для записи и чтения.
//! Старые файлы читаются за счёт `serde(default)`, схема — для будущих миграций.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Utterance;

pub const SCHEMA_VERSION: u32 = 1;
const MAX_JSON_BYTES: usize = 50 * 1024 * 1024;
const MAX_VIOLATIONS: usize = 200;
const TIME_TOLERANCE: f64 = 0.15;

fn schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TranscriptDoc {
    #[serde(default = "schema_version")]
    pub schema: u32,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub stats: Stats,
    #[serde(default)]
    pub utterances: Vec<Utterance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Meta {
    #[serde(default)]
    pub audio: String,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub speaker_method: String,
    #[serde(default)]
    pub speakers: Vec<String>,
    /// Карточки спикеров формирует UI; сохраняем их как есть, чтобы правка
    /// расшифровки не теряла поля, которых нет в этом крейте.
    #[serde(default)]
    pub speakers_info: Vec<serde_json::Value>,
    #[serde(default)]
    pub hotwords_applied: Vec<String>,
    #[serde(default)]
    pub confidence: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Stats {
    #[serde(default)]
    pub segments: usize,
    #[serde(default)]
    pub words: usize,
    #[serde(default)]
    pub speech_sec: f64,
    #[serde(default)]
    pub diarized: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("файл не найден: {0}")]
    NotFound(PathBuf),
    #[error("ошибка чтения {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("битый JSON {path}: {message} (строка {line}, столбец {column})")]
    Json {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    #[error("неподдерживаемая схема {schema} в {path}")]
    UnsupportedSchema { path: PathBuf, schema: u32 },
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub index: Option<usize>,
    pub field: &'static str,
    pub value: String,
    pub rule: &'static str,
}

fn violation(
    violations: &mut Vec<Violation>,
    index: Option<usize>,
    field: &'static str,
    value: impl Into<String>,
    rule: &'static str,
) {
    if violations.len() < MAX_VIOLATIONS {
        violations.push(Violation {
            index,
            field,
            value: value.into(),
            rule,
        });
    }
}

pub fn read_transcript(path: &Path) -> Result<TranscriptDoc, TranscriptError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(TranscriptError::NotFound(path.to_path_buf()));
        }
        Err(source) => {
            log::warn!(
                target: "json",
                path = crate::logging::file_token(path),
                error = source.to_string();
                "ev=json.load"
            );
            return Err(TranscriptError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut doc: TranscriptDoc = match serde_json::from_slice(&bytes) {
        Ok(doc) => doc,
        Err(error) => {
            let head: String = String::from_utf8_lossy(&bytes)
                .chars()
                .take(200)
                .collect::<String>();
            log::warn!(
                target: "json",
                path = crate::logging::file_token(path),
                bytes = bytes.len(),
                error = error.to_string(),
                line = error.line(),
                col = error.column(),
                head = crate::logging::text_preview(&head);
                "ev=json.load"
            );
            return Err(TranscriptError::Json {
                path: path.to_path_buf(),
                line: error.line(),
                column: error.column(),
                message: error.to_string(),
            });
        }
    };
    if doc.schema == 0 {
        doc.schema = SCHEMA_VERSION;
    }
    if doc.schema > SCHEMA_VERSION {
        log::warn!(
            target: "json",
            path = crate::logging::file_token(path),
            schema = doc.schema,
            supported = SCHEMA_VERSION;
            "ev=json.load"
        );
        return Err(TranscriptError::UnsupportedSchema {
            path: path.to_path_buf(),
            schema: doc.schema,
        });
    }
    Ok(doc)
}

/// Уникальное покрытие таймлайна: сумма по каналам завышает длительность
/// из-за одновременной речи микрофона и системы.
pub fn unique_speech_sec(utterances: &[Utterance]) -> f64 {
    let mut spans: Vec<(f64, f64)> = utterances
        .iter()
        .map(|u| (u.start.max(0.0), u.end.max(u.start)))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut covered = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for (start, end) in spans {
        match current {
            None => current = Some((start, end)),
            Some((a, b)) if start <= b => current = Some((a, b.max(end))),
            Some((a, b)) => {
                covered += b - a;
                current = Some((start, end));
            }
        }
    }
    if let Some((a, b)) = current {
        covered += b - a;
    }
    covered
}

fn check_utterances(doc: &TranscriptDoc, violations: &mut Vec<Violation>) {
    let known: HashSet<&str> = doc.meta.speakers.iter().map(String::as_str).collect();
    let mut last_start = f64::NEG_INFINITY;
    let mut last_end_by_channel: HashMap<Option<i32>, f64> = HashMap::new();

    for (index, utterance) in doc.utterances.iter().enumerate() {
        if !utterance.start.is_finite()
            || !utterance.end.is_finite()
            || utterance.start < 0.0
            || utterance.start > utterance.end
        {
            violation(
                violations,
                Some(index),
                "start/end",
                format!("{}..{}", utterance.start, utterance.end),
                "finite_start_end",
            );
        }
        if utterance.start < last_start {
            violation(
                violations,
                Some(index),
                "start",
                utterance.start.to_string(),
                "sorted_by_start",
            );
        }
        last_start = last_start.max(utterance.start);
        if let Some(previous_end) = last_end_by_channel.get(&utterance.channel).copied() {
            if utterance.start < previous_end - 0.01 {
                violation(
                    violations,
                    Some(index),
                    "start",
                    format!("{} < {}", utterance.start, previous_end),
                    "no_overlap_same_channel",
                );
            }
        }
        let entry = last_end_by_channel
            .entry(utterance.channel)
            .or_insert(utterance.end);
        *entry = entry.max(utterance.end);

        if utterance.speaker.trim().is_empty() {
            violation(
                violations,
                Some(index),
                "speaker",
                "empty",
                "speaker_not_empty",
            );
        } else if !known.contains(utterance.speaker.as_str()) {
            violation(
                violations,
                Some(index),
                "speaker",
                crate::logging::speaker_token(&utterance.speaker),
                "speaker_known",
            );
        }

        for (word_index, word) in utterance.words.iter().enumerate() {
            let timed = word.start_sec != 0.0 || word.end_sec != 0.0;
            if timed
                && (!word.start_sec.is_finite()
                    || !word.end_sec.is_finite()
                    || word.start_sec > word.end_sec
                    || word.start_sec < utterance.start - 0.5
                    || word.end_sec > utterance.end + 0.5)
            {
                violation(
                    violations,
                    Some(index),
                    "words",
                    format!("#{word_index} {}..{}", word.start_sec, word.end_sec),
                    "word_timing",
                );
            }
            if !(0.0..=1.0).contains(&word.conf) {
                violation(
                    violations,
                    Some(index),
                    "words.conf",
                    word.conf.to_string(),
                    "conf_in_range",
                );
            }
        }
        if utterance.ctc_used && utterance.words.is_empty() {
            violation(
                violations,
                Some(index),
                "ctc_used",
                "true",
                "ctc_requires_words",
            );
        }
        if utterance
            .text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
            || utterance.text.contains('\u{FFFD}')
        {
            violation(
                violations,
                Some(index),
                "text",
                format!("{} chars", utterance.text.chars().count()),
                "text_clean",
            );
        }
    }
    check_duplicates(doc, violations);
}

// Отдельная функция: в цикле выше нужен доступ к соседней реплике.
fn check_duplicates(doc: &TranscriptDoc, violations: &mut Vec<Violation>) {
    for index in 1..doc.utterances.len() {
        let previous = &doc.utterances[index - 1];
        let current = &doc.utterances[index];
        if previous.speaker == current.speaker && previous.text == current.text {
            violation(
                violations,
                Some(index),
                "text",
                "duplicate",
                "no_adjacent_duplicates",
            );
        }
    }
}

fn check_stats(doc: &TranscriptDoc, violations: &mut Vec<Violation>) {
    if doc.stats.segments != doc.utterances.len() {
        violation(
            violations,
            None,
            "stats.segments",
            doc.stats.segments.to_string(),
            "segments_match",
        );
    }
    let words: usize = doc
        .utterances
        .iter()
        .map(|u| u.text.split_whitespace().count())
        .sum();
    if doc.stats.words != words {
        violation(
            violations,
            None,
            "stats.words",
            doc.stats.words.to_string(),
            "words_match",
        );
    }
    let coverage = unique_speech_sec(&doc.utterances);
    if (doc.stats.speech_sec - coverage).abs() > TIME_TOLERANCE {
        violation(
            violations,
            None,
            "stats.speech_sec",
            doc.stats.speech_sec.to_string(),
            "speech_sec_match",
        );
    }
}

fn check_speakers_info(doc: &TranscriptDoc, violations: &mut Vec<Violation>) {
    let mut counts: HashMap<&str, (usize, f64)> = HashMap::new();
    for utterance in &doc.utterances {
        let entry = counts.entry(utterance.speaker.as_str()).or_default();
        entry.0 += 1;
        entry.1 += (utterance.end - utterance.start).max(0.0);
    }
    let mut covered = HashSet::new();
    for info in &doc.meta.speakers_info {
        let Some(name) = info.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        covered.insert(name);
        let Some(&(count, speech)) = counts.get(name) else {
            continue;
        };
        if let Some(present) = info.get("utterances_count").and_then(|v| v.as_u64()) {
            if present != count as u64 {
                violation(
                    violations,
                    None,
                    "speakers_info.utterances_count",
                    format!("{present} != {count}"),
                    "speaker_count_match",
                );
            }
        }
        if let Some(present) = info.get("total_speech_sec").and_then(|v| v.as_f64()) {
            if (present - speech).abs() > TIME_TOLERANCE {
                violation(
                    violations,
                    None,
                    "speakers_info.total_speech_sec",
                    present.to_string(),
                    "speaker_speech_match",
                );
            }
        }
    }
    for name in counts.keys() {
        if !covered.contains(name) {
            violation(
                violations,
                None,
                "speakers_info.name",
                crate::logging::speaker_token(name),
                "speaker_card_exists",
            );
        }
    }
}

fn check_size(doc: &TranscriptDoc, violations: &mut Vec<Violation>) {
    if let Ok(bytes) = serde_json::to_vec(doc) {
        if bytes.len() > MAX_JSON_BYTES {
            violation(
                violations,
                None,
                "size",
                bytes.len().to_string(),
                "size_limit",
            );
        }
    }
}

pub fn validate(doc: &TranscriptDoc) -> Vec<Violation> {
    let mut violations = Vec::new();
    check_utterances(doc, &mut violations);
    check_stats(doc, &mut violations);
    check_speakers_info(doc, &mut violations);
    check_size(doc, &mut violations);
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utterance(start: f64, end: f64, speaker: &str, text: &str) -> Utterance {
        Utterance {
            start,
            end,
            speaker: speaker.to_string(),
            channel: Some(0),
            text: text.to_string(),
            diarized: true,
            silhouette: 0.5,
            words: Vec::new(),
            ctc_used: false,
        }
    }

    fn doc(utterances: Vec<Utterance>) -> TranscriptDoc {
        let speech = unique_speech_sec(&utterances);
        let words = utterances
            .iter()
            .map(|u| u.text.split_whitespace().count())
            .sum();
        let speakers = utterances
            .iter()
            .map(|u| u.speaker.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let info = speakers
            .iter()
            .map(|name| {
                let matching: Vec<&Utterance> =
                    utterances.iter().filter(|u| &u.speaker == name).collect();
                serde_json::json!({
                    "name": name,
                    "utterances_count": matching.len(),
                    "total_speech_sec": matching
                        .iter()
                        .map(|u| (u.end - u.start).max(0.0))
                        .sum::<f64>(),
                })
            })
            .collect::<Vec<_>>();
        TranscriptDoc {
            schema: SCHEMA_VERSION,
            meta: Meta {
                speakers,
                speakers_info: info,
                ..Meta::default()
            },
            stats: Stats {
                segments: utterances.len(),
                words,
                speech_sec: (speech * 10.0).round() / 10.0,
                diarized: true,
            },
            utterances,
        }
    }

    #[test]
    fn old_document_without_schema_and_words_parses() {
        let value = serde_json::json!({
            "meta": {"audio": "a.wav", "speakers": ["S"]},
            "stats": {"segments": 1, "words": 2, "speech_sec": 1.0},
            "utterances": [
                {"start": 0.0, "end": 1.0, "speaker": "S", "text": "раз два"}
            ]
        });
        let text = serde_json::to_string(&value).unwrap();
        let dir = std::env::temp_dir().join(format!("mr-doc-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.transcript.json");
        std::fs::write(&path, text).unwrap();
        let doc = read_transcript(&path).unwrap();
        assert_eq!(doc.schema, SCHEMA_VERSION);
        assert_eq!(doc.utterances.len(), 1);
        assert!(doc.utterances[0].words.is_empty());
        assert!(!doc.utterances[0].ctc_used);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_json_reports_line_and_column() {
        let dir = std::env::temp_dir().join(format!("mr-doc-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.transcript.json");
        std::fs::write(&path, b"{\"meta\": ").unwrap();
        match read_transcript(&path) {
            Err(TranscriptError::Json { line, column, .. }) => {
                assert!(line > 0);
                assert!(column > 0);
            }
            other => panic!("ожидалась ошибка JSON: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_document_has_no_violations_and_roundtrips() {
        let doc = doc(vec![
            utterance(0.0, 1.0, "A", "раз два"),
            utterance(1.0, 2.0, "B", "три"),
        ]);
        assert!(validate(&doc).is_empty(), "{:?}", validate(&doc));
        let json = serde_json::to_string_pretty(&doc).unwrap();
        let back: TranscriptDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stats.segments, doc.stats.segments);
        assert_eq!(back.utterances.len(), doc.utterances.len());
        assert_eq!(back.meta.speakers, doc.meta.speakers);
    }

    #[test]
    fn catches_nan_unsorted_overlap_and_unknown_speaker() {
        let mut broken = doc(vec![
            utterance(2.0, 3.0, "A", "вторая"),
            utterance(1.5, 2.5, "B", "первая"),
        ]);
        broken.utterances[0].end = f64::NAN;
        broken.meta.speakers = vec!["A".to_string()];
        let rules: Vec<&str> = validate(&broken).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"finite_start_end"), "{rules:?}");
        assert!(rules.contains(&"sorted_by_start"), "{rules:?}");
        assert!(rules.contains(&"speaker_known"), "{rules:?}");

        let overlap = doc(vec![
            utterance(0.0, 2.0, "A", "первая"),
            utterance(1.5, 3.0, "B", "вторая"),
        ]);
        let rules: Vec<&str> = validate(&overlap).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"no_overlap_same_channel"), "{rules:?}");
    }

    #[test]
    fn catches_word_and_stats_mismatch() {
        let mut broken = doc(vec![utterance(0.0, 1.0, "A", "раз два")]);
        broken.utterances[0].words = vec![super::super::WordConf {
            w: "раз".into(),
            conf: 2.0,
            needs_review: false,
            start_sec: 5.0,
            end_sec: 6.0,
        }];
        broken.utterances[0].ctc_used = true;
        broken.stats.segments = 99;
        broken.stats.words = 99;
        let rules: Vec<&str> = validate(&broken).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"word_timing"), "{rules:?}");
        assert!(rules.contains(&"conf_in_range"), "{rules:?}");
        assert!(rules.contains(&"segments_match"), "{rules:?}");
        assert!(rules.contains(&"words_match"), "{rules:?}");
    }

    #[test]
    fn ctc_without_words_is_invariant_violation() {
        let mut broken = doc(vec![utterance(0.0, 1.0, "A", "раз")]);
        broken.utterances[0].ctc_used = true;
        let rules: Vec<&str> = validate(&broken).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"ctc_requires_words"), "{rules:?}");
    }

    #[test]
    fn duplicate_and_control_text_are_reported() {
        let mut control = doc(vec![utterance(0.0, 1.0, "A", "чистый текст")]);
        control.utterances[0].text = "текст\u{7}".to_string();
        let rules: Vec<&str> = validate(&control).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"text_clean"), "{rules:?}");

        let duplicate = doc(vec![
            utterance(0.0, 1.0, "A", "одно и то же"),
            utterance(1.0, 2.0, "A", "одно и то же"),
        ]);
        let rules: Vec<&str> = validate(&duplicate).iter().map(|v| v.rule).collect();
        assert!(rules.contains(&"no_adjacent_duplicates"), "{rules:?}");
    }
}
