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
        ("Борис", "Борис доделает отчёт до среды."),
        ("Вера", "Вере нужно позвонить клиенту."),
        ("Анна", "Анна позвонит клиенту к пятнице."),
        ("Вера", "Вера сверит бюджет завтра."),
        ("Спикер 1", "Надо заполнить журнал."),
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
                "Борис доделает отчёт до среды.",
                Some("Борис"),
                Some("до среды")
            ),
            (
                "Анна позвонит клиенту к пятнице.",
                Some("Анна"),
                Some("к пятнице")
            ),
            ("Вера сверит бюджет завтра.", Some("Вера"), Some("завтра")),
            ("Надо заполнить журнал.", None, None),
        ]
    );
    for task in &tasks {
        let text = task.item.text.to_lowercase();
        assert!(!text.contains("коллеги"), "{text}");
        assert!(!text.contains("я считаю"), "{text}");
        assert!(!text.contains("цель"), "{text}");
    }
    let decisions: Vec<_> = found
        .iter()
        .filter(|c| c.section_id == "decisions")
        .collect();
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].item.text.contains("Решили перенести"));
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
