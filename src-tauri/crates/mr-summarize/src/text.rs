use std::collections::HashSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::error::SummaryError;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Utterance {
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub speaker: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub utterances: Vec<Utterance>,
}

impl Transcript {
    pub fn from_json(json: &str) -> Result<Self, SummaryError> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn non_empty_texts(&self) -> impl Iterator<Item = &str> {
        self.utterances
            .iter()
            .map(|u| u.text.trim())
            .filter(|t| !t.is_empty())
    }

    pub fn full_text(&self) -> String {
        self.non_empty_texts().collect::<Vec<_>>().join("\n")
    }

    pub fn total_chars(&self) -> usize {
        self.non_empty_texts().map(|t| t.chars().count()).sum()
    }

    pub fn lexical_window(&self) -> String {
        lexical_window(&self.full_text(), 24_000, 10_000, 8_000, 6_000)
    }
}

pub fn lexical_window(full: &str, max: usize, head: usize, middle: usize, tail: usize) -> String {
    let chars: Vec<char> = full.chars().collect();
    if chars.len() <= max {
        return full.to_owned();
    }
    let mid_start = chars.len() / 2 - middle / 2;
    let mid_end = mid_start + middle;
    let head_s: String = chars[..head.min(chars.len())].iter().collect();
    let middle_s: String = chars[mid_start..mid_end.min(chars.len())].iter().collect();
    let tail_s: String = chars[chars.len().saturating_sub(tail)..].iter().collect();
    [head_s, middle_s, tail_s].join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Ru,
    En,
    Other,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
            Self::Other => "other",
        }
    }
}

pub fn normalize(input: &str) -> String {
    let composed: String = input.nfc().collect();
    let lowered = composed.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for ch in lowered.chars() {
        let ch = if ch == 'ё' { 'е' } else { ch };
        let keep = if ch.is_ascii() {
            ch.is_ascii_alphanumeric()
        } else if ('\u{0400}'..='\u{052F}').contains(&ch) {
            true
        } else {
            ch.is_alphanumeric()
        };
        if keep {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            out.push(ch);
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    out
}

fn stopwords() -> &'static HashSet<&'static str> {
    static STOP: OnceLock<HashSet<&'static str>> = OnceLock::new();
    STOP.get_or_init(|| {
        include_str!("../stopwords-v1.txt")
            .lines()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .collect()
    })
}

pub fn tokens(input: &str) -> Vec<String> {
    normalize(input)
        .split_whitespace()
        .filter(|t| t.chars().count() >= 3 && !stopwords().contains(*t))
        .map(str::to_owned)
        .collect()
}

pub fn token_set(input: &str) -> HashSet<String> {
    tokens(input).into_iter().collect()
}

/// Стем-матч группы: токен начинается со стема, фраза — точная с пробелами.
pub fn stem_hit(normalized: &str, stem: &str) -> bool {
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    stem_hit_tokens(normalized, &tokens, stem)
}

pub fn stem_hit_tokens(normalized: &str, tokens: &[&str], stem: &str) -> bool {
    if stem.contains(' ') {
        phrase_in(normalized, stem)
    } else {
        tokens.iter().any(|token| token.starts_with(stem))
    }
}

fn phrase_in(haystack: &str, needle: &str) -> bool {
    for (start, _) in haystack.match_indices(needle) {
        let end = start + needle.len();
        let before = start == 0 || haystack.as_bytes()[start - 1] == b' ';
        let after = end == haystack.len() || haystack.as_bytes()[end] == b' ';
        if before && after {
            return true;
        }
    }
    false
}

pub fn language_of(text: &str) -> Language {
    language_with(text, 0.60, 0.80, 0.10)
}

pub fn language_with(text: &str, ru_cyrillic: f64, en_latin: f64, en_cyrillic: f64) -> Language {
    let mut cyrillic = 0usize;
    let mut latin = 0usize;
    let mut letters = 0usize;
    for ch in text.chars() {
        if ('\u{0400}'..='\u{052F}').contains(&ch) {
            letters += 1;
            cyrillic += 1;
        } else if ch.is_ascii() {
            if ch.is_ascii_alphabetic() {
                letters += 1;
                latin += 1;
            }
        } else if ch.is_alphabetic() {
            letters += 1;
        }
    }
    if letters == 0 {
        return Language::Other;
    }
    let cyr = cyrillic as f64 / letters as f64;
    let lat = latin as f64 / letters as f64;
    if cyr >= ru_cyrillic {
        Language::Ru
    } else if lat >= en_latin && cyr < en_cyrillic {
        Language::En
    } else {
        Language::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[serde(rename = "monologue")]
    Monologue,
    #[serde(rename = "dialogue")]
    Dialogue,
    #[serde(rename = "multi-party")]
    MultiParty,
}

impl Style {
    pub fn from_speakers(n_speakers: usize) -> Self {
        match n_speakers {
            0 | 1 => Self::Monologue,
            2 => Self::Dialogue,
            _ => Self::MultiParty,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monologue => "monologue",
            Self::Dialogue => "dialogue",
            Self::MultiParty => "multi-party",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_folds_case_yo_and_punctuation() {
        assert_eq!(normalize("  ЁЖИК, «тест»!  "), "ежик тест");
    }

    #[test]
    fn normalize_composes_decomposed_cyrillic_before_folding() {
        assert_eq!(normalize("И\u{0306}"), "й");
        assert_eq!(normalize("е\u{0308}"), "е");
    }

    #[test]
    fn tokens_drop_short_and_stopwords() {
        assert_eq!(tokens("Я и ты решили задачу"), vec!["решили", "задачу"]);
    }

    #[test]
    fn language_thresholds() {
        assert_eq!(language_of("Привет, коллеги"), Language::Ru);
        assert_eq!(language_of("Hello colleagues"), Language::En);
        assert_eq!(language_of("Hello коллеги"), Language::Other);
        assert_eq!(language_of("123 !"), Language::Other);
    }

    #[test]
    fn window_keeps_head_middle_tail() {
        let text: String = ('а'..='я').cycle().take(30_000).collect();
        let window = lexical_window(&text, 24_000, 10_000, 8_000, 6_000);
        assert_eq!(window.chars().count(), 24_002);
        assert!(window.contains('\n'));
    }

    #[test]
    fn transcript_ignores_extra_fields() {
        let t = Transcript::from_json(
            r#"{"meta":{"x":1},"utterances":[{"start":0,"end":1,"speaker":"S","text":"ok","words":[]}]}"#,
        )
        .unwrap();
        assert_eq!(t.utterances.len(), 1);
        assert_eq!(t.utterances[0].text, "ok");
    }
}
