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
fn synthetic_meeting_tightening() {
    let t = transcript(&[
        ("Анна", "Коллеги, начинаем."),
        ("Борис", "Я считаю, что можно встроить отчёт в план."),
        ("Анна", "Цель встречи — согласовать запуск."),
        ("Анна", "Решили перенести запуск на следующую неделю."),
        (
            "Анна",
            "Тогда договорились. Итоги: запуск, подсказки, журнал.",
        ),
        ("Борис", "Борис обновит экран до вторника."),
        ("Вера", "Нужно доработать три подсказки."),
        ("Вера", "А я обновлю подсказки до четверга. Срок — четверг…"),
        (
            "Анна",
            "Проверку на телефоне беру на себя до пятницы. Сделаю вместе с публикацией.",
        ),
        (
            "Спикер 1",
            "После публикации нужно проверить журнал ошибок. Это задача без срока",
        ),
        (
            "Борис",
            "Убедиться, что если модель не скачана, пользователь видит кнопку — Кто: Борис",
        ),
        ("Анна", "Ещё вопрос: нужен ли новый сервер? Пока не решили."),
        ("Анна", "Обсудили запуск и сроки."),
    ]);
    let found = candidates(&t, preset::get("meeting").unwrap());
    let tasks: Vec<_> = found.iter().filter(|c| c.section_id == "tasks").collect();
    assert_eq!(
        tasks.len(),
        4,
        "{:?}",
        tasks.iter().map(|c| &c.item.text).collect::<Vec<_>>()
    );
    let meta: Vec<_> = tasks
        .iter()
        .map(|c| {
            let task = c.item.task.as_ref().unwrap();
            (
                c.item.text.as_str(),
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
    for task in &tasks {
        let text = task.item.text.to_lowercase();
        assert!(!text.contains("коллеги"), "{text}");
        assert!(!text.contains("я считаю"), "{text}");
        assert!(!text.contains("цель"), "{text}");
        assert!(!text.contains("убедиться"), "{text}");
        assert!(!text.contains("пользователь"), "{text}");
    }
    assert_eq!(
        tasks
            .iter()
            .filter(|c| c.item.text.to_lowercase().contains("подсказ"))
            .count(),
        1,
        "дубли подсказок: {:?}",
        tasks.iter().map(|c| &c.item.text).collect::<Vec<_>>()
    );
    let decisions: Vec<_> = found
        .iter()
        .filter(|c| c.section_id == "decisions")
        .collect();
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].item.text.contains("Решили перенести"));
    assert!(!decisions[0].item.text.to_lowercase().contains("итоги"));
    let questions: Vec<_> = found
        .iter()
        .filter(|c| c.section_id == "questions")
        .collect();
    assert_eq!(questions.len(), 1);
    assert!(questions[0].item.text.contains("Ещё вопрос"));
    assert!(!decisions.iter().any(|c| c.item.text.contains("вопрос")));
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

#[test]
fn fallback_task_text_strips_discourse_markers() {
    assert_eq!(
        clean_task_text("Тогда я доделаю отчёт.", None),
        "Доделать отчёт."
    );
    assert_eq!(
        clean_task_text("А я обновлю подсказки до четверга.", None),
        "Обновить подсказки до четверга."
    );
    assert_eq!(
        clean_task_text("Это моя задача: проверить отчёт.", None),
        "Проверить отчёт."
    );
    assert_eq!(
        clean_task_text("Я подготовлю план, сделаю вместе с публикацией", None),
        "Подготовить план"
    );
    assert_eq!(
        clean_task_text("Борис обновит экран до вторника.", None),
        "Борис обновит экран до вторника."
    );
    assert_eq!(
        clean_task_text(
            "Проверку на телефоне беру на себя до пятницы. Сделаю вместе с публикацией.",
            Some("до пятницы")
        ),
        "Проверить на телефоне до пятницы."
    );
    assert_eq!(
        clean_task_text(
            "Обновить подсказки до четверга. Срок — четверг…",
            Some("до четверга")
        ),
        "Обновить подсказки до четверга."
    );
    assert_eq!(
        clean_task_text(
            "После публикации нужно проверить журнал ошибок. Это задача без срока",
            None
        ),
        "После публикации нужно проверить журнал ошибок."
    );
}

#[test]
fn object_key_deduplicates_hints_tasks() {
    assert_eq!(
        task_object_key("Нужно доработать три подсказки."),
        task_object_key("Обновить подсказки до четверга.")
    );
    assert_ne!(
        task_object_key("Борис обновит экран до вторника."),
        task_object_key("Надо заполнить журнал.")
    );
}
