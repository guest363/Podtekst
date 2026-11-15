//! Личная базовая линия метрик: только локально, файл 0600.
//! Сравнение с собой, не с командой и не с чужой популяцией.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;

use mr_metrics::{Baseline, Metrics};
use serde::{Deserialize, Serialize};

use crate::config::app_data_dir;
use crate::fsutil::write_atomic_private;

const BASELINE_VERSION: u32 = 2;
const MIN_MEETINGS: usize = 5;
const MIN_SPEECH_SEC: f64 = 120.0;
const KEEP_SAMPLES: usize = 20;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
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

pub(crate) fn apply(
    metrics: &mut Metrics,
    source: &str,
    persons: &HashMap<String, String>,
    record: bool,
) {
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let path = app_data_dir().join("metrics-baseline.json");
    let Some(mut store) = load(&path) else {
        return;
    };
    let energy_valid = !metrics
        .quality
        .clipping_ratio
        .is_some_and(|value| value > 0.001);
    let mut changed = false;

    for speaker in &mut metrics.speakers {
        let name = speaker.name.trim();
        if !is_person(name) || speaker.speech_sec < MIN_SPEECH_SEC {
            continue;
        }
        let key = persons
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(name);
        let channel = speaker
            .channel
            .map(|channel| channel.to_string())
            .unwrap_or_else(|| "none".to_string());
        let person = store.people.entry(format!("{key}#{channel}")).or_default();
        let f0_previous = previous(&person.f0, source);
        let energy_previous = previous(&person.energy, source);
        let f0 = (f0_previous.len() >= MIN_MEETINGS)
            .then(|| median(&f0_previous))
            .flatten();
        let energy = (energy_previous.len() >= MIN_MEETINGS)
            .then(|| median(&energy_previous))
            .flatten();
        let n_meetings = f0_previous.len().max(energy_previous.len());

        if record {
            if let Some(value) = speaker.f0_median_hz {
                changed |= record_sample(&mut person.f0, source, value);
            }
            if energy_valid {
                if let Some(value) = speaker.energy_dbfs {
                    changed |= record_sample(&mut person.energy, source, value);
                }
            }
        }

        let semitones = |hz: f64| 12.0 * (hz / 27.5).log2();
        let f0_delta = match (speaker.f0_median_hz, f0) {
            (Some(current), Some(median)) => Some(semitones(current) - semitones(median)),
            _ => None,
        };
        let energy_delta = if energy_valid {
            match (speaker.energy_dbfs, energy) {
                (Some(current), Some(median)) => Some(current - median),
                _ => None,
            }
        } else {
            None
        };
        speaker.baseline = Some(Baseline {
            available: f0_delta.is_some() || energy_delta.is_some(),
            n_meetings,
            f0_delta_st: f0_delta,
            energy_delta_db: energy_delta,
        });
    }

    if changed {
        store.version = BASELINE_VERSION;
        if let Ok(text) = serde_json::to_vec(&store) {
            if let Err(error) = write_atomic_private(&path, &text) {
                log::warn!(target: "metrics", "ev=baseline.save error={error}");
            }
        }
    }
}

pub(crate) fn reset() -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    match std::fs::remove_file(app_data_dir().join("metrics-baseline.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_person(name: &str) -> bool {
    const PLACEHOLDERS: [&str; 5] = ["Собеседник", "Участник", "Гость", "Спикер", "SPEAKER_"];
    !name.is_empty() && !PLACEHOLDERS.iter().any(|prefix| name.starts_with(prefix))
}

fn previous(samples: &[Sample], source: &str) -> Vec<f64> {
    samples
        .iter()
        .filter(|sample| sample.src != source)
        .map(|sample| sample.v)
        .collect()
}

fn record_sample(samples: &mut Vec<Sample>, source: &str, value: f64) -> bool {
    if samples
        .iter()
        .any(|sample| sample.src == source && sample.v == value)
    {
        return false;
    }
    samples.retain(|sample| sample.src != source);
    samples.push(Sample {
        src: source.to_string(),
        v: value,
    });
    if samples.len() > KEEP_SAMPLES {
        samples.remove(0);
    }
    true
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
        Ok(bytes) => match serde_json::from_slice::<Store>(&bytes) {
            Ok(store) if store.version == BASELINE_VERSION => Some(store),
            Ok(_) => Some(Store {
                version: BASELINE_VERSION,
                people: BTreeMap::new(),
            }),
            Err(error) => {
                log::warn!(target: "metrics", "ev=baseline.load error={error}");
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Store {
            version: BASELINE_VERSION,
            people: BTreeMap::new(),
        }),
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
    fn record_keeps_single_sample_per_source() {
        let mut samples = Vec::new();
        record_sample(&mut samples, "a", 10.0);
        record_sample(&mut samples, "a", 12.0);
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
            record_sample(&mut samples, &format!("m{index}"), 100.0);
        }
        assert_eq!(previous(&samples, "m5").len(), 5);
        assert_eq!(median(&previous(&samples, "m5")), Some(100.0));
    }

    #[test]
    fn baseline_excludes_current_recording() {
        let mut samples = Vec::new();
        for index in 0..5 {
            record_sample(&mut samples, &format!("m{index}"), 100.0);
        }
        record_sample(&mut samples, "m5", 200.0);
        assert_eq!(median(&previous(&samples, "m5")), Some(100.0));
    }
}
