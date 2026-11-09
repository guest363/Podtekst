use std::fs;
use std::path::Path;

use mr_summarize::artifact::Warning;
use mr_summarize::parse;
use mr_summarize::preset::{self, SectionRole};

fn parse_fixture(preset: &str, file: &str) -> parse::ParsedSummary {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/parser")
        .join(file);
    parse::parse(
        &fs::read_to_string(path).unwrap(),
        preset::get(preset).unwrap(),
    )
    .unwrap()
}

#[test]
fn meeting_fixture_recovers_lists_bold_and_tasks() {
    let parsed = parse_fixture("meeting", "meeting-hybrid.txt");
    assert_eq!(parsed.sections[0].items.len(), 1);
    assert_eq!(
        parsed.sections[1].items[0].text,
        "Перенести публикацию на пятницу."
    );
    let tasks = &parsed.sections[2].items;
    assert_eq!(tasks.len(), 2);
    let first = tasks[0].task.as_ref().unwrap();
    assert_eq!(first.assignee.as_deref(), Some("Анна"));
    assert_eq!(first.deadline.as_deref(), Some("до среды"));
    let second = tasks[1].task.as_ref().unwrap();
    assert!(second.assignee.is_none());
    assert!(second.deadline.is_none());
    assert_eq!(parsed.sections[3].items.len(), 1);
}

#[test]
fn legal_fixture_keeps_descriptive_items_plain() {
    let parsed = parse_fixture("legal", "legal-tolerant.txt");
    let substance = &parsed.sections[0];
    assert_eq!(substance.id, "substance");
    assert_eq!(substance.role, SectionRole::Plain);
    assert_eq!(substance.items.len(), 2);
    for item in &substance.items {
        assert!(item.task.is_none(), "no task metadata in Суть");
        assert!(!item.text.contains("не назначен"));
        assert!(!item.text.contains("срок не назван"));
    }
    let next = &parsed.sections[5].items[0];
    assert_eq!(
        next.task.as_ref().unwrap().deadline.as_deref(),
        Some("до следующего заседания")
    );
}

#[test]
fn lecture_fixture_recovers_h4_and_continuation() {
    let parsed = parse_fixture("lecture", "lecture-tolerant.txt");
    assert_eq!(parsed.sections[0].id, "topic");
    assert_eq!(parsed.sections[1].items.len(), 1);
    assert!(parsed.sections[1].items[0]
        .text
        .contains("продолжение с отступом"));
    assert!(parsed.sections.iter().all(|s| s.role == SectionRole::Plain));
}

#[test]
fn scrum_fixture_attaches_task_labels() {
    let parsed = parse_fixture("scrum", "scrum-labels.txt");
    let task = &parsed.sections[5].items[0];
    let meta = task.task.as_ref().unwrap();
    assert_eq!(task.text, "Выдать доступ.");
    assert_eq!(meta.assignee.as_deref(), Some("Павел"));
    assert_eq!(meta.deadline.as_deref(), Some("сегодня"));
    assert!(!parsed.warnings.contains(&Warning::InjectionSuspected));
}
