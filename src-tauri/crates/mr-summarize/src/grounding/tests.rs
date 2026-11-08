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
        Language::Ru,
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
    assert_eq!(task.assignee.as_deref(), Some("Борис"), "{task:?}");
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
fn model_output_is_not_diluted_with_fallback() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Анна", "Решили перенести запуск на следующую неделю."),
            u(1.0, "Борис", "Борис доделает отчёт до среды."),
            u(2.0, "Вера", "Вера проверит журнал завтра."),
            u(3.0, "Анна", "Анна обновит документацию к пятнице."),
            u(4.0, "Борис", "Я подготовлю план."),
            u(5.0, "Анна", "Что осталось по бюджету?"),
        ],
    };
    let output = "## Кратко\n- Обсудили запуск.\n## Задачи\n- Подготовить отчёт; Кто: Борис; Срок: до среды\n";
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Long, Language::Ru);

    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("tasks потеряны");
    assert_eq!(tasks.items.len(), 1, "{:?}", tasks.items);
    let task = tasks.items[0].task.as_ref().unwrap();
    assert_eq!(task.assignee.as_deref(), Some("Борис"));
    assert_eq!(task.deadline.as_deref(), Some("до среды"));
    assert!(
        summary.sections.iter().all(|s| s.id != "decisions"),
        "fallback подмешал решение: {:?}",
        summary.sections
    );
    assert!(
        summary.sections.iter().all(|s| s.id != "questions"),
        "fallback подмешал вопрос: {:?}",
        summary.sections
    );
    assert!(!summary.warnings.contains(&Warning::FallbackExtracted));
}

#[test]
fn fallback_runs_only_when_model_output_is_empty() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Анна", "Решили перенести запуск на следующую неделю."),
            u(1.0, "Борис", "Борис доделает отчёт до среды."),
            u(2.0, "Анна", "Что осталось по бюджету?"),
        ],
    };
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::empty(preset);
    let summary = ground(&parsed, &transcript, preset, Detail::Long, Language::Ru);
    assert!(summary.warnings.contains(&Warning::FallbackExtracted));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "decisions" && !s.items.is_empty()));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "tasks" && !s.items.is_empty()));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "questions" && !s.items.is_empty()));
}

#[test]
fn tasks_and_decisions_survive_brief_flood_at_normal() {
    let mut utterances = Vec::new();
    let mut brief_lines = String::from("## Кратко\n");
    let brief_words = [
        "запуска",
        "бюджета",
        "отчета",
        "документа",
        "журнала",
        "плана",
        "срока",
        "риска",
        "команды",
        "клиента",
        "сервера",
        "макета",
        "релиза",
    ];
    for (i, word) in brief_words.iter().enumerate() {
        let text = format!("Обсудили тему {word} и детали.");
        brief_lines.push_str(&format!("- {text}\n"));
        utterances.push(u(i as f64, "Секретарь", &text));
    }
    let decision = "Решили перенести запуск на следующую неделю.";
    let tasks = [
        "Борис доделает отчёт до среды.",
        "Вера проверит журнал завтра.",
        "Анна обновит документацию к пятнице.",
        "Я подготовлю план.",
    ];
    let question = "Что осталось по бюджету?";
    let mut output = brief_lines;
    output.push_str("## Решения\n");
    output.push_str(&format!("- {decision}\n"));
    output.push_str("## Задачи\n");
    for task in tasks {
        output.push_str(&format!("- {task}\n"));
    }
    output.push_str("## Открытые вопросы\n");
    output.push_str(&format!("- {question}\n"));
    let mut index = 20.0;
    for (speaker, text) in [
        ("Анна", decision),
        ("Борис", tasks[0]),
        ("Вера", tasks[1]),
        ("Анна", tasks[2]),
        ("Борис", tasks[3]),
        ("Анна", question),
    ] {
        utterances.push(u(index, speaker, text));
        index += 1.0;
    }
    let transcript = Transcript { utterances };
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(&output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::Ru);

    let tasks_section = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("tasks");
    assert_eq!(tasks_section.items.len(), 4, "{:?}", tasks_section.items);
    let decisions = summary
        .sections
        .iter()
        .find(|s| s.id == "decisions")
        .expect("decisions");
    assert_eq!(decisions.items.len(), 1);
    let questions = summary
        .sections
        .iter()
        .find(|s| s.id == "questions")
        .expect("questions");
    assert_eq!(questions.items.len(), 1);
    let brief = summary.sections.iter().find(|s| s.id == "brief").unwrap();
    assert!(brief.items.len() <= 4);
    assert_eq!(
        summary
            .sections
            .iter()
            .map(|s| s.items.len())
            .sum::<usize>(),
        10
    );
}

#[test]
fn brief_is_truncated_to_four_without_warning() {
    let mut output = String::from("## Кратко\n");
    let mut utterances = Vec::new();
    let brief_words = [
        "запуска",
        "бюджета",
        "отчета",
        "документа",
        "журнала",
        "плана",
        "срока",
        "риска",
        "команды",
        "клиента",
        "сервера",
        "макета",
        "релиза",
    ];
    for (i, word) in brief_words.iter().enumerate() {
        let text = format!("Обсудили тему {word} и детали.");
        output.push_str(&format!("- {text}\n"));
        utterances.push(u(i as f64, "Секретарь", &text));
    }
    let transcript = Transcript { utterances };
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(&output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::Ru);
    let brief = summary.sections.iter().find(|s| s.id == "brief").unwrap();
    assert_eq!(brief.items.len(), 4);
    assert!(!summary.warnings.contains(&Warning::OutputCapped));
}

#[test]
fn model_text_is_not_replaced_by_fallback() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Анна", "Коллеги, начинаем."),
            u(1.0, "Борис", "Я считаю, что можно встроить отчёт в план."),
            u(2.0, "Анна", "Решили перенести запуск на следующую неделю."),
            u(3.0, "Борис", "Борис обновит экран до вторника."),
            u(4.0, "Анна", "Что осталось по бюджету?"),
        ],
    };
    let output = "## Кратко\n- Обсудили запуск и сроки.\n";
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::Ru);
    assert!(summary.sections.iter().all(|s| s.id != "tasks"));
    assert!(summary.sections.iter().all(|s| s.id != "decisions"));
    assert!(summary.sections.iter().all(|s| s.id != "questions"));
    let brief = summary
        .sections
        .iter()
        .find(|s| s.id == "brief")
        .expect("brief");
    assert_eq!(brief.items.len(), 1, "{:?}", brief.items);
    assert!(!summary.warnings.contains(&Warning::FallbackExtracted));
}

#[test]
fn model_task_is_kept_without_fallback_replacement() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Анна", "Обсудили запуск и сроки."),
            u(1.0, "Вера", "Вере нужно позвонить клиенту."),
            u(2.0, "Анна", "Анна позвонит клиенту к пятнице."),
        ],
    };
    let output =
        "## Кратко\n- Обсудили запуск и сроки.\n## Задачи\n- Вере нужно позвонить клиенту.\n";
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::Ru);
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("tasks");
    assert_eq!(tasks.items.len(), 1, "{:?}", tasks.items);
    assert_eq!(tasks.items[0].text, "Вере нужно позвонить клиенту.");
    assert!(!summary.warnings.contains(&Warning::FallbackExtracted));
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
fn unsupported_items_are_kept_with_warning() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к пятнице.\n- Обсудили запуск.\n",
        vec![u(0.0, "Анна", "Обсудили запуск.")],
    );
    assert_eq!(summary.items_dropped, 0);
    assert!(summary.warnings.contains(&Warning::FactUnverified));
    let items: Vec<_> = summary.sections.iter().flat_map(|s| &s.items).collect();
    assert_eq!(items.len(), 2);
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
fn quote_markers_are_stripped_without_refs() {
    let summary = run(
        "meeting",
        "## Кратко\n- «Точная цитата»\n",
        vec![u(0.0, "Анна", "Точная цитата.")],
    );
    assert_eq!(summary.sections[0].items[0].text, "Точная цитата");
}

#[test]
fn closing_small_talk_moves_to_brief() {
    let summary = run(
        "meeting",
        "## Задачи\n- Всё тогда, всем хорошего дня! Да. Да. Борис, давай я тебе наберу.\n",
        vec![u(
            0.0,
            "Анна",
            "Всё тогда, всем хорошего дня! Да. Да. Борис, давай я тебе наберу.",
        )],
    );
    assert!(
        summary.sections.iter().all(|s| s.id != "tasks"),
        "{:?}",
        summary.sections
    );
    assert_eq!(summary.items_dropped, 0);
    let brief = summary
        .sections
        .iter()
        .find(|s| s.id == "brief")
        .expect("текст не должен потеряться");
    assert!(brief.items[0].text.contains("хорошего дня"));
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
fn psychology_judgement_is_kept_without_soft_claim_filter() {
    let summary = run(
        "psychology",
        "## Темы сессии\n- У клиента диагноз депрессия.\n",
        vec![u(0.0, "Клиент", "Мне тяжело, я плохо сплю.")],
    );
    assert_eq!(summary.soft_claims_dropped, 0);
    assert!(summary.sections.iter().any(|s| !s.items.is_empty()));
}

#[test]
fn agreement_is_kept_without_soft_claim_filter() {
    let summary = run(
        "scrum",
        "## Сделано\n- Команда согласны с планом запуска.\n",
        vec![u(0.0, "Анна", "Команда обсудила план запуска.")],
    );
    assert_eq!(summary.soft_claims_dropped, 0);
    assert!(summary.sections.iter().any(|s| !s.items.is_empty()));
}

#[test]
fn unsupported_number_is_kept_with_warning() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к 14.11.\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт.")],
    );
    assert_eq!(summary.items_dropped, 0);
    assert!(summary.warnings.contains(&Warning::FactUnverified));
    let brief = summary
        .sections
        .iter()
        .find(|s| s.id == "brief")
        .expect("brief");
    assert_eq!(brief.items[0].text, "Подготовить отчёт к 14.11.");
    assert!(
        summary.sections.iter().all(|s| s.id != "tasks"),
        "fallback не должен подмешивать задачу: {:?}",
        summary.sections
    );
}

#[test]
fn wrong_year_is_kept_with_warning() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к 2025.\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт в 2024 году.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary.warnings.contains(&Warning::FactUnverified));
}

#[test]
fn speaker_name_supports_hard_fact() {
    let summary = run(
        "meeting",
        "## Кратко\n- Анна подготовит отчёт.\n",
        vec![u(0.0, "Анна", "Подготовит отчёт.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "brief" && !s.items.is_empty()));
}

#[test]
fn inflected_name_matches_speaker() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт для Олега.\n",
        vec![u(0.0, "Олег", "Подготовить отчёт.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary.sections.iter().any(|s| s.id == "tasks"));
}

#[test]
fn sentence_start_infinitive_is_not_a_hard_fact() {
    let summary = run(
        "meeting",
        "## Кратко\n- Пройти проверку документов.\n",
        vec![u(0.0, "Анна", "Проверю документы.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "brief" && !s.items.is_empty()));
}

#[test]
fn brief_is_not_derived_from_tasks() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт; Кто: Анна; Срок: срок не назван\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт.")],
    );
    assert!(summary.sections.iter().all(|s| s.id != "brief"));
    assert!(summary.sections.iter().any(|s| s.id == "tasks"));
    assert!(!summary.warnings.contains(&Warning::BriefDerived));
}

#[test]
fn assignee_from_another_utterance_is_dropped() {
    let summary = run(
        "meeting",
        "## Задачи\n- Подготовить отчёт; Кто: Вера\n",
        vec![
            u(
                0.0,
                "Анна",
                "Анна обсуждала, как подготовить отчёт к среде.",
            ),
            u(1.0, "Анна", "Обсудили сроки."),
            u(2.0, "Анна", "Проверили план."),
            u(3.0, "Вера", "Вера проверит журнал."),
        ],
    );
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("task потерян");
    let task = tasks.items[0].task.as_ref().unwrap();
    assert_eq!(task.assignee, None, "чужой спикер назначен исполнителем");
    assert!(summary.warnings.contains(&Warning::AssigneeNotStated));
}

#[test]
fn invented_mid_sentence_name_is_flagged() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовит отчёт Анна.\n",
        vec![u(0.0, "Борис", "Подготовит отчёт.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary.warnings.contains(&Warning::FactUnverified));
    assert!(!summary.sections.is_empty());
}

#[test]
fn sentence_start_verb_is_not_a_hard_fact() {
    let summary = run(
        "meeting",
        "## Кратко\n- Решили перенести запуск.\n",
        vec![u(0.0, "Анна", "Договорились перенести запуск.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "brief" && !s.items.is_empty()));
}

#[test]
fn sentence_start_common_noun_is_not_a_hard_fact() {
    let summary = run(
        "meeting",
        "## Кратко\n- Команда закрыла проект.\n",
        vec![u(0.0, "Анна", "Мы закрыли проект.")],
    );
    assert_eq!(summary.items_dropped, 0, "{summary:?}");
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "brief" && !s.items.is_empty()));
}

#[test]
fn task_survives_reject_words_elsewhere_in_source_utterance() {
    let summary = run(
        "meeting",
        "## Задачи\n- Проверить логи до среды.\n",
        vec![u(
            0.0,
            "Анна",
            "Коллеги, я считаю, что тут есть риск, поэтому я до среды проверю логи.",
        )],
    );
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .unwrap_or_else(|| panic!("{summary:?}"));
    assert_eq!(tasks.items[0].text, "Проверить логи до среды.");
}

#[test]
fn discussion_is_not_a_task() {
    let summary = run(
        "meeting",
        "## Задачи\n- Обсудить бюджет.\n",
        vec![u(0.0, "Анна", "Задача: обсудить бюджет.")],
    );
    assert!(
        summary.sections.iter().all(|s| s.id != "tasks"),
        "{:?}",
        summary.sections
    );
}

#[test]
fn negated_need_is_not_a_task() {
    for support in ["Не нужно готовить отчёт.", "Не надо готовить отчёт."]
    {
        let summary = run(
            "meeting",
            "## Задачи\n- Подготовить отчёт.\n",
            vec![u(0.0, "Анна", support)],
        );
        assert!(
            summary.sections.iter().all(|s| s.id != "tasks"),
            "{support}: {:?}",
            summary.sections
        );
    }
}

#[test]
fn negated_status_and_agreement_are_kept_without_soft_claim_filter() {
    let status = run(
        "scrum",
        "## В работе\n- Запуск в работе.\n",
        vec![u(0.0, "Анна", "Запуск не в работе.")],
    );
    assert_eq!(status.soft_claims_dropped, 0);
    let agreement = run(
        "scrum",
        "## Сделано\n- Команда согласны с планом.\n",
        vec![u(0.0, "Анна", "Команда не согласны с планом.")],
    );
    assert_eq!(agreement.soft_claims_dropped, 0);
}

#[test]
fn english_sections_and_deadline_are_preserved() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Ann", "We decided to move the launch to next week."),
            u(1.0, "Bob", "Bob will update the report by Friday."),
        ],
    };
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(
        "## Decisions\n- We decided to move the launch to next week.\n## Tasks\n- Update the report; Who: Bob; Due: by Friday\n",
        preset,
    )
    .unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::En);
    let decisions = summary
        .sections
        .iter()
        .find(|s| s.id == "decisions")
        .expect("decisions");
    assert_eq!(decisions.title, "Decisions");
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("tasks");
    assert_eq!(tasks.title, "Tasks");
    let task = tasks.items[0].task.as_ref().unwrap();
    assert_eq!(task.assignee.as_deref(), Some("Bob"));
    assert_eq!(task.deadline.as_deref(), Some("by Friday"));
    assert!(summary.sections.iter().all(|s| s.title != "Задачи"));
}
