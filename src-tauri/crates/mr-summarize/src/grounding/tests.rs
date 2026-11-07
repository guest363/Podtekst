use super::*;
use crate::artifact::DeadlineSource;
use crate::parse;

fn u(start: f64, speaker: &str, text: &str) -> Utterance {
    Utterance {
        start,
        end: start + 1.0,
        speaker: speaker.into(),
        text: text.into(),
    }
}

fn run(preset: &str, output: &str, utterances: Vec<Utterance>) -> GroundedSummary {
    let transcript = Transcript { utterances };
    let parsed = parse::parse(output, crate::preset::get(preset).unwrap()).unwrap();
    ground(
        &parsed,
        &transcript,
        crate::preset::get(preset).unwrap(),
        Detail::Normal,
    )
}

#[test]
fn russian_morphology_supports_task_stems() {
    let summary = run(
        "meeting",
        "## Кратко\n- Обсудили работу.\n## Задачи\n- Обновить документацию.\n- Доделать отчёт.\n",
        vec![
            u(0.0, "Анна", "Обсудили работу."),
            u(1.0, "Борис", "Обновлю документацию."),
            u(2.0, "Борис", "Доделаю отчёт."),
        ],
    );
    let tasks: Vec<_> = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .map(|s| s.items.iter().map(|i| i.text.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(tasks, vec!["Обновить документацию.", "Доделать отчёт."]);
    assert!(!summary.warnings.contains(&Warning::ItemsDroppedUnsupported));
}

#[test]
fn task_assignee_from_speaker_and_text() {
    let summary = run(
        "meeting",
        "## Кратко\n- Обсудили работу.\n## Задачи\n- Борис доделает отчёт до среды.\n",
        vec![
            u(0.0, "Анна", "Обсудили работу."),
            u(1.0, "Борис", "Борис доделает отчёт до среды."),
        ],
    );
    let task = &summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap_or_else(|| panic!("{summary:#?}"))
        .items[0]
        .task
        .as_ref()
        .unwrap();
    assert_eq!(task.assignee.as_deref(), Some("Борис"));
    assert_eq!(task.deadline.as_deref(), Some("до среды"));
}

#[test]
fn task_assignee_from_inverted_name_and_verb() {
    let summary = run(
        "meeting",
        "## Кратко\n- Обсудили работу.\n## Задачи\n- Отчёт подготовит Борис до пятницы.\n",
        vec![
            u(0.0, "Анна", "Обсудили работу."),
            u(1.0, "Борис", "Отчёт подготовит Борис до пятницы."),
        ],
    );
    let task = &summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap()
        .items[0]
        .task
        .as_ref()
        .unwrap();
    assert_eq!(task.assignee.as_deref(), Some("Борис"));
}

#[test]
fn adjacent_unrelated_deadline_is_not_attached() {
    let summary = run(
        "meeting",
        "## Кратко\n- Обсудили работу.\n## Задачи\n- Подготовить отчёт; Кто: Борис; Срок: на пятницу\n",
        vec![
            u(0.0, "Борис", "Борис подготовит отчёт."),
            u(1.0, "Анна", "Встреча на пятницу перенесена."),
        ],
    );
    let task = &summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap()
        .items[0]
        .task
        .as_ref()
        .unwrap();
    assert!(task.deadline.is_none(), "чужой срок приклеен: {task:?}");
    assert_eq!(task.assignee.as_deref(), Some("Борис"));
    assert!(summary.warnings.contains(&Warning::DeadlineNotStated));
}

#[test]
fn decision_cue_detection_uses_full_source() {
    let with_cue = Transcript {
        utterances: vec![u(0.0, "Анна", "Решили перенести запуск.")],
    };
    let without = Transcript {
        utterances: vec![u(0.0, "Анна", "Обсудили запуск.")],
    };
    assert!(has_decision_cue(&with_cue));
    assert!(!has_decision_cue(&without));
}

#[test]
fn unsupported_items_are_dropped() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к пятнице.\n- Обсудили запуск.\n",
        vec![u(0.0, "Анна", "Обсудили запуск.")],
    );
    assert_eq!(summary.items_dropped, 1);
    assert!(summary.warnings.contains(&Warning::ItemsDroppedUnsupported));
    let items: Vec<_> = summary.sections.iter().flat_map(|s| &s.items).collect();
    assert_eq!(items.len(), 1);
}

#[test]
fn invented_deadline_is_removed() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт; Кто: Анна; Срок: до пятницы\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт.")],
    );
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap_or_else(|| panic!("{summary:?}"));
    let item = &tasks.items[0];
    assert!(item.task.as_ref().unwrap().deadline.is_none());
    assert!(summary.warnings.contains(&Warning::DeadlineNotStated));
}

#[test]
fn extracted_deadline_is_attached() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт; Кто: не назначен; Срок: срок не назван\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт до среды.")],
    );
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap_or_else(|| panic!("{summary:?}"));
    let task = tasks.items[0].task.as_ref().unwrap();
    assert_eq!(task.deadline.as_deref(), Some("до среды"));
    assert_eq!(task.deadline_source, DeadlineSource::Extracted);
    assert!(summary.warnings.contains(&Warning::DeadlineRecovered));
}

#[test]
fn unsupported_decision_moves_to_brief() {
    let summary = run(
        "meeting",
        "## Решения\n- Запуск переносится.\n",
        vec![u(0.0, "Анна", "Запуск переносится.")],
    );
    assert!(summary.sections.iter().all(|s| s.id != "decisions"));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "brief" && !s.items.is_empty()));
    assert!(summary.warnings.contains(&Warning::DecisionWithoutCue));
}

#[test]
fn quote_must_be_exact() {
    let summary = run(
        "meeting",
        "## Кратко\n- «Точная цитата»\n",
        vec![u(0.0, "Анна", "Точная цитата.")],
    );
    assert!(summary.sections[0].items[0].quote.is_some());
    let bad = run(
        "meeting",
        "## Кратко\n- «Точная цитата»\n",
        vec![u(0.0, "Анна", "Другие слова.")],
    );
    assert!(bad.sections.is_empty());
}

#[test]
fn legal_conclusion_requires_exact_source() {
    let summary = run(
        "legal",
        "## Суть\n- Суд решит дело в пользу истца.\n",
        vec![u(0.0, "Судья", "Суд решит дело в пользу истца.")],
    );
    let item = &summary.sections[0].items[0];
    assert_eq!(item.role, SectionRole::Plain);
    assert!(item.task.is_none());
    assert!(!item.text.contains("не назначен"));
}

#[test]
fn psychology_judgement_is_dropped() {
    let summary = run(
        "psychology",
        "## Темы сессии\n- У клиента диагноз депрессия.\n",
        vec![u(0.0, "Клиент", "Мне тяжело, я плохо сплю.")],
    );
    assert!(summary.items_dropped + summary.soft_claims_dropped > 0);
}

#[test]
fn agreement_without_cue_is_dropped_as_soft_claim() {
    let summary = run(
        "scrum",
        "## Сделано\n- Команда согласны с планом запуска.\n",
        vec![u(0.0, "Анна", "Команда обсудила план запуска.")],
    );
    assert!(summary.soft_claims_dropped >= 1);
    assert!(summary.warnings.contains(&Warning::SoftClaimDropped));
    assert!(summary.sections.is_empty());
}

#[test]
fn hard_fact_missing_from_source_drops_item() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к 14.11.\n",
        vec![u(0.0, "Анна", "Подготовить отчёт.")],
    );
    assert_eq!(summary.items_dropped, 1);
    assert!(summary.warnings.contains(&Warning::ItemsDroppedUnsupported));
    assert!(summary.sections.is_empty());
}

#[test]
fn brief_is_derived_when_empty() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт; Кто: Анна; Срок: срок не назван\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт.")],
    );
    let brief = summary.sections.iter().find(|s| s.id == "brief").unwrap();
    assert!(!brief.items.is_empty());
    assert!(brief.items.iter().all(|i| i.task.is_none()));
    assert!(summary.warnings.contains(&Warning::BriefDerived));
}
