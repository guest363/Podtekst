use thiserror::Error;

#[derive(Debug, Error)]
pub enum SummaryError {
    #[error("too short: {chars} chars, {utterances} utterances")]
    TooShort { chars: usize, utterances: usize },
    #[error("no text")]
    NoText,
    #[error("unknown preset: {0}")]
    InvalidPreset(String),
    #[error("unknown detail: {0}")]
    InvalidDetail(String),
    #[error("unsupported summary schema: {0}")]
    SchemaUnsupported(String),
    #[error("invalid model output: {0}")]
    InvalidOutput(String),
    #[error("output capped")]
    OutputCapped,
    #[error("invalid transcript: {0}")]
    InvalidTranscript(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SummaryError>;
