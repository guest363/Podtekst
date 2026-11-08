use std::sync::OnceLock;

use regex::Regex;

use crate::text::{self, Utterance};

/// Факт из соседней реплики — не выдумка: сводка законно склеивает контекст.
pub(super) fn hard_facts_supported(item: &str, utterances: &[Utterance]) -> bool {
    let support = utterances
        .iter()
        .map(|u| format!("{} {}", u.speaker, u.text))
        .collect::<Vec<_>>()
        .join(" ");
    hard_facts(item)
        .iter()
        .all(|fact| fact_supported(fact, &support))
}

fn hard_facts(text: &str) -> Vec<String> {
    let mut facts = Vec::new();
    for pattern in fact_patterns() {
        for m in pattern.find_iter(text) {
            facts.push(m.as_str().to_owned());
        }
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    for (index, word) in words.iter().enumerate() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        let mut chars = clean.chars();
        let Some(first) = chars.next() else {
            continue;
        };
        if !first.is_uppercase() || !chars.any(|c| c.is_lowercase()) {
            continue;
        }
        // Заглавная в начале пункта — начало строки, а не имя («Пройти»).
        if sentence_start(&words, index) && infinitive(clean) {
            continue;
        }
        facts.push(clean.to_owned());
    }
    facts
}

fn sentence_start(words: &[&str], index: usize) -> bool {
    index == 0
        || words[index - 1]
            .chars()
            .last()
            .is_some_and(|c| matches!(c, '.' | '!' | '?' | ':' | ';' | '—'))
}

fn infinitive(word: &str) -> bool {
    let word = word.to_lowercase();
    word.ends_with("ть") || word.ends_with("ться") || word.ends_with("ти") || word.ends_with("чь")
}

fn fact_patterns() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"\d+(?:[.,]\d+)?",
            r"(?i)\b(января|февраля|марта|апреля|мая|июня|июля|августа|сентября|октября|ноября|декабря|january|february|march|april|may|june|july|august|september|october|november|december)\b",
            r"(?i)\b(понедельник(?:а|у|ом)?|вторник(?:а|у|ом)?|сред(?:а|у|ы|е|ой)|четверг(?:а|у|ом)?|пятниц(?:а|у|ы|е|ей)|суббот(?:а|у|ы|е|ой)|воскресень(?:е|я|ю|ем)|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
            r"\b\d{1,2}[:.]\d{2}\b",
            r"(?i)(₽|руб\.?|рублей|доллар(?:ов|а)?|евро|\$|€)",
            r"%",
            r"(?i)(№|ст\.|статья|article|дело)\s*\d+",
            r"[A-Za-z][A-Za-z0-9_-]{2,}",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("fact regex"))
        .collect()
    })
}

fn fact_supported(fact: &str, support: &str) -> bool {
    let normalized_fact = text::normalize(fact);
    if normalized_fact.is_empty() {
        return true;
    }
    let normalized_support = text::normalize(support);
    if normalized_fact.contains(' ') {
        return format!(" {normalized_support} ").contains(&format!(" {normalized_fact} "));
    }
    let fact_stem = text::stem_token(&normalized_fact);
    let name_like = normalized_fact.chars().all(char::is_alphabetic);
    normalized_support.split_whitespace().any(|token| {
        token == normalized_fact
            || (normalized_fact.chars().count() >= 5 && token.starts_with(&normalized_fact))
            || (token.chars().count() >= 5 && normalized_fact.starts_with(token))
            || (normalized_fact.chars().count() >= 4 && fact_stem == text::stem_token(token))
            // Падеж меняет 4–5-й символ («Катя/Катю»), короткие имена — по общему началу.
            || (name_like
                && common_prefix(&normalized_fact, token)
                    >= if normalized_fact.chars().count().min(token.chars().count()) <= 4 {
                        3
                    } else {
                        4
                    })
    })
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(a, b)| a == b).count()
}

pub(super) fn decision_cue(support: &str) -> bool {
    crate::cues::decision_cue(support)
}
