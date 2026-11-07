use crate::preset::{self, Preset};

pub(super) fn normalize_heading(raw: &str) -> String {
    let mut s = raw.trim();
    s = s.trim_end_matches([':', '*', '`']);
    s = s.trim_start_matches(['*', '`']);
    s.to_lowercase().replace('ё', "е").trim().to_owned()
}

pub(super) fn resolve_heading(preset: &Preset, raw: &str) -> Option<&'static str> {
    let key = normalize_heading(raw);
    for section in preset.sections {
        if key == normalize_heading(section.title_ru) || key == normalize_heading(section.title_en)
        {
            return Some(section.id);
        }
    }
    let candidates: &[&str] = match key.as_str() {
        "кратко" | "итог" => &["brief"],
        "главное" => &["main", "brief"],
        "решения" | "договоренности" | "решения и договоренности" => {
            &["decisions"]
        }
        "задачи" | "что сделать" | "следующие шаги" | "следующие действия" => {
            &["tasks", "next_steps"]
        }
        "открытые вопросы" | "вопросы" | "вопросы к разбору" => {
            &["questions"]
        }
        "сделано" | "готово" => &["done"],
        "в работе" | "сейчас" => &["in_progress"],
        "блокеры и риски" | "блокеры" | "риски" => &["blockers"],
        "суть" | "обсуждали" => &["substance"],
        "стороны и роли" | "стороны" => &["parties"],
        "факты и доводы" | "факты" => &["facts"],
        "нормы и ссылки" | "нормы" => &["norms"],
        "тема и тезис" | "тема" => &["topic"],
        "ключевые понятия" | "понятия" => &["concepts"],
        "структура" => &["structure"],
        "примеры" => &["examples"],
        "что запомнить" | "выводы" => &["takeaways"],
        "темы сессии" | "темы" => &["topics"],
        "слова клиента" => &["client_words"],
        "наблюдения и гипотезы" | "наблюдения" => &["observations"],
        "к следующей сессии" => &["next_session"],
        "открытые темы" => &["open_topics"],
        "о чем запись" => &["about"],
        "мысли и выводы" => &["thoughts"],
        "сделать" => &["todo"],
        "brief" => &["brief"],
        "decisions" => &["decisions"],
        "tasks" => &["tasks", "next_steps"],
        "open questions" | "questions" => &["questions"],
        "done" => &["done"],
        "in progress" => &["in_progress"],
        "blockers and risks" | "blockers" => &["blockers"],
        _ => &[],
    };
    candidates
        .iter()
        .find(|id| preset::section(preset, id).is_some())
        .copied()
}
