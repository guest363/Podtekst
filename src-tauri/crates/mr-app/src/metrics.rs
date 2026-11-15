//! Личная базовая линия метрик: только локально, файл 0600.
//! Сравнение с собой, не с командой и не с чужой популяцией.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use mr_metrics::{Baseline, Metrics};
use serde::{Deserialize, Serialize};

use crate::config::app_data_dir;
use crate::fsutil::write_atomic_private;

const MIN_MEETINGS: usize = 5;
const MIN_SPEECH_SEC: f64 = 120.0;
const KEEP_SAMPLES: usize = 20;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    people: BTreeMap<String, Person>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Person {
    #[serde(default)]
    f0: Vec<Sample>,
    #[serde(default)]
    energy: Vec<Sample>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sample {
    src: String,
    v: f64,
}

pub(crate) fn apply(metrics: &mut Metrics, source: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let path = app_data_dir().join("metrics-baseline.json");
    let Some(mut store) = load(&path) else {
        return;
    };
    let mut changed = false;

    for speaker in &mut metrics.speakers {
        let name = speaker.name.trim();
        if !is_person(name) || name == speaker.id || speaker.speech_sec < MIN_SPEECH_SEC {
            continue;
        }
        let person = store.people.entry(name.to_string()).or_default();
        let f0 = speaker
            .f0_median_hz
            .and_then(|value| update(&mut person.f0, source, value));
        let energy = speaker
            .energy_dbfs
            .and_then(|value| update(&mut person.energy, source, value));
        if speaker.f0_median_hz.is_some() || speaker.energy_dbfs.is_some() {
            changed = true;
        }
        let n_meetings = person.f0.len().max(person.energy.len()).saturating_sub(1);
        let semitones = |hz: f64| 12.0 * (hz / 27.5).log2();
        let f0_delta = match (speaker.f0_median_hz, f0) {
            (Some(current), Some(median)) => Some(semitones(current) - semitones(median)),
            _ => None,
        };
        let energy_delta = match (speaker.energy_dbfs, energy) {
            (Some(current), Some(median)) => Some(current - median),
            _ => None,
        };
        speaker.baseline = Some(Baseline {
            available: f0_delta.is_some() || energy_delta.is_some(),
            n_meetings,
            f0_delta_st: f0_delta,
            energy_delta_db: energy_delta,
        });
    }

    if changed {
        if let Ok(text) = serde_json::to_vec(&store) {
            if let Err(error) = write_atomic_private(&path, &text) {
                log::warn!(target: "metrics", "ev=baseline.save error={error}");
            }
        }
    }
}

fn is_person(name: &str) -> bool {
    const PLACEHOLDERS: [&str; 5] = ["Собеседник", "Участник", "Гость", "Спикер", "SPEAKER_"];
    !name.is_empty() && !PLACEHOLDERS.iter().any(|prefix| name.starts_with(prefix))
}

fn update(samples: &mut Vec<Sample>, source: &str, value: f64) -> Option<f64> {
    let previous: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.src != source)
        .map(|sample| sample.v)
        .collect();
    let baseline = (previous.len() >= MIN_MEETINGS)
        .then(|| median(&previous))
        .flatten();
    samples.retain(|sample| sample.src != source);
    samples.push(Sample {
        src: source.to_string(),
        v: value,
    });
    if samples.len() > KEEP_SAMPLES {
        samples.remove(0);
    }
    baseline
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

fn load(path: &Path) -> Option<Store> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(store) => Some(store),
            Err(error) => {
                log::warn!(target: "metrics", "ev=baseline.load error={error}");
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Store::default()),
        Err(error) => {
            log::warn!(target: "metrics", "ev=baseline.load error={error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_keeps_single_sample_per_source() {
        let mut samples = Vec::new();
        update(&mut samples, "a", 10.0);
        update(&mut samples, "a", 12.0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].v, 12.0);
    }

    #[test]
    fn placeholders_are_not_people() {
        for name in [
            "Собеседник 1",
            "Участник 2",
            "Гость",
            "Спикер 3",
            "SPEAKER_00",
            "",
        ] {
            assert!(!is_person(name), "{name}");
        }
        assert!(is_person("Иван"));
    }

    #[test]
    fn baseline_needs_five_previous_meetings() {
        let mut samples = Vec::new();
        for index in 0..5 {
            assert!(update(&mut samples, &format!("m{index}"), 100.0).is_none());
        }
        assert!(update(&mut samples, "m5", 100.0).is_some());
    }
}
