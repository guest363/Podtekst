use crate::cues;
use crate::text;

use super::{is_placeholder_name, known_name, name_key, TaskKind, FIRST_PERSON};

pub(crate) fn assignee_for(
    task_text: &str,
    support: Option<&str>,
    speaker: Option<&str>,
    known: &[String],
    kind: TaskKind,
) -> Option<String> {
    let support_norm = support.map(text::normalize).unwrap_or_default();
    let task_norm = text::normalize(task_text);
    let speaker = speaker.map(str::trim).filter(|s| !s.is_empty());

    if kind == TaskKind::Need {
        // «Надо/нужно» не назначает исполнителя само по себе: только явное
        // имя говорящего в тексте задачи (в т.ч. «Вере нужно…»).
        return known_name_in_text(&task_norm, known).or_else(|| label_assignee(task_text, known));
    }

    let first_person = FIRST_PERSON
        .iter()
        .any(|marker| support_norm.contains(marker) || task_norm.contains(marker));
    if first_person {
        if let Some(speaker) = speaker.filter(|s| !is_placeholder_name(s)) {
            return Some(speaker.to_owned());
        }
    }
    if let Some(name) = speaker_name_in_text(speaker, &support_norm, &task_norm) {
        return Some(name);
    }
    if let Some(name) = label_assignee(task_text, known) {
        return Some(name);
    }
    extract_assignee_name(task_text, known)
}

fn speaker_name_in_text(speaker: Option<&str>, support: &str, task_text: &str) -> Option<String> {
    let speaker = speaker?;
    if is_placeholder_name(speaker) {
        return None;
    }
    let key = name_key(speaker);
    let appears = support
        .split_whitespace()
        .chain(task_text.split_whitespace())
        .any(|word| name_key(word) == key);
    appears.then(|| speaker.to_owned())
}

fn known_name_in_text(text: &str, known: &[String]) -> Option<String> {
    for word in text.split_whitespace() {
        if let Some(name) = known_name(word, known) {
            return Some(name);
        }
    }
    None
}

fn label_assignee(text: &str, _known: &[String]) -> Option<String> {
    for part in text.split(';') {
        let Some((label, value)) = part.split_once(':') else {
            continue;
        };
        let normalized = text::normalize(label);
        if normalized == "кто" || normalized == "ответственный" {
            let value = value.trim().trim_matches('*').trim();
            if !value.is_empty() && !is_placeholder_name(value) {
                return Some(value.to_owned());
            }
        }
    }
    None
}

pub(crate) fn extract_assignee_name(text: &str, known: &[String]) -> Option<String> {
    if known.is_empty() {
        return None;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let Some(name) = known_name(word, known) else {
            continue;
        };
        let prev_verb = index
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(|w| cues::action_verb(w))
            .unwrap_or(false);
        let next_verb = words
            .iter()
            .skip(index + 1)
            .take(2)
            .any(|w| cues::action_verb(w));
        if prev_verb || next_verb {
            return Some(name);
        }
    }
    None
}
