use crate::artifact::{DeadlineSource, SummaryItem, SummaryRef, TaskMeta};
use crate::cues;
use crate::deadline;
use crate::preset::{Preset, SectionRole};
use crate::text::{self, Transcript};

pub(crate) struct Candidate {
    pub section_id: &'static str,
    pub item: SummaryItem,
}

/// Детерминированный fallback: только явные source-реплики, без новых фактов.
/// Секции выбираются по каталогу; порядок кандидатов — по приоритету секций.
pub(crate) fn candidates(transcript: &Transcript, preset: &Preset) -> Vec<Candidate> {
    let decisions = section_id(preset, &["decisions"]);
    let tasks = section_id(preset, &["tasks", "next_steps", "todo"]);
    let questions = section_id(preset, &["questions"]);
    let mut decision_items = Vec::new();
    let mut task_items = Vec::new();
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
            if task_line(&normalized) {
                let task = task_meta(source, &utterance.speaker, &reference);
                task_items.push(Candidate {
                    section_id: section,
                    item: SummaryItem {
                        text: source.to_owned(),
                        role: SectionRole::Task,
                        task: Some(task),
                        refs: vec![reference],
                        quote: None,
                    },
                });
            }
        }
    }

    decision_items
        .into_iter()
        .chain(task_items)
        .chain(question_items)
        .collect()
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

/// Только действие/обязательство. «Нужно» без глагола задачей не считается.
fn task_line(normalized: &str) -> bool {
    cues::action_verb(normalized)
}

fn task_meta(source: &str, speaker: &str, reference: &SummaryRef) -> TaskMeta {
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
        assignee: assignee_for(source, Some(source), Some(speaker)),
        deadline,
        deadline_ref,
        deadline_source: source_kind,
    }
}

pub(crate) fn assignee_for(
    task_text: &str,
    support: Option<&str>,
    speaker: Option<&str>,
) -> Option<String> {
    let support = support.map(text::normalize).unwrap_or_default();
    let normalized_task = text::normalize(task_text);
    let first_person = support.starts_with("я ")
        || normalized_task.starts_with("я ")
        || [
            "я сделаю",
            "я подготовлю",
            "я проверю",
            "я обновлю",
            "я доделаю",
        ]
        .iter()
        .any(|needle| support.contains(needle))
        || support.contains("беру")
        || support.contains("отвечаю");
    let speaker = speaker.map(str::trim).filter(|s| !s.is_empty());
    if first_person {
        if let Some(speaker) = speaker.filter(|s| !is_placeholder_name(s)) {
            return Some(speaker.to_owned());
        }
    }
    if let Some(speaker) = speaker {
        let normalized = text::normalize(speaker);
        if !is_placeholder_name(speaker)
            && (support.contains(&normalized) || normalized_task.contains(&normalized))
        {
            return Some(speaker.to_owned());
        }
    }
    extract_assignee_name(task_text)
}

pub(crate) fn extract_assignee_name(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        let mut chars = clean.chars();
        let Some(first) = chars.next() else {
            continue;
        };
        if !first.is_uppercase() || !chars.any(char::is_lowercase) {
            continue;
        }
        let normalized = text::normalize(clean);
        if is_not_name(&normalized) {
            continue;
        }
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
            return Some(clean.to_owned());
        }
    }
    None
}

fn is_placeholder_name(name: &str) -> bool {
    let normalized = text::normalize(name);
    normalized.starts_with("спикер")
        || normalized.starts_with("speaker")
        || normalized == "гость"
        || normalized == "участник"
}

fn is_not_name(normalized: &str) -> bool {
    matches!(
        normalized,
        "срок"
            | "кто"
            | "что"
            | "задача"
            | "решение"
            | "отчет"
            | "встреча"
            | "план"
            | "документ"
            | "документация"
            | "проект"
            | "бюджет"
            | "вопрос"
            | "итог"
            | "текст"
            | "письмо"
            | "звонок"
            | "тест"
            | "релиз"
            | "презентация"
            | "макет"
            | "баг"
            | "тикет"
            | "сегодня"
            | "завтра"
            | "послезавтра"
            | "понедельник"
            | "вторник"
            | "среда"
            | "четверг"
            | "пятница"
            | "суббота"
            | "воскресенье"
    ) || normalized.starts_with("январ")
        || normalized.starts_with("феврал")
        || normalized.starts_with("март")
        || normalized.starts_with("апрел")
        || normalized.starts_with("май")
        || normalized.starts_with("июн")
        || normalized.starts_with("июл")
        || normalized.starts_with("август")
        || normalized.starts_with("сентябр")
        || normalized.starts_with("октябр")
        || normalized.starts_with("ноябр")
        || normalized.starts_with("декабр")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{preset, text::Utterance};

    fn transcript(texts: &[(&str, &str)]) -> Transcript {
        Transcript {
            utterances: texts
                .iter()
                .enumerate()
                .map(|(i, (speaker, text))| Utterance {
                    start: i as f64,
                    end: i as f64 + 1.0,
                    speaker: speaker.to_string(),
                    text: text.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn extracts_tasks_assignees_and_deadlines() {
        let t = transcript(&[
            ("Анна", "Решили перенести запуск на следующую неделю."),
            ("Борис", "Борис доделает отчёт до среды."),
            ("Вера", "Вера проверит журнал завтра."),
            ("Анна", "Анна обновит документацию к пятнице."),
            ("Борис", "Я подготовлю план."),
            ("Анна", "Что осталось по бюджету?"),
        ]);
        let found = candidates(&t, preset::get("meeting").unwrap());
        let tasks: Vec<_> = found
            .iter()
            .filter(|c| c.section_id == "tasks")
            .map(|c| (c.item.text.as_str(), c.item.task.as_ref().unwrap()))
            .collect();
        assert_eq!(tasks.len(), 4);
        assert_eq!(tasks[0].1.assignee.as_deref(), Some("Борис"));
        assert_eq!(tasks[0].1.deadline.as_deref(), Some("до среды"));
        assert_eq!(tasks[1].1.assignee.as_deref(), Some("Вера"));
        assert_eq!(tasks[1].1.deadline.as_deref(), Some("завтра"));
        assert_eq!(tasks[2].1.assignee.as_deref(), Some("Анна"));
        assert_eq!(tasks[2].1.deadline.as_deref(), Some("к пятнице"));
        assert_eq!(tasks[3].1.assignee.as_deref(), Some("Борис"));
        assert!(tasks[3].1.deadline.is_none());
        assert!(found.iter().any(|c| c.section_id == "decisions"));
        assert!(found.iter().any(|c| c.section_id == "questions"));
    }

    #[test]
    fn task_requires_action_verb_not_just_need() {
        let t = transcript(&[
            ("Анна", "Нужно больше времени на обсуждение."),
            ("Борис", "Надо подготовить отчёт."),
            ("Вера", "Обсудили задачи на неделю."),
        ]);
        let found = candidates(&t, preset::get("meeting").unwrap());
        let tasks: Vec<_> = found.iter().filter(|c| c.section_id == "tasks").collect();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].item.text.contains("подготовить"));
    }

    #[test]
    fn first_person_uses_speaker_unless_placeholder() {
        let t = transcript(&[
            ("Борис", "Я подготовлю план."),
            ("Спикер 1", "Я проверю отчёт."),
        ]);
        let found = candidates(&t, preset::get("meeting").unwrap());
        let tasks: Vec<_> = found.iter().filter(|c| c.section_id == "tasks").collect();
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks[0].item.task.as_ref().unwrap().assignee.as_deref(),
            Some("Борис")
        );
        assert!(tasks[1].item.task.as_ref().unwrap().assignee.is_none());
    }
}
