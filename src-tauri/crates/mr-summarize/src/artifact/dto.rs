use serde::{Deserialize, Serialize};

use super::{SummaryArtifact, SummarySection, Warning};
use crate::preset::Detail;
use crate::text::Language;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRecord {
    pub id: String,
    pub job_id: String,
    pub file: String,
    pub preset_id: String,
    pub preset_label: String,
    pub model_id: String,
    pub model_title: String,
    pub created_at: String,
    pub duration_ms: u64,
    pub source_rev: String,
    pub detail: Detail,
    pub language: Language,
    pub output_language: Language,
    pub sections: Vec<SummarySection>,
    pub markdown: String,
    pub truncated: bool,
    pub warnings: Vec<Warning>,
    pub schema: String,
    pub schema_minor: u32,
}

pub(super) fn project(artifact: &SummaryArtifact, file: &str) -> SummaryRecord {
    SummaryRecord {
        id: artifact.id.clone(),
        job_id: artifact.id.clone(),
        file: file.to_owned(),
        preset_id: artifact.preset.id.clone(),
        preset_label: artifact.preset.label.clone(),
        model_id: artifact.generator.model_id.clone(),
        model_title: artifact.generator.model_title.clone(),
        created_at: artifact.generated_at.clone(),
        duration_ms: artifact.generator.duration_ms,
        source_rev: artifact.source.rev.clone(),
        detail: artifact.detail,
        language: artifact.source.language,
        output_language: artifact.output_language,
        sections: artifact.summary.sections.clone(),
        markdown: artifact.to_markdown(),
        truncated: artifact.summary.truncated,
        warnings: artifact.summary.warnings.clone(),
        schema: artifact.schema.clone(),
        schema_minor: artifact.schema_minor,
    }
}
