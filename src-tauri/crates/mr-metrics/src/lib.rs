//! Измеримые метрики речи: взаимодействие, просодия и качество записи.
//! Эмоции не измеряются: только наблюдаемые свойства сигнала и тайминги.

mod interaction;
mod prosody;
mod quality;

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub channel: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grade {
    Good,
    Degraded,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Quality {
    pub grade: Grade,
    pub snr_db: Option<f64>,
    pub clipping_ratio: Option<f64>,
    pub bandwidth_hz: Option<f64>,
    pub channel_balance_db: Option<f64>,
}

const PROSODY_SNR_DB: f64 = 20.0;
const PROSODY_BAND_HZ: f64 = 6_400.0;

impl Quality {
    /// Шум и узкая полоса искажают F0 сильнее, чем перегрузка: клиппинг меняет громкость, но не период.
    pub fn permits_prosody(&self) -> bool {
        self.snr_db.is_some_and(|snr| snr >= PROSODY_SNR_DB)
            && self.bandwidth_hz.is_some_and(|hz| hz >= PROSODY_BAND_HZ)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Baseline {
    pub available: bool,
    pub n_meetings: usize,
    pub f0_delta_st: Option<f64>,
    pub energy_delta_db: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeakerMetrics {
    pub id: String,
    pub name: String,
    pub channel: Option<usize>,
    pub speech_sec: f64,
    pub share: f64,
    pub turns: usize,
    pub words: usize,
    pub vowels: usize,
    pub fillers: usize,
    pub fillers_per_100_words: Option<f64>,
    pub turn_sec_median: Option<f64>,
    pub turn_sec_p90: Option<f64>,
    pub monologue_max_sec: Option<f64>,
    pub monologues_over_30s: usize,
    pub interruptions_made: usize,
    pub interruptions_successful: usize,
    pub interruptions_suffered: usize,
    pub overlap_sec: f64,
    pub response_latency_ms_median: Option<f64>,
    pub response_latency_n: usize,
    pub wpm: Option<f64>,
    pub f0_median_hz: Option<f64>,
    pub f0_iqr_st: Option<f64>,
    pub f0_range_st: Option<f64>,
    pub energy_dbfs: Option<f64>,
    pub energy_iqr_db: Option<f64>,
    pub articulation_rate: Option<f64>,
    pub pause_share: Option<f64>,
    pub voiced_sec: Option<f64>,
    pub baseline: Option<Baseline>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Monologue {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Totals {
    pub turns: usize,
    pub turn_sec_median: Option<f64>,
    pub turn_sec_p90: Option<f64>,
    pub monologue_sec_median: Option<f64>,
    pub monologue_sec_p90: Option<f64>,
    pub monologues_over_30s: usize,
    pub interruptions: usize,
    pub interruptions_successful: usize,
    pub response_latency_ms_median: Option<f64>,
    pub switches_per_min: Option<f64>,
    pub overlap_sec: f64,
    pub overlap_share: Option<f64>,
    pub dead_air_n: usize,
    pub dead_air_sec: f64,
    pub dead_air_max_sec: Option<f64>,
    pub dead_air_share: Option<f64>,
    pub density: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Navigation {
    pub longest_monologue: Option<Monologue>,
    pub first_long_pause: Option<Span>,
    pub first_interruption: Option<Span>,
    pub pauses_over_5s: Vec<Span>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub schema: u32,
    pub duration_sec: f64,
    pub speech_sec: f64,
    pub quality: Quality,
    pub totals: Totals,
    pub speakers: Vec<SpeakerMetrics>,
    pub navigation: Navigation,
    pub warnings: Vec<String>,
}

pub fn analyze(
    turns: &[Turn],
    duration_sec: f64,
    audio: Option<&Path>,
    channels: usize,
    names: &HashMap<String, String>,
) -> Metrics {
    let interaction = interaction::compute(turns, duration_sec, names);
    let mut metrics = Metrics {
        schema: SCHEMA_VERSION,
        duration_sec: interaction.duration_sec,
        speech_sec: interaction.speech_sec,
        quality: Quality::default(),
        totals: interaction.totals,
        speakers: interaction.speakers,
        navigation: interaction.navigation,
        warnings: interaction.warnings,
    };
    if let Some(path) = audio {
        match quality::analyze(path, turns, channels) {
            Ok(q) => {
                metrics.warnings.extend(q.1);
                metrics.quality = q.0;
            }
            Err(error) => {
                log_warning(&mut metrics, "quality_unavailable");
                let _ = error;
            }
        }
        // L2 не считается на плохом звуке: иначе шум микрофона станет свойством говорящего [SPEC §5.1].
        if !metrics.quality.permits_prosody() {
            log_warning(&mut metrics, "prosody_unavailable");
            return metrics;
        }
        match prosody::analyze(path, turns, channels) {
            Ok(per_speaker) => {
                for speaker in &mut metrics.speakers {
                    let Some(p) = per_speaker.get(&speaker.id) else {
                        continue;
                    };
                    speaker.voiced_sec = Some(p.voiced_sec);
                    speaker.f0_median_hz = p.f0_median_hz;
                    speaker.f0_iqr_st = p.f0_iqr_st;
                    speaker.f0_range_st = p.f0_range_st;
                    speaker.energy_dbfs = p.energy_dbfs;
                    speaker.energy_iqr_db = p.energy_iqr_db;
                    speaker.pause_share = p.pause_share;
                    // Гласные считаются только по кириллице: для другого языка
                    // ноль — это «нет данных», а не нулевая артикуляция.
                    if p.voiced_sec > 0.0 && speaker.vowels > 0 {
                        speaker.articulation_rate = Some(speaker.vowels as f64 / p.voiced_sec);
                    }
                }
            }
            Err(_) => log_warning(&mut metrics, "prosody_unavailable"),
        }
    } else {
        log_warning(&mut metrics, "no_audio");
    }
    metrics
}

fn log_warning(metrics: &mut Metrics, code: &str) {
    if !metrics.warnings.iter().any(|w| w == code) {
        metrics.warnings.push(code.to_string());
    }
}

pub(crate) fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    } else {
        Some(sorted[mid])
    }
}

pub(crate) fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let idx = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    Some(sorted[idx.min(sorted.len() - 1)])
}

pub(crate) fn vowels(text: &str) -> usize {
    text.to_lowercase()
        .chars()
        .filter(|c| "аеёиоуыэюя".contains(*c))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vowels_counted_for_russian_text() {
        assert_eq!(vowels("Привет, как дела?"), 5);
    }

    #[test]
    fn percentile_handles_empty() {
        assert_eq!(percentile(&[], 0.9), None);
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 0.5), Some(2.0));
    }
}
