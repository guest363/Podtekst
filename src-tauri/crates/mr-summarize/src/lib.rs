pub mod artifact;
pub mod chunk;
pub mod classify;
pub mod cues;
pub mod deadline;
pub mod error;
mod extract;
pub mod grounding;
pub mod parse;
pub mod preset;
pub mod prompt;
pub mod text;

pub use artifact::{
    ArtifactClassification, DeadlineSource, Generator, PresetInfo, Provenance, SourceInfo,
    SummaryArtifact, SummaryBody, SummaryItem, SummaryRecord, SummaryRef, SummarySection, TaskMeta,
    Timings, Warning,
};
pub use classify::{classify, validate_source, Classification, Confidence, Metrics, Signal};
pub use error::{Result, SummaryError};
pub use preset::{Detail, Preset, SectionRole, SectionSpec};
pub use text::{Language, Style, Transcript, Utterance};
