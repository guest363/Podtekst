use crate::text::{self, normalize};

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
    "возьму",
    "возьмет",
    "взять",
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
    "доработает",
    "доработаю",
    "доработать",
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
    "обсудить",
    "обсуждение",
    "предлагаю",
    "должн",
    "требован",
    "риск",
    "если пользователь",
    "нужно увидеть",
    "убедиться",
    "не нужно",
    "не надо",
    "не буду",
    "не будет",
    "не сможет",
    "не готов",
];

pub fn task_rejected(text: &str) -> bool {
    let normalized = normalize(text);
    TASK_REJECT.iter().any(|marker| normalized.contains(marker))
}

/// Отрицание в реплике-источнике отменяет задачу, даже если модель её
/// сформулировала; остальные стоп-слова источника — контекст, не отмена.
pub fn task_negated(text: &str) -> bool {
    let normalized = normalize(text);
    [
        "не нужно",
        "не надо",
        "не буду",
        "не будет",
        "не сможет",
        "не готов",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
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
    if decision_recap(text) {
        return false;
    }
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

/// «Итоги: …» и «Тогда договорились. …» — пересказ, а не новое решение.
pub fn decision_recap(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.starts_with("итоги:") || lower.contains("итоги:") {
        return true;
    }
    lower.starts_with("тогда договорились")
        && (lower.contains("итоги") || lower.contains(". ") || lower.contains(';'))
}

pub fn task_cue(text: &str) -> bool {
    let normalized = normalize(text);
    TASK_CUES
        .iter()
        .any(|marker| text::contains_phrase_normalized(&normalized, marker))
}

pub fn action_verb(text: &str) -> bool {
    let normalized = normalize(text);
    normalized
        .split_whitespace()
        .any(|token| ACTION_VERBS.contains(&token))
}

pub fn question_line(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.contains('?') {
        return true;
    }
    let normalized = normalize(trimmed);
    if normalized.starts_with("вопрос:") || normalized.starts_with("вопросы:") {
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
        assert!(!question_line("Если у меня будут вопросы, я их задам."));
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
    fn recap_decisions_are_not_decisions() {
        for text in [
            "Итоги: запуск, подсказки, журнал.",
            "Тогда договорились. Итоги: запуск и подсказки.",
            "Тогда договорились; журнал и подсказки.",
        ] {
            assert!(!decision_cue(text), "{text}");
            assert!(decision_recap(text));
        }
        assert!(decision_cue("Тогда договорились перенести запуск."));
    }

    #[test]
    fn questions_without_trailing_mark_are_questions() {
        assert!(question_line("Ещё вопрос: нужен ли новый сервер"));
        assert!(question_line("Нужен ли отдельный стенд"));
        assert!(question_line("Стоит ли переносить встречу"));
        assert!(question_line("Вопрос по бюджету"));
    }

    #[test]
    fn beru_does_not_match_naberu() {
        assert!(!task_cue("Я тебе наберу."));
        assert!(!task_cue("Я тебя наберу."));
        assert!(task_cue("Беру на себя проверку."));
        assert!(!action_verb("наберу"));
        assert!(action_verb("позвонить"));
    }

    #[test]
    fn requirement_and_risk_lines_are_rejected() {
        for text in [
            "Убедиться, что если модель не скачана, пользователь видит кнопку.",
            "Требование: кнопка должна быть доступна.",
            "Риск: пользователь не увидит кнопку.",
            "Нужно увидеть кнопку на экране.",
        ] {
            assert!(task_rejected(text), "{text}");
        }
        assert!(!task_rejected("Борис обновит экран до вторника."));
    }
}
