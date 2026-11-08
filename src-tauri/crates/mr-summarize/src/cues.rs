use crate::text::normalize;

pub const DECISION_CUES: &[&str] = &[
    "решили",
    "решение",
    "договорились",
    "зафиксировали",
    "фиксируем",
    "утвердили",
    "согласовали",
    "приняли",
    "постановили",
    "decided",
    "decision",
    "agreed",
    "approved",
];

/// Формы обязательства: первое лицо, третье лицо, инфинитив и явные метки
/// задачи. Расширены под живую русскую морфологию 4B-модели.
pub const TASK_CUES: &[&str] = &[
    "сделаю",
    "сделает",
    "сделать",
    "сделаем",
    "подготовлю",
    "подготовит",
    "подготовить",
    "подготовим",
    "беру",
    "берет",
    "взял",
    "отвечаю",
    "ответит",
    "нужно",
    "надо",
    "проверю",
    "проверит",
    "проверить",
    "отправлю",
    "отправит",
    "отправить",
    "обновлю",
    "обновит",
    "обновить",
    "доделаю",
    "доделает",
    "доделать",
    "проведу",
    "проведет",
    "провести",
    "добавлю",
    "добавит",
    "добавить",
    "назначу",
    "назначит",
    "назначить",
    "выдам",
    "выдаст",
    "выдать",
    "позвоню",
    "позвонит",
    "позвонить",
    "напишу",
    "напишет",
    "написать",
    "согласую",
    "согласует",
    "согласовать",
    "задача",
    "это моя задача",
    "will update",
    "task",
    "deadline",
    "to do",
    "todo",
];

pub const ACTION_VERBS: &[&str] = &[
    "сделает",
    "сделаю",
    "сделать",
    "подготовит",
    "подготовлю",
    "подготовить",
    "проверит",
    "проверю",
    "проверить",
    "отправит",
    "отправлю",
    "отправить",
    "обновит",
    "обновлю",
    "обновить",
    "доделает",
    "доделаю",
    "доделать",
    "проведет",
    "проведу",
    "провести",
    "добавит",
    "добавлю",
    "добавить",
    "назначит",
    "назначу",
    "назначить",
    "выдаст",
    "выдам",
    "выдать",
    "позвонит",
    "позвоню",
    "позвонить",
    "напишет",
    "напишу",
    "написать",
    "согласует",
    "согласую",
    "согласовать",
    "заполнит",
    "заполню",
    "заполнить",
    "перенесет",
    "перенесу",
    "перенести",
    "уточнит",
    "уточню",
    "уточнить",
    "соберет",
    "соберу",
    "собрать",
    "оформит",
    "оформлю",
    "оформить",
    "загрузит",
    "загружу",
    "загрузить",
    "настроит",
    "настрою",
    "настроить",
    "протестирует",
    "протестирую",
    "протестировать",
    "пришлет",
    "пришлю",
    "прислать",
    "сверит",
    "сверю",
    "сверить",
    "update",
    "prepare",
    "check",
    "send",
    "write",
    "call",
    "add",
    "assign",
    "fix",
    "test",
    "finish",
    "make",
    "do",
];

pub const TASK_REJECT: &[&str] = &[
    "я считаю",
    "можно",
    "цель",
    "коллеги",
    "начинаем",
    "давайте обсудим",
    "обсудим",
    "предлагаю",
];

pub fn task_rejected(text: &str) -> bool {
    let normalized = normalize(text);
    TASK_REJECT.iter().any(|marker| normalized.contains(marker))
}

const QUESTION_STARTS: &[&str] = &[
    "кто",
    "что",
    "когда",
    "где",
    "почему",
    "зачем",
    "как",
    "какой",
    "какая",
    "какие",
    "сколько",
    "вопрос",
    "нужен ли",
    "стоит ли",
    "надо ли",
    "можем ли",
    "можно ли",
    "будет ли",
];

fn has_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub fn decision_cue(text: &str) -> bool {
    let normalized = normalize(text);
    if has_any(
        &normalized,
        &[
            "не решили",
            "не согласовали",
            "не утвердили",
            "не приняли",
            "пока не решили",
        ],
    ) {
        return false;
    }
    has_any(&normalized, DECISION_CUES)
}

pub fn task_cue(text: &str) -> bool {
    let normalized = normalize(text);
    has_any(&normalized, TASK_CUES)
}

pub fn action_verb(text: &str) -> bool {
    let normalized = normalize(text);
    has_any(&normalized, ACTION_VERBS)
}

pub fn question_line(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.contains('?') {
        return true;
    }
    let normalized = normalize(trimmed);
    if normalized
        .split_whitespace()
        .any(|word| word == "вопрос" || word == "вопросы")
    {
        return true;
    }
    if has_any(
        &normalized,
        &["нужен ли", "стоит ли", "надо ли", "можно ли"],
    ) {
        return true;
    }
    QUESTION_STARTS
        .iter()
        .any(|start| normalized == *start || normalized.starts_with(&format!("{start} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morphology_and_questions() {
        assert!(decision_cue("Решили перенести запуск."));
        assert!(task_cue("Борис доделает отчёт."));
        assert!(task_cue("Обновить документацию."));
        assert!(action_verb("доделает"));
        assert!(question_line("Когда запускаем?"));
        assert!(question_line("Какой срок?"));
        assert!(!question_line("Обсудили срок."));
    }

    #[test]
    fn negated_decisions_are_not_decisions() {
        for text in [
            "Пока не решили, когда запуск.",
            "Не согласовали бюджет.",
            "Не утвердили план.",
            "Не приняли финальную версию.",
        ] {
            assert!(!decision_cue(text), "{text}");
        }
    }

    #[test]
    fn questions_without_trailing_mark_are_questions() {
        assert!(question_line("Ещё вопрос: нужен ли новый сервер"));
        assert!(question_line("Нужен ли отдельный стенд"));
        assert!(question_line("Стоит ли переносить встречу"));
        assert!(question_line("Вопрос по бюджету"));
    }
}
