use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::{unique_speech_sec, TranscriptDoc};

const MAX_JSON_BYTES: usize = 50 * 1024 * 1024;
const MAX_VIOLATIONS: usize = 200;
const TIME_TOLERANCE: f64 = 0.15;

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
