use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::classify::{Classification, Confidence, Signal};
use crate::error::SummaryError;
use crate::preset::{Detail, Preset, SectionRole};
use crate::text::{Language, Style, Transcript, Utterance};

mod dto;
mod markdown;
pub use dto::SummaryRecord;

pub const SCHEMA: &str = "podtekst.summary/v1";
pub const SCHEMA_MAJOR: u32 = 1;
pub const SCHEMA_MINOR: u32 = 0;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_MD_BYTES: usize = 2 * 1024 * 1024;

pub const DROP_ORDER: &[&str] = &[
    "questions",
    "examples",
    "structure",
    "observations",
    "open_topics",
    "parties",
    "norms",
    "facts",
    "topic",
    "about",
    "thoughts",
    "main",
];
pub const NEVER_DROP: &[&str] = &[
    "brief",
    "decisions",
    "tasks",
    "next_steps",
    "blockers",
    "done",
    "in_progress",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRef {
    pub utterance_index: usize,
    pub start: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeadlineSource {
    Model,
    Extracted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskMeta {
    pub assignee: Option<String>,
    pub deadline: Option<String>,
    pub deadline_ref: Option<SummaryRef>,
    pub deadline_source: DeadlineSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryItem {
    pub text: String,
    pub role: SectionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMeta>,
    #[serde(default)]
    pub refs: Vec<SummaryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummarySection {
    pub id: String,
    pub title: String,
    pub role: SectionRole,
    pub items: Vec<SummaryItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    ShortSource,
    SlowHardware,
    Truncated,
    CoverageLow,
    ItemsDroppedUnsupported,
    SoftClaimDropped,
    DeadlineNotStated,
    DeadlineRecovered,
    AssigneeNotStated,
    DecisionWithoutCue,
    DecisionMissed,
    OutputThin,
    ParserRecovered,
    OutputCapped,
    BriefDerived,
    StaleSourceRace,
    LanguageMixed,
    MdWriteFailed,
    InjectionSuspected,
    DetailLongShortSource,
    FallbackExtracted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryBody {
    pub sections: Vec<SummarySection>,
    pub warnings: Vec<Warning>,
    pub truncated: bool,
    pub coverage: f64,
    pub items_total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    pub schema: u32,
    pub rev: String,
    pub utterances: usize,
    pub chars: usize,
    pub language: Language,
    pub duration_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactClassification {
    pub preset_id: Option<String>,
    pub confidence: Option<Confidence>,
    pub alternatives: Vec<String>,
    pub signals: Vec<Signal>,
    pub ambiguous: bool,
    pub style: Style,
    pub language: Language,
    pub classified_at: String,
    pub heuristic_version: u32,
    pub spec_version: String,
    pub user_override: bool,
}

impl ArtifactClassification {
    pub fn from_classification(
        classification: &Classification,
        classified_at: String,
        user_override: bool,
    ) -> Self {
        Self {
            preset_id: classification.preset_id.clone(),
            confidence: classification.confidence,
            alternatives: classification.alternatives.clone(),
            signals: classification.signals.clone(),
            ambiguous: classification.ambiguous,
            style: classification.style,
            language: classification.language,
            classified_at,
            heuristic_version: crate::classify::HEURISTIC_VERSION,
            spec_version: crate::classify::SPEC_VERSION.to_owned(),
            user_override,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: String,
    pub version: u32,
    pub label: String,
    pub confirmed_sensitive: bool,
}

impl PresetInfo {
    pub fn from_preset(preset: &Preset, confirmed_sensitive: bool) -> Self {
        Self {
            id: preset.id.to_owned(),
            version: crate::preset::PRESET_VERSION,
            label: preset.label_ru.to_owned(),
            confirmed_sensitive,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    pub load: u64,
    pub prepare: u64,
    pub generate: u64,
    pub verify: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Generator {
    pub runtime: String,
    pub runtime_commit: String,
    pub backend: String,
    pub model_id: String,
    pub model_title: String,
    pub model_sha256: String,
    pub quant: String,
    pub context: u32,
    pub threads: u32,
    pub temperature: f64,
    pub top_p: f64,
    pub repeat_penalty: f64,
    pub max_tokens: u32,
    pub seed: u64,
    pub prompt_version: String,
    pub duration_ms: u64,
    pub timings_ms: Timings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub mode: String,
    pub chunks: usize,
    pub reduce_levels: usize,
    pub items_dropped: usize,
    pub soft_claims_dropped: usize,
    pub parser_retries: usize,
    pub parser_recovered: usize,
    pub retry_reason: Option<String>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            mode: "single_pass".into(),
            chunks: 1,
            reduce_levels: 0,
            items_dropped: 0,
            soft_claims_dropped: 0,
            parser_retries: 0,
            parser_recovered: 0,
            retry_reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryArtifact {
    pub schema: String,
    pub schema_minor: u32,
    pub id: String,
    pub generated_at: String,
    pub source: SourceInfo,
    pub classification: ArtifactClassification,
    pub preset: PresetInfo,
    pub detail: Detail,
    pub output_language: Language,
    pub generator: Generator,
    pub summary: SummaryBody,
    pub provenance: Provenance,
}

impl SummaryArtifact {
    pub fn to_json(&self) -> Result<String, SummaryError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn to_markdown(&self) -> String {
        markdown::render(self)
    }

    pub fn project(&self, file: &str) -> SummaryRecord {
        dto::project(self, file)
    }

    pub fn enforce_caps(&mut self) -> Result<(), SummaryError> {
        let mut size = serde_json::to_vec(self)?.len();
        for id in DROP_ORDER {
            if size <= MAX_JSON_BYTES {
                break;
            }
            self.summary.sections.retain(|s| s.id != *id);
            self.summary.items_total = self.summary.sections.iter().map(|s| s.items.len()).sum();
            size = serde_json::to_vec(self)?.len();
        }
        if size > MAX_JSON_BYTES || self.to_markdown().len() > MAX_MD_BYTES {
            return Err(SummaryError::OutputCapped);
        }
        Ok(())
    }
}

pub fn source_rev(utterances: &[Utterance]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"schema=1\n");
    for u in utterances {
        hash.update(u.start.to_bits().to_le_bytes());
        hash.update(u.end.to_bits().to_le_bytes());
        let speaker = u.speaker.as_bytes();
        hash.update((speaker.len() as u32).to_le_bytes());
        hash.update(speaker);
        let text = u.text.as_bytes();
        hash.update((text.len() as u32).to_le_bytes());
        hash.update(text);
    }
    let digest = hash.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn source_info(transcript: &Transcript, rev: String) -> SourceInfo {
    let duration = match (
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
    SourceInfo {
        schema: 1,
        rev,
        utterances: transcript.non_empty_texts().count(),
        chars: transcript.total_chars(),
        language: crate::text::language_of(&transcript.full_text()),
        duration_sec: duration,
    }
}

pub fn parse_artifact(json: &str) -> Result<SummaryArtifact, SummaryError> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !matches!(schema, SCHEMA) {
        return Err(SummaryError::SchemaUnsupported(schema.to_owned()));
    }
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests;
