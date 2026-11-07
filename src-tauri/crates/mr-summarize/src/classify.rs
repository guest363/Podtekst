use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use self::spec::{rule_matches, spec, Spec};
use crate::error::SummaryError;
use crate::preset::{self, SectionRole};
use crate::text::{self, Language, Style, Transcript};

mod signals;
mod spec;

pub const HEURISTIC_VERSION: u32 = 1;
pub const SPEC_VERSION: &str = "classify-spec.v1";
pub const SPEC_SHA256: &str = "27599505935b6332ced237e4e4c48d2590ecf427fcba1eb2b87341ca6026f343";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Tasks,
    Deadlines,
    Decisions,
    MultipleSpeakers,
    SingleSpeaker,
    StatusRound,
    Blockers,
    SprintTerms,
    LegalTerms,
    ProceduralRoles,
    LectureTerms,
    Definitions,
    TherapyTerms,
    PersonalTopics,
    QaPattern,
    Monologue,
    ShortTurns,
    TimingShort,
    TimingLong,
    NoSignal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub n_speakers: usize,
    pub n_utt: usize,
    pub total_chars: usize,
    pub monologue_ratio: f64,
    pub short_turn_ratio: f64,
    pub question_ratio: f64,
    pub chars_per_utt: f64,
    pub dur_sec: f64,
    pub speakers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceCheck {
    pub short: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Classification {
    pub preset_id: Option<String>,
    pub confidence: Option<Confidence>,
    pub alternatives: Vec<String>,
    pub signals: Vec<Signal>,
    pub ambiguous: bool,
    pub style: Style,
    pub language: Language,
    pub scores: BTreeMap<String, u32>,
    pub base_scores: BTreeMap<String, u32>,
    pub margin: u32,
    pub sensitive_suppressed: bool,
}

pub fn validate_source(transcript: &Transcript) -> Result<SourceCheck, SummaryError> {
    let spec = spec();
    let chars = transcript.total_chars();
    let utterances = transcript.non_empty_texts().count();
    if utterances == 0 || chars == 0 {
        return Err(SummaryError::NoText);
    }
    if chars < spec.source_validation.hard_min_chars
        || utterances < spec.source_validation.hard_min_utterances
    {
        return Err(SummaryError::TooShort { chars, utterances });
    }
    Ok(SourceCheck {
        short: chars <= spec.source_validation.short_max_chars,
    })
}

pub fn classify(transcript: &Transcript) -> Classification {
    let spec = spec();
    let metrics = metrics(transcript);
    let window = text::lexical_window(
        &transcript.full_text(),
        spec.lexical_window.max_chars,
        spec.lexical_window.head_chars,
        spec.lexical_window.middle_chars,
        spec.lexical_window.tail_chars,
    );
    let normalized = text::normalize(&window);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();

    let mut scores = BTreeMap::new();
    let mut base_scores = BTreeMap::new();
    for preset_id in &spec.presets {
        let base = base_score(spec, preset_id, &normalized, &tokens);
        let base = base.min(spec.scoring.base_cap);
        let structure = structure_score(spec, preset_id, &metrics);
        let cues = cue_score(spec, preset_id, &normalized, &metrics);
        base_scores.insert(preset_id.clone(), base);
        scores.insert(
            preset_id.clone(),
            (base + structure + cues).min(spec.scoring.max_score),
        );
    }

    let ranked = rank(spec, &scores);
    let (top_id, top_score) = &ranked[0];
    let (_second_id, second_score) = &ranked[1];
    let margin = top_score.saturating_sub(*second_score);
    let mut confidence_level = confidence(spec, *top_score, margin);
    let mut ambiguous =
        *top_score >= spec.scoring.ambiguous.top_gte && margin <= spec.scoring.ambiguous.margin_lte;

    let mut primary = (*top_score >= spec.scoring.low.score_gte).then(|| top_id.clone());
    if primary.is_some() && top_id == "scrum" {
        let meeting = scores.get("meeting").copied().unwrap_or(0);
        if meeting + spec.scoring.scrum_meeting_takeover.meeting_within >= *top_score
            && meeting >= spec.scoring.scrum_meeting_takeover.min_score
        {
            primary = Some("meeting".to_owned());
            ambiguous = true;
            let next = ranked
                .iter()
                .find(|(id, _)| id != "meeting")
                .map(|(_, score)| *score)
                .unwrap_or(0);
            let new_margin = meeting.saturating_sub(next);
            confidence_level = confidence(spec, meeting, new_margin);
        }
    }

    let mut sensitive_suppressed = false;
    if let Some(id) = primary.clone() {
        if let Some(p) = preset::get(&id) {
            let base = base_scores.get(&id).copied().unwrap_or(0);
            if p.sensitive
                && !(confidence_level == Some(Confidence::High)
                    && base >= spec.scoring.sensitive_primary.base_gte)
            {
                sensitive_suppressed = true;
                primary = ranked
                    .iter()
                    .find(|(candidate, score)| {
                        *score >= 3 && !preset::get(candidate).is_some_and(|p| p.sensitive)
                    })
                    .map(|(id, _)| id.clone());
            }
        }
    }

    if sensitive_suppressed {
        if let Some(id) = primary.as_ref() {
            let score = scores.get(id).copied().unwrap_or(0);
            let next = ranked
                .iter()
                .find(|(candidate, _)| candidate != id)
                .map(|(_, value)| *value)
                .unwrap_or(0);
            confidence_level = confidence(spec, score, score.saturating_sub(next));
        }
    }
    if primary.is_none() {
        confidence_level = None;
    }
    let primary_score = primary
        .as_ref()
        .and_then(|id| scores.get(id).copied())
        .unwrap_or(0);
    let alternatives = if primary.is_some() {
        alternatives(spec, &ranked, primary.as_deref(), primary_score)
    } else {
        Vec::new()
    };
    let signals = signals::signals(spec, &normalized, &tokens, &metrics, &base_scores);

    let language = text::language_with(
        &transcript.full_text(),
        spec.language.ru_cyrillic_ratio_gte,
        spec.language.en_latin_ratio_gte,
        spec.language.en_cyrillic_ratio_lt,
    );
    Classification {
        preset_id: primary,
        confidence: confidence_level,
        alternatives,
        signals,
        ambiguous,
        style: Style::from_speakers(metrics.n_speakers),
        language,
        scores,
        base_scores,
        margin,
        sensitive_suppressed,
    }
}

fn confidence(spec: &Spec, score: u32, margin: u32) -> Option<Confidence> {
    if score >= spec.scoring.high.score_gte && margin >= spec.scoring.high.margin_gte {
        Some(Confidence::High)
    } else if score >= spec.scoring.medium.score_gte && margin >= spec.scoring.medium.margin_gte {
        Some(Confidence::Medium)
    } else if score >= spec.scoring.low.score_gte {
        Some(Confidence::Low)
    } else {
        None
    }
}

fn alternatives(
    spec: &Spec,
    ranked: &[(String, u32)],
    primary: Option<&str>,
    primary_score: u32,
) -> Vec<String> {
    ranked
        .iter()
        .filter(|(id, _score)| Some(id.as_str()) != primary)
        .filter(|(id, score)| {
            *score >= spec.scoring.alternatives.min_score
                && primary_score.saturating_sub(*score) <= spec.scoring.alternatives.within_top
                && !preset::get(id).is_some_and(|p| p.sensitive)
        })
        .take(spec.scoring.alternatives.max)
        .map(|(id, _)| id.clone())
        .collect()
}

fn rank(spec: &Spec, scores: &BTreeMap<String, u32>) -> Vec<(String, u32)> {
    let mut ranked: Vec<(String, u32)> = scores.iter().map(|(k, v)| (k.clone(), *v)).collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let ia = spec
                .tie_break
                .iter()
                .position(|x| x == &a.0)
                .unwrap_or(usize::MAX);
            let ib = spec
                .tie_break
                .iter()
                .position(|x| x == &b.0)
                .unwrap_or(usize::MAX);
            ia.cmp(&ib)
        })
    });
    ranked
}

fn base_score(spec: &Spec, preset_id: &str, normalized: &str, tokens: &[&str]) -> u32 {
    spec.keyword_groups
        .get(preset_id)
        .map(|groups| {
            groups
                .iter()
                .filter(|(stem, _)| text::stem_hit_tokens(normalized, tokens, stem))
                .map(|(_, weight)| *weight)
                .sum()
        })
        .unwrap_or(0)
}

fn structure_score(spec: &Spec, preset_id: &str, metrics: &Metrics) -> u32 {
    spec.structure
        .get(preset_id)
        .map(|rules| rules.iter().filter(|r| rule_matches(r, metrics)).count() as u32)
        .unwrap_or(0)
}

fn cue_score(spec: &Spec, preset_id: &str, normalized: &str, metrics: &Metrics) -> u32 {
    let Some(cues) = spec.cues.get(preset_id) else {
        return 0;
    };
    let a = cues.a.iter().any(|cue| normalized.contains(cue));
    let b = cues.b.iter().any(|cue| normalized.contains(cue));
    let role = cues.role_labels.iter().any(|label| {
        metrics.speakers.iter().any(|speaker| {
            text::normalize(speaker)
                .split_whitespace()
                .any(|t| t.starts_with(label))
        })
    });
    (a as u32 + b as u32 + role as u32).min(2)
}

fn metrics(transcript: &Transcript) -> Metrics {
    let mut speakers = Vec::new();
    for u in &transcript.utterances {
        let speaker = u.speaker.trim();
        if !speaker.is_empty() && !speakers.iter().any(|s| s == speaker) {
            speakers.push(speaker.to_owned());
        }
    }
    let n_speakers = speakers.len().max(1);
    let texts: Vec<&str> = transcript.non_empty_texts().collect();
    let n_utt = texts.len();
    let total_chars: usize = texts.iter().map(|t| t.chars().count()).sum();
    let monologue_chars: usize = texts
        .iter()
        .filter(|t| t.chars().count() >= 400)
        .map(|t| t.chars().count())
        .sum();
    let short_turns = texts
        .iter()
        .filter(|t| {
            let len = t.chars().count();
            (1..=80).contains(&len)
        })
        .count();
    let questions = texts.iter().filter(|t| t.trim_end().ends_with('?')).count();
    let dur_sec = match (
        transcript.utterances.iter().map(|u| u.end).reduce(f64::max),
        transcript
            .utterances
            .iter()
            .map(|u| u.start)
            .reduce(f64::min),
    ) {
        (Some(end), Some(start)) => (end - start).max(0.0),
        _ => 0.0,
    };
    let ratio = |part: usize, all: usize| {
        if all == 0 {
            0.0
        } else {
            part as f64 / all as f64
        }
    };
    Metrics {
        n_speakers,
        n_utt,
        total_chars,
        monologue_ratio: ratio(monologue_chars, total_chars),
        short_turn_ratio: ratio(short_turns, n_utt),
        question_ratio: ratio(questions, n_utt),
        chars_per_utt: ratio(total_chars, n_utt),
        dur_sec,
        speakers,
    }
}

pub fn spec_sha256() -> String {
    spec::spec_sha256()
}

pub fn stopwords_sha256() -> String {
    spec::stopwords_sha256()
}

pub fn spec_metadata() -> (&'static str, u32) {
    (spec().spec_version.as_str(), spec().heuristic_version)
}

pub fn universal_id() -> &'static str {
    spec().universal_preset.as_str()
}

pub fn section_role(preset_id: &str, section_id: &str) -> SectionRole {
    preset::get(preset_id)
        .and_then(|p| preset::section(p, section_id))
        .map(|s| s.role)
        .unwrap_or(SectionRole::Plain)
}

#[cfg(test)]
mod tests;
