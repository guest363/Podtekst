use std::collections::HashSet;

use crate::artifact::{DeadlineSource, SummaryItem, SummaryRef, TaskMeta};
use crate::cues;
use crate::deadline;
use crate::preset::{Preset, SectionRole};
use crate::text::{self, Transcript, Utterance};

mod assignee;
pub(crate) use assignee::assignee_for;

pub(crate) struct Candidate {
    pub section_id: &'static str,
    pub item: SummaryItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKind {
    Explicit,
    Need,
}

const FIRST_PERSON: &[&str] = &[
    "я сделаю",
    "я подготовлю",
    "я проверю",
    "я обновлю",
    "я доделаю",
    "я позвоню",
    "я напишу",
    "я отправлю",
    "я добавлю",
    "я назначу",
    "я выдам",
    "я проведу",
    "я заполню",
    "я перенесу",
    "я уточню",
    "я соберу",
    "я оформлю",
    "я загружу",
    "я настрою",
    "я пришлю",
    "я согласую",
    "беру",
    "отвечаю",
];

/// Детерминированный fallback: только явные source-реплики, без новых фактов.
pub(crate) fn candidates(transcript: &Transcript, preset: &Preset) -> Vec<Candidate> {
    let decisions = section_id(preset, &["decisions"]);
    let tasks = section_id(preset, &["tasks", "next_steps", "todo"]);
    let questions = section_id(preset, &["questions"]);
    let known = speaker_names(&transcript.utterances);
    let mut decision_items = Vec::new();
    let mut task_items: Vec<(Candidate, u8)> = Vec::new();
    let mut question_items = Vec::new();

    for (index, utterance) in transcript.utterances.iter().enumerate() {
        let source = utterance.text.trim();
        if source.is_empty() || crate::parse::contains_injection(source) {
            continue;
        }
        let normalized = text::normalize(source);
        let reference = SummaryRef {
            utterance_index: index,
            start: utterance.start,
        };
        if let Some(section) = questions {
            if cues::question_line(&normalized) {
                question_items.push(Candidate {
                    section_id: section,
                    item: plain_item(source, SectionRole::Plain, reference.clone()),
                });
                continue;
            }
        }
        if let Some(section) = decisions {
            if cues::decision_cue(&normalized) {
                decision_items.push(Candidate {
                    section_id: section,
                    item: plain_item(source, SectionRole::Decision, reference),
                });
                continue;
            }
        }
        if let Some(section) = tasks {
            if let Some(kind) = task_kind(source, &normalized, &known) {
                let task = task_meta(source, &utterance.speaker, &reference, kind, &known);
                let score = task_score(kind, &task);
                task_items.push((
                    Candidate {
                        section_id: section,
                        item: SummaryItem {
                            text: source.to_owned(),
                            role: SectionRole::Task,
                            task: Some(task),
                            refs: vec![reference],
                            quote: None,
                        },
                    },
                    score,
                ));
            }
        }
    }

    let task_items = dedupe_tasks(task_items);
    decision_items
        .into_iter()
        .chain(task_items)
        .chain(question_items)
        .collect()
}

pub(crate) fn speaker_names(utterances: &[Utterance]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for utterance in utterances {
        let name = utterance.speaker.trim();
        if name.is_empty() || is_placeholder_name(name) {
            continue;
        }
        let key = name_key(name);
        if seen.insert(key) {
            out.push(name.to_owned());
        }
    }
    out
}

fn name_key(name: &str) -> String {
    let normalized = text::normalize(name);
    let take = if normalized.chars().count() <= 4 {
        3
    } else {
        5
    };
    normalized.chars().take(take).collect()
}

fn section_id(preset: &Preset, ids: &[&str]) -> Option<&'static str> {
    ids.iter()
        .find_map(|id| preset.sections.iter().find(|s| s.id == *id))
        .map(|s| s.id)
}

fn plain_item(source: &str, role: SectionRole, reference: SummaryRef) -> SummaryItem {
    SummaryItem {
        text: source.to_owned(),
        role,
        task: None,
        refs: vec![reference],
        quote: None,
    }
}

/// Только реальное обязательство/потребность. Намеренно отсекает вступления,
/// мнения, предложения и общие «цели» без действия.
pub(crate) fn task_kind(source: &str, normalized: &str, known: &[String]) -> Option<TaskKind> {
    if cues::task_rejected(source) {
        return None;
    }
    if FIRST_PERSON
        .iter()
        .any(|marker| normalized.contains(marker))
        || named_commitment(source, known)
    {
        return Some(TaskKind::Explicit);
    }
    let need = ["нужно", "надо", "задача", "требуется"]
        .iter()
        .any(|marker| normalized.contains(marker));
    if need && has_infinitive(normalized) {
        return Some(TaskKind::Need);
    }
    None
}

fn has_infinitive(normalized: &str) -> bool {
    normalized.split_whitespace().any(|word| {
        (word.ends_with("ть") || word.ends_with("ться") || word.ends_with("ти"))
            && cues::action_verb(word)
    })
}

fn named_commitment(source: &str, known: &[String]) -> bool {
    if known.is_empty() {
        return false;
    }
    let words: Vec<&str> = source.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let Some(name) = known_name(word, known) else {
            continue;
        };
        let _ = name;
        let next_verb = words
            .iter()
            .skip(index + 1)
            .take(2)
            .any(|w| cues::action_verb(w));
        let prev_verb = index
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(|w| cues::action_verb(w))
            .unwrap_or(false);
        if next_verb || prev_verb {
            return true;
        }
    }
    false
}

fn known_name(token: &str, known: &[String]) -> Option<String> {
    let clean = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
    if clean.is_empty() {
        return None;
    }
    let key = name_key(clean);
    known.iter().find(|name| name_key(name) == key).cloned()
}

pub(crate) fn is_known_speaker(name: &str, known: &[String]) -> bool {
    known_name(name, known).is_some()
}

fn task_meta(
    source: &str,
    speaker: &str,
    reference: &SummaryRef,
    kind: TaskKind,
    known: &[String],
) -> TaskMeta {
    let found = deadline::find_in_text(source, reference.utterance_index, reference.start);
    let (deadline, deadline_ref, source_kind) = match &found {
        Some(hit) => (
            Some(hit.phrase.clone()),
            Some(SummaryRef {
                utterance_index: hit.utterance_index,
                start: hit.start,
            }),
            DeadlineSource::Extracted,
        ),
        None => (None, None, DeadlineSource::Model),
    };
    TaskMeta {
        assignee: assignee_for(source, Some(source), Some(speaker), known, kind),
        deadline,
        deadline_ref,
        deadline_source: source_kind,
    }
}

fn task_score(kind: TaskKind, task: &TaskMeta) -> u8 {
    let mut score = if kind == TaskKind::Explicit { 10 } else { 0 };
    if task.assignee.is_some() {
        score += 1;
    }
    if task.deadline.is_some() {
        score += 1;
    }
    score
}

fn dedupe_tasks(items: Vec<(Candidate, u8)>) -> Vec<Candidate> {
    let mut deduped: Vec<(Candidate, u8)> = Vec::new();
    for (candidate, score) in items {
        if let Some(position) = deduped
            .iter()
            .position(|(existing, _)| task_overlap(&existing.item, &candidate.item))
        {
            if score > deduped[position].1 {
                deduped[position] = (candidate, score);
            }
        } else {
            deduped.push((candidate, score));
        }
    }
    deduped
        .into_iter()
        .map(|(candidate, _)| candidate)
        .collect()
}

fn task_overlap(a: &SummaryItem, b: &SummaryItem) -> bool {
    let a = text::stem_set(&a.text);
    let b = text::stem_set(&b.text);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let intersection = a.intersection(&b).count();
    if intersection < 2 {
        return false;
    }
    let union = a.union(&b).count();
    let jaccard = intersection as f64 / union.max(1) as f64;
    let containment = intersection as f64 / a.len().min(b.len()).max(1) as f64;
    jaccard >= 0.3 || containment >= 0.5
}

fn is_placeholder_name(name: &str) -> bool {
    let normalized = text::normalize(name);
    normalized.starts_with("спикер")
        || normalized.starts_with("speaker")
        || normalized == "гость"
        || normalized == "участник"
}

#[cfg(test)]
mod tests;
