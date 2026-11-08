use crate::{cues, text};

const INFINITIVE_MAP: &[(&str, &str)] = &[
    ("доделаю", "доделать"),
    ("обновлю", "обновить"),
    ("проверю", "проверить"),
    ("беру", "взять"),
    ("сделаю", "сделать"),
    ("подготовлю", "подготовить"),
    ("отправлю", "отправить"),
    ("позвоню", "позвонить"),
    ("напишу", "написать"),
    ("добавлю", "добавить"),
    ("назначу", "назначить"),
    ("выдам", "выдать"),
    ("проведу", "провести"),
    ("заполню", "заполнить"),
    ("перенесу", "перенести"),
    ("уточню", "уточнить"),
    ("соберу", "собрать"),
    ("оформлю", "оформить"),
    ("загружу", "загрузить"),
    ("настрою", "настроить"),
    ("пришлю", "прислать"),
    ("согласую", "согласовать"),
];

const LEADING_DISCOURSE: &[&str] = &[
    "это моя задача",
    "а я",
    "тогда",
    "я",
    "а",
    "и",
    "ну",
    "так",
    "вот",
];

const TRAILING_DISCOURSE: &[&str] = &[
    "сделаю вместе с публикацией",
    "сделаем вместе с публикацией",
    "сделаю вместе",
    "сделаем вместе",
];

/// Убирает разговорные маркеры из fallback-задачи, не меняя факты.
pub(crate) fn clean_task_text(source: &str) -> String {
    let mut text = source.trim().to_owned();
    loop {
        let lower = text.to_lowercase();
        let mut stripped = false;
        for marker in LEADING_DISCOURSE {
            if let Some(rest) = strip_word_prefix(&text, &lower, marker) {
                text = rest;
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    let lower = text.to_lowercase();
    for suffix in TRAILING_DISCOURSE {
        if lower.ends_with(suffix) {
            let cut = text.len() - suffix.len();
            text = text[..cut]
                .trim_end_matches(|c: char| {
                    c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '—')
                })
                .to_owned();
            break;
        }
    }
    map_first_person(&text)
}

fn strip_word_prefix(text: &str, lower: &str, marker: &str) -> Option<String> {
    let rest = lower.strip_prefix(marker)?;
    if !rest.is_empty()
        && !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '—'))
    {
        return None;
    }
    let tail = text.get(marker.len()..)?;
    Some(
        tail.trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '—')
        })
        .to_owned(),
    )
}

fn map_first_person(text: &str) -> String {
    let mut text = text.to_owned();
    if let Some(split) = text.find(char::is_whitespace) {
        let (first, rest) = text.split_at(split);
        let lower = first.to_lowercase();
        if let Some((_, infinitive)) = INFINITIVE_MAP
            .iter()
            .find(|(form, _)| *form == lower.trim_end_matches(','))
        {
            let punctuation = first
                .chars()
                .rev()
                .take_while(|c| matches!(c, ',' | ';' | ':'))
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            text = format!("{}{}{}", capitalize(infinitive), punctuation, rest);
        }
    } else {
        let lower = text.to_lowercase();
        if let Some((_, infinitive)) = INFINITIVE_MAP
            .iter()
            .find(|(form, _)| *form == lower.trim_end_matches(','))
        {
            text = capitalize(infinitive);
        }
    }
    capitalize(&text)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(chars.as_str());
    out
}

/// Главный объект задачи: самый длинный не-глагольный токен. Нужен для
/// дедупликации «доработать подсказки» / «обновить подсказки».
pub(crate) fn task_object_key(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for token in text::tokens(text) {
        if cues::action_verb(&token) || token.chars().count() < 4 || is_time_token(&token) {
            continue;
        }
        let better = best
            .as_ref()
            .map(|current| token.chars().count() >= current.chars().count())
            .unwrap_or(true);
        if better {
            best = Some(token);
        }
    }
    best.map(|token| text::stem_token(&token))
}

fn is_time_token(token: &str) -> bool {
    [
        "сегодня",
        "завтра",
        "послезавтра",
        "неделя",
        "неделе",
        "недели",
        "недель",
        "месяц",
        "месяца",
        "месяцев",
        "квартал",
        "квартала",
        "год",
        "года",
        "понедельник",
        "вторник",
        "среда",
        "среду",
        "среды",
        "четверг",
        "четверга",
        "пятница",
        "пятницу",
        "пятницы",
        "пятнице",
        "суббота",
        "субботу",
        "субботы",
        "воскресенье",
    ]
    .iter()
    .any(|time| token.starts_with(time))
        || token.starts_with("январ")
        || token.starts_with("феврал")
        || token.starts_with("март")
        || token.starts_with("апрел")
        || token.starts_with("май")
        || token.starts_with("июн")
        || token.starts_with("июл")
        || token.starts_with("август")
        || token.starts_with("сентябр")
        || token.starts_with("октябр")
        || token.starts_with("ноябр")
        || token.starts_with("декабр")
}
