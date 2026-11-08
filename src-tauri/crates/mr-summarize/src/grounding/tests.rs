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
fn structured_fallback_fills_missing_tasks_without_duplicates() {
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
    assert_eq!(tasks.items.len(), 4, "{:?}", tasks.items);
    let assignees: Vec<_> = tasks
        .items
        .iter()
        .map(|i| i.task.as_ref().unwrap().assignee.as_deref())
        .collect();
    assert_eq!(
        assignees,
        vec![Some("Борис"), Some("Вера"), Some("Анна"), Some("Борис")]
    );
    let deadlines: Vec<_> = tasks
        .items
        .iter()
        .map(|i| i.task.as_ref().unwrap().deadline.as_deref())
        .collect();
    assert_eq!(
        deadlines,
        vec![Some("до среды"), Some("завтра"), Some("к пятнице"), None]
    );
    let mut texts: Vec<&str> = tasks.items.iter().map(|i| i.text.as_str()).collect();
    texts.sort_unstable();
    texts.dedup();
    assert_eq!(texts.len(), tasks.items.len(), "дубли задач: {texts:?}");
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "decisions" && s.items[0].text == transcript.utterances[0].text));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "questions" && s.items[0].text == transcript.utterances[5].text));
    assert!(summary.warnings.contains(&Warning::FallbackExtracted));
    assert!(
        summary.sections.iter().all(|s| s.id != "brief"),
        "fallback-задачи выданы за «Кратко»"
    );
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
fn synthetic_meeting_fallback_has_no_false_tasks() {
    let transcript = Transcript {
        utterances: vec![
            u(0.0, "Анна", "Коллеги, начинаем."),
            u(1.0, "Борис", "Я считаю, что можно встроить отчёт в план."),
            u(2.0, "Анна", "Цель встречи — согласовать запуск."),
            u(3.0, "Анна", "Решили перенести запуск на следующую неделю."),
            u(
                4.0,
                "Анна",
                "Тогда договорились. Итоги: запуск, подсказки, журнал.",
            ),
            u(5.0, "Борис", "Борис обновит экран до вторника."),
            u(6.0, "Вера", "Нужно доработать три подсказки."),
            u(
                7.0,
                "Вера",
                "А я обновлю подсказки до четверга. Срок — четверг…",
            ),
            u(
                8.0,
                "Анна",
                "Проверку на телефоне беру на себя до пятницы. Сделаю вместе с публикацией.",
            ),
            u(
                9.0,
                "Спикер 1",
                "После публикации нужно проверить журнал ошибок. Это задача без срока",
            ),
            u(
                10.0,
                "Борис",
                "Убедиться, что если модель не скачана, пользователь видит кнопку — Кто: Борис",
            ),
            u(
                11.0,
                "Анна",
                "Ещё вопрос: нужен ли новый сервер? Пока не решили.",
            ),
            u(12.0, "Анна", "Обсудили запуск и сроки."),
        ],
    };
    let output = "## Кратко\n- Обсудили запуск и сроки.\n";
    let preset = crate::preset::get("meeting").unwrap();
    let parsed = parse::parse(output, preset).unwrap();
    let summary = ground(&parsed, &transcript, preset, Detail::Normal, Language::Ru);

    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("tasks");
    assert_eq!(tasks.items.len(), 4, "{:?}", tasks.items);
    let meta: Vec<_> = tasks
        .items
        .iter()
        .map(|i| {
            let task = i.task.as_ref().unwrap();
            (
                i.text.as_str(),
                task.assignee.as_deref(),
                task.deadline.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        meta,
        vec![
            (
                "Борис обновит экран до вторника.",
                Some("Борис"),
                Some("до вторника")
            ),
            (
                "Обновить подсказки до четверга.",
                Some("Вера"),
                Some("до четверга")
            ),
            (
                "Проверить на телефоне до пятницы.",
                Some("Анна"),
                Some("до пятницы")
            ),
            (
                "После публикации нужно проверить журнал ошибок.",
                None,
                None
            ),
        ]
    );
    assert_eq!(
        tasks
            .items
            .iter()
            .filter(|i| i.text.to_lowercase().contains("подсказ"))
            .count(),
        1,
        "дубли подсказок: {:?}",
        tasks.items.iter().map(|i| &i.text).collect::<Vec<_>>()
    );
    for task in &tasks.items {
        let text = task.text.to_lowercase();
        assert!(!text.contains("коллеги"), "{text}");
        assert!(!text.contains("я считаю"), "{text}");
        assert!(!text.contains("цель"), "{text}");
        assert!(!text.contains("убедиться"), "{text}");
        assert!(!text.contains("пользователь"), "{text}");
    }
    let decisions = summary
        .sections
        .iter()
        .find(|s| s.id == "decisions")
        .expect("decisions");
    assert_eq!(decisions.items.len(), 1);
    assert!(!decisions.items[0].text.contains("вопрос"));
    assert!(!decisions.items[0].text.to_lowercase().contains("итоги"));
    assert!(summary
        .sections
        .iter()
        .any(|s| s.id == "questions" && !s.items.is_empty()));
    let brief = summary
        .sections
        .iter()
        .find(|s| s.id == "brief")
        .expect("brief");
    assert_eq!(brief.items.len(), 1, "{:?}", brief.items);
    assert!(summary.warnings.contains(&Warning::FallbackExtracted));
    assert!(!summary.warnings.contains(&Warning::BriefDerived));
}

#[test]
fn explicit_phone_commitment_replaces_need_task() {
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
    assert_eq!(tasks.items[0].text, "Анна позвонит клиенту к пятнице.");
    let task = tasks.items[0].task.as_ref().unwrap();
    assert_eq!(task.assignee.as_deref(), Some("Анна"));
    assert_eq!(task.deadline.as_deref(), Some("к пятнице"));
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
        vec![u(0.0, "Анна", "Надо подготовить отчёт.")],
    );
    assert_eq!(summary.items_dropped, 1);
    assert!(summary.warnings.contains(&Warning::ItemsDroppedUnsupported));
    let tasks = summary
        .sections
        .iter()
        .find(|s| s.id == "tasks")
        .expect("fallback task из источника");
    assert_eq!(tasks.items[0].text, "Надо подготовить отчёт.");
    assert!(!tasks.items[0].text.contains("14.11"));
    assert!(
        summary.sections.iter().all(|s| s.id != "brief"),
        "fallback-задача выдана за «Кратко»"
    );
}

#[test]
fn wrong_year_is_not_supported_by_prefix() {
    let summary = run(
        "meeting",
        "## Кратко\n- Подготовить отчёт к 2025.\n",
        vec![u(0.0, "Анна", "Надо подготовить отчёт в 2024 году.")],
    );
    assert_eq!(summary.items_dropped, 1, "{summary:?}");
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
fn invented_first_word_name_is_dropped() {
    let summary = run(
        "meeting",
        "## Кратко\n- Анна подготовит отчёт.\n",
        vec![u(0.0, "Борис", "Подготовит отчёт.")],
    );
    assert_eq!(summary.items_dropped, 1, "{summary:?}");
    assert!(summary.warnings.contains(&Warning::ItemsDroppedUnsupported));
    assert!(summary.sections.is_empty());
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
fn negated_status_and_agreement_do_not_support_soft_claims() {
    let status = run(
        "scrum",
        "## В работе\n- Запуск в работе.\n",
        vec![u(0.0, "Анна", "Запуск не в работе.")],
    );
    assert!(status.soft_claims_dropped >= 1, "{status:?}");
    let agreement = run(
        "scrum",
        "## Сделано\n- Команда согласны с планом.\n",
        vec![u(0.0, "Анна", "Команда не согласны с планом.")],
    );
    assert!(agreement.soft_claims_dropped >= 1, "{agreement:?}");
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
