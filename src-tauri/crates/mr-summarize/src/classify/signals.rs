use std::collections::BTreeMap;

use super::spec::{has_role_label, Spec};
use super::{Metrics, Signal};
use crate::text;

pub(super) fn signals(
    spec: &Spec,
    normalized: &str,
    tokens: &[&str],
    m: &Metrics,
    base_scores: &BTreeMap<String, u32>,
) -> Vec<Signal> {
    let mut out = Vec::new();
    let hit = |stem: &str| text::stem_hit_tokens(normalized, tokens, stem);
    let cue = |preset: &str, which: bool| {
        spec.cues.get(preset).is_some_and(|c| {
            let list = if which { &c.a } else { &c.b };
            list.iter().any(|x| normalized.contains(x))
        })
    };
    let mut semantic = false;
    if cue("meeting", false) {
        out.push(Signal::Tasks);
        semantic = true;
    }
    if hit("срок") || hit("дедлайн") || hit("deadline") {
        out.push(Signal::Deadlines);
        semantic = true;
    }
    if cue("meeting", true) {
        out.push(Signal::Decisions);
        semantic = true;
    }
    if m.n_speakers >= 2 {
        out.push(Signal::MultipleSpeakers);
    } else {
        out.push(Signal::SingleSpeaker);
    }
    if cue("scrum", true) {
        out.push(Signal::StatusRound);
        semantic = true;
    }
    if cue("scrum", false) {
        out.push(Signal::Blockers);
        semantic = true;
    }
    if [
        "спринт",
        "стендап",
        "бэклог",
        "sprint",
        "standup",
        "backlog",
    ]
    .iter()
    .any(|s| hit(s))
    {
        out.push(Signal::SprintTerms);
        semantic = true;
    }
    if base_scores.get("legal").copied().unwrap_or(0) > 0 {
        out.push(Signal::LegalTerms);
        semantic = true;
    }
    if has_role_label(m, "legal") {
        out.push(Signal::ProceduralRoles);
        semantic = true;
    }
    if base_scores.get("lecture").copied().unwrap_or(0) > 0 {
        out.push(Signal::LectureTerms);
        semantic = true;
    }
    if cue("lecture", true) {
        out.push(Signal::Definitions);
        semantic = true;
    }
    if base_scores.get("psychology").copied().unwrap_or(0) > 0 {
        out.push(Signal::TherapyTerms);
        semantic = true;
    }
    if base_scores.get("personal").copied().unwrap_or(0) > 0 {
        out.push(Signal::PersonalTopics);
        semantic = true;
    }
    if m.question_ratio >= 0.15 {
        out.push(Signal::QaPattern);
        semantic = true;
    }
    if m.monologue_ratio >= 0.7 {
        out.push(Signal::Monologue);
        semantic = true;
    }
    if m.short_turn_ratio >= 0.55 {
        out.push(Signal::ShortTurns);
        semantic = true;
    }
    if m.dur_sec > 0.0 && m.dur_sec <= 1800.0 {
        out.push(Signal::TimingShort);
    }
    if m.dur_sec >= 3600.0 {
        out.push(Signal::TimingLong);
    }
    if !semantic {
        out.push(Signal::NoSignal);
    }
    out
}
