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
];

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
    "можем ли",
    "нужно ли",
    "будет ли",
];

fn has_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub fn decision_cue(text: &str) -> bool {
    let normalized = normalize(text);
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
    if trimmed.ends_with('?') {
        return true;
    }
    let normalized = normalize(trimmed);
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
}
