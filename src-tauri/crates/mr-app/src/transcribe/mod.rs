mod artifacts;
mod asr_engine;
mod channel_mix;
mod clean_speech;
mod diag;
pub mod diarize;
pub mod document;
mod echo;
pub mod events;
pub mod memory;
pub mod report;
mod speaker_assignment;
mod word_dedupe;
mod word_speakers;

use std::path::PathBuf;

mod models;
mod pipeline;
mod profile_verification;

/// Первый класс одновременной речи PyAnnote: 4+ — оверлап, младше — чистые голоса из mr-ml.
pub(super) const OVERLAP_CLASS: u8 = 4;

pub(crate) use models::extract_utterance_embedding;
pub use models::invalidate_models_cache;
pub use pipeline::transcribe_file;

pub use events::{
    clear_queue, emit_model_event, enqueue_transcription, finish_model_events,
    finish_summary_activity, get_queue_status, has_pending_work, is_transcribe_running,
    remove_from_queue, rename_in_queue, spawn_transcription_worker, start_model_download,
    start_summary_activity, take_new_events, transcribing_file,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WordConf {
    pub w: String,
    pub conf: f64,
    pub needs_review: bool,
    #[serde(default)]
    pub start_sec: f64,
    #[serde(default)]
    pub end_sec: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Utterance {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub channel: Option<i32>,
    pub text: String,
    #[serde(default)]
    pub diarized: bool,
    #[serde(default)]
    pub silhouette: f64,
    #[serde(default)]
    pub words: Vec<WordConf>,
    #[serde(default)]
    pub ctc_used: bool,
}

#[derive(Debug, Clone)]
pub struct SpeakerInfo {
    pub id: String,
    pub name: String,
    pub channel: Option<i32>,
    pub matched: bool,
    pub matched_name: Option<String>,
    pub matched_pack_id: Option<String>,
    pub matched_pack_name: Option<String>,
    pub similarity: f64,
    pub silhouette: f64,
    pub candidates: Vec<(String, f64)>,
    pub embedding: Option<Vec<f64>>,
    pub samples: Vec<Sample>,
    pub utterances_count: usize,
    pub total_speech_sec: f64,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub start: f64,
    pub end: f64,
    pub dur: f64,
    pub text: String,
    pub full_text: String,
    pub channel: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct TranscribeOptions {
    pub speaker_left: String,
    pub speaker_right: String,
    pub engine: String,
    /// Язык Whisper в виде кода («ru», «en»…); «auto» — определение моделью.
    /// GigaAM игнорирует поле (он одноязычный, русский).
    pub language: String,
    pub min_speakers: usize,
    pub max_speakers: usize,
    pub hotwords: Vec<String>,
    pub ctc_enabled: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            speaker_left: "Я".to_string(),
            speaker_right: "Собеседник".to_string(),
            engine: "auto".to_string(),
            language: "auto".to_string(),
            min_speakers: 0,
            max_speakers: 0,
            hotwords: Vec::new(),
            ctc_enabled: true,
        }
    }
}

pub struct TranscribeReport {
    pub md_path: PathBuf,
    pub txt_path: PathBuf,
    pub json_path: PathBuf,
    pub utterances: usize,
    pub speakers: Vec<String>,
    pub words: usize,
    pub speech_sec: f64,
    pub engine: String,
    pub speaker_method: String,
    pub duration_sec: f64,
}

#[cfg(test)]
mod tests {
    use super::Utterance;

    #[test]
    fn old_transcript_without_words_parses() {
        // P9: старые .transcript.json без words/ctc_used читаются с дефолтами.
        let u: Utterance = serde_json::from_value(serde_json::json!({
            "start": 0.0, "end": 1.0, "speaker": "Спикер",
            "channel": null, "text": "раз два"
        }))
        .unwrap();
        assert!(u.words.is_empty());
        assert!(!u.ctc_used);
    }
}
