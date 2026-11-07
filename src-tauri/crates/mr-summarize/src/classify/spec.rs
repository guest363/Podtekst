use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Metrics;
use crate::text;

#[derive(Debug, Deserialize)]
pub(super) struct Spec {
    pub(super) spec_version: String,
    pub(super) heuristic_version: u32,
    pub(super) presets: Vec<String>,
    pub(super) tie_break: Vec<String>,
    pub(super) universal_preset: String,
    pub(super) lexical_window: Window,
    #[allow(dead_code)] // metadata only; сверка SHA и количества — в тесте
    pub(super) token: TokenCfg,
    pub(super) keyword_groups: BTreeMap<String, Vec<(String, u32)>>,
    pub(super) structure: BTreeMap<String, Vec<Rule>>,
    pub(super) cues: BTreeMap<String, Cues>,
    pub(super) scoring: Scoring,
    pub(super) language: LanguageCfg,
    pub(super) source_validation: SourceValidation,
}

#[derive(Debug, Deserialize)]
pub(super) struct Window {
    pub(super) max_chars: usize,
    pub(super) head_chars: usize,
    pub(super) middle_chars: usize,
    pub(super) tail_chars: usize,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // metadata only; сверка SHA и количества — в тесте
pub(super) struct TokenCfg {
    pub(super) stopwords_sha256: String,
    pub(super) stopwords_total: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct Cues {
    pub(super) a: Vec<String>,
    pub(super) b: Vec<String>,
    pub(super) role_labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Scoring {
    pub(super) base_cap: u32,
    pub(super) max_score: u32,
    pub(super) high: Gate,
    pub(super) medium: Gate,
    pub(super) low: LowGate,
    pub(super) ambiguous: Ambiguous,
    pub(super) scrum_meeting_takeover: Takeover,
    pub(super) sensitive_primary: Sensitive,
    pub(super) alternatives: Alternatives,
}

#[derive(Debug, Deserialize)]
pub(super) struct Gate {
    pub(super) score_gte: u32,
    pub(super) margin_gte: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct LowGate {
    pub(super) score_gte: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct Ambiguous {
    pub(super) top_gte: u32,
    pub(super) margin_lte: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct Takeover {
    pub(super) meeting_within: u32,
    pub(super) min_score: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct Sensitive {
    pub(super) base_gte: u32,
}

#[derive(Debug, Deserialize)]
pub(super) struct Alternatives {
    pub(super) min_score: u32,
    pub(super) within_top: u32,
    pub(super) max: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct LanguageCfg {
    pub(super) ru_cyrillic_ratio_gte: f64,
    pub(super) en_latin_ratio_gte: f64,
    pub(super) en_cyrillic_ratio_lt: f64,
}

#[derive(Debug, Deserialize)]
pub(super) struct SourceValidation {
    pub(super) hard_min_chars: usize,
    pub(super) hard_min_utterances: usize,
    pub(super) short_max_chars: usize,
}

pub(super) fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    SPEC.get_or_init(|| {
        serde_json::from_str(include_str!("../../classify-spec.v1.json"))
            .expect("classify-spec.v1.json is embedded and must parse")
    })
}

pub(super) fn spec_sha256() -> String {
    hex(&Sha256::digest(include_bytes!(
        "../../classify-spec.v1.json"
    )))
}

pub(super) fn stopwords_sha256() -> String {
    hex(&Sha256::digest(include_bytes!("../../stopwords-v1.txt")))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[derive(Debug, Deserialize)]
pub(super) struct Rule {
    #[serde(default)]
    n_speakers_gte: Option<usize>,
    #[serde(default)]
    n_speakers_lte: Option<usize>,
    #[serde(default)]
    n_speakers_eq: Option<usize>,
    #[serde(default)]
    short_turn_ratio_lt: Option<f64>,
    #[serde(default)]
    short_turn_ratio_gte: Option<f64>,
    #[serde(default)]
    chars_per_utt_gte: Option<f64>,
    #[serde(default)]
    monologue_ratio_gte: Option<f64>,
    #[serde(default)]
    dur_sec_gte: Option<f64>,
    #[serde(default)]
    dur_sec_lte: Option<f64>,
    #[serde(default)]
    question_ratio_gte: Option<f64>,
    #[serde(default)]
    role_label: Option<bool>,
    #[serde(default)]
    any: Option<Vec<Rule>>,
}

pub(super) fn rule_matches(rule: &Rule, m: &Metrics) -> bool {
    if let Some(v) = rule.n_speakers_gte {
        if m.n_speakers < v {
            return false;
        }
    }
    if let Some(v) = rule.n_speakers_lte {
        if m.n_speakers > v {
            return false;
        }
    }
    if let Some(v) = rule.n_speakers_eq {
        if m.n_speakers != v {
            return false;
        }
    }
    if let Some(v) = rule.short_turn_ratio_lt {
        if m.short_turn_ratio >= v {
            return false;
        }
    }
    if let Some(v) = rule.short_turn_ratio_gte {
        if m.short_turn_ratio < v {
            return false;
        }
    }
    if let Some(v) = rule.chars_per_utt_gte {
        if m.chars_per_utt < v {
            return false;
        }
    }
    if let Some(v) = rule.monologue_ratio_gte {
        if m.monologue_ratio < v {
            return false;
        }
    }
    if let Some(v) = rule.dur_sec_gte {
        if m.dur_sec < v {
            return false;
        }
    }
    if let Some(v) = rule.dur_sec_lte {
        if m.dur_sec > v {
            return false;
        }
    }
    if let Some(v) = rule.question_ratio_gte {
        if m.question_ratio < v {
            return false;
        }
    }
    if rule.role_label == Some(true) && !has_role_label(m, "legal") {
        return false;
    }
    if let Some(any) = &rule.any {
        if !any.iter().any(|r| rule_matches(r, m)) {
            return false;
        }
    }
    true
}

pub(super) fn has_role_label(m: &Metrics, preset_id: &str) -> bool {
    let Some(cues) = spec().cues.get(preset_id) else {
        return false;
    };
    cues.role_labels.iter().any(|label| {
        m.speakers.iter().any(|speaker| {
            text::normalize(speaker)
                .split_whitespace()
                .any(|t| t.starts_with(label))
        })
    })
}
