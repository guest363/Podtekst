use std::sync::OnceLock;

use regex::Regex;

use crate::parse::ParsedItem;
use crate::preset::Preset;
use crate::text::{self, Utterance};

/// Жёсткий факт проверяется по всему источнику, а не только по лучшим репликам:
/// сводка законно склеивает соседние реплики, и термин из соседней фразы —
/// всё ещё факт встречи, а не выдумка. Привязку к автору проверяют отдельно
/// `reconcile_task` и decision-cue.
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
        // «Пройти/Развернуть/Заполнить» в начале пункта — глагол, а не имя:
        // заглавная буква здесь от начала строки, не от имени собственного.
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
            r"(?i)\b(понедельник|вторник|среда|среду|среды|четверг|пятница|пятницу|пятницы|суббота|субботу|субботы|воскресенье|monday|tuesday|wednesday|thursday|friday|saturday|sunday)\b",
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
            // «Катя/Катю», «Олег/Олега», «Ольга/Ольги»: падеж меняет 4–5-й
            // символ, поэтому короткие слова сравниваем по общему началу.
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

pub(super) fn soft_claim_unsupported(
    item: &ParsedItem,
    support: &str,
    preset: &Preset,
    utterances: &[Utterance],
    indices: &[usize],
) -> bool {
    let text = text::normalize(&item.text);
    if contains_any(
        &text,
        &[
            "работает над",
            "в работе",
            "сделано",
            "готово",
            "закрыт",
            "взял",
            "начал",
            "завершил",
            "заблокирован",
        ],
    ) && !contains_any_unnegated(
        support,
        &[
            "сделал",
            "сделано",
            "готово",
            "закрыл",
            "закрыто",
            "в работе",
            "работаю над",
            "начал",
            "завершил",
            "взял",
            "взялся",
            "тестирую",
            "проверяю",
            "блокер",
            "заблокирован",
        ],
    ) {
        return true;
    }
    if contains_any(
        &text,
        &[
            "согласны",
            "согласен",
            "договорились",
            "поддерживают",
            "не возражают",
        ],
    ) && !contains_any_unnegated(
        support,
        &[
            "согласова",
            "согласны",
            "согласен",
            "договорились",
            "поддерж",
            "не возража",
            "утвердили",
            "приняли",
        ],
    ) {
        return true;
    }
    if contains_any(
        &text,
        &[
            "потому что",
            "из-за",
            "поэтому",
            "в результате",
            "привело к",
            "следовательно",
        ],
    ) && !contains_any(
        support,
        &[
            "потому что",
            "из-за",
            "поэтому",
            "в результате",
            "привело к",
            "следовательно",
        ],
    ) {
        return true;
    }
    if contains_any(&text, &["отвечает", "владелец", "owner", " делает"])
        && !has_explicit_name(utterances, indices)
    {
        return true;
    }
    if contains_any(
        &text,
        &[
            "суд решит",
            "положено",
            "обязан",
            "виновен",
            "невиновен",
            "имеет право",
        ],
    ) && !support.contains(&text)
    {
        return true;
    }
    if preset.id == "psychology"
        && contains_any(
            &text,
            &[
                "диагноз",
                "расстройств",
                "депресс",
                "личность",
                "характер",
                "советую",
                "вам нужно",
                "вы должны",
                "вы всегда",
                "вы никогда",
            ],
        )
    {
        return true;
    }
    false
}

fn has_explicit_name(utterances: &[Utterance], indices: &[usize]) -> bool {
    indices.iter().filter_map(|i| utterances.get(*i)).any(|u| {
        u.text.split_whitespace().enumerate().any(|(i, word)| {
            if i == 0 {
                return false;
            }
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
            let mut chars = clean.chars();
            chars.next().is_some_and(char::is_uppercase) && chars.any(char::is_lowercase)
        })
    })
}

pub(super) fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| text.contains(n))
}

/// «Не согласны» и «не в работе» содержат положительные cue, но отрицают
/// их; для soft-claim поддержкой считаем только неотрицательное вхождение.
fn contains_any_unnegated(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        text.match_indices(needle).any(|(index, _)| {
            let tail: Vec<&str> = text[..index].split_whitespace().rev().take(2).collect();
            !tail.iter().any(|word| {
                let word = word.trim_matches(|c: char| !c.is_alphanumeric());
                word == "не" || word == "not"
            })
        })
    })
}

pub(super) fn decision_cue(support: &str) -> bool {
    crate::cues::decision_cue(support)
}
