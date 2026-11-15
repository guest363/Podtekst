use std::collections::HashMap;
use std::path::Path;

use mr_audio::decode::decode_wav_range_16k;

use super::{median, percentile, Turn};

const RATE: f32 = 8_000.0;
const FRAME: usize = 256;
const HOP: usize = 128;
const F0_MIN: f32 = 60.0;
const F0_MAX: f32 = 500.0;
const MIN_VOICED_SEC: f64 = 1.5;
const MIN_TURN_SEC: f64 = 0.3;
const MAX_TURN_SEC: f64 = 300.0;
const CLARITY_MIN: f32 = 0.3;

#[derive(Debug, Clone, Default)]
pub struct SpeakerProsody {
    pub f0_median_hz: Option<f64>,
    pub f0_iqr_st: Option<f64>,
    pub f0_range_st: Option<f64>,
    pub energy_dbfs: Option<f64>,
    pub energy_iqr_db: Option<f64>,
    pub voiced_sec: f64,
    pub pause_share: Option<f64>,
}

#[derive(Default)]
struct Acc {
    f0_hz: Vec<f32>,
    energy_db: Vec<f64>,
    voiced_sec: f64,
    pause_sec: f64,
    total_sec: f64,
}

pub fn analyze(
    path: &Path,
    turns: &[Turn],
    channels: usize,
) -> mr_audio::Result<HashMap<String, SpeakerProsody>> {
    let mut acc: HashMap<&str, Acc> = HashMap::new();
    for turn in turns {
        let duration = turn.end - turn.start;
        if duration < MIN_TURN_SEC {
            continue;
        }
        let end = (turn.start + duration.min(MAX_TURN_SEC)).min(turn.end);
        let channel = turn.channel.unwrap_or(0).min(channels.saturating_sub(1));
        let Ok(samples) = decode_wav_range_16k(path, turn.start, end, Some(channel)) else {
            continue;
        };
        analyze_turn(&samples, acc.entry(turn.speaker.as_str()).or_default());
    }

    let mut out = HashMap::new();
    for (speaker, acc) in acc {
        if acc.voiced_sec < MIN_VOICED_SEC {
            continue;
        }
        let semitones: Vec<f64> = acc
            .f0_hz
            .iter()
            .map(|hz| 12.0 * (*hz as f64 / 27.5).log2())
            .collect();
        let f0_median_hz = median(&acc.f0_hz.iter().map(|v| *v as f64).collect::<Vec<_>>());
        let f0_iqr_st = match (percentile(&semitones, 0.25), percentile(&semitones, 0.75)) {
            (Some(low), Some(high)) => Some(high - low),
            _ => None,
        };
        let f0_range_st = match (percentile(&semitones, 0.05), percentile(&semitones, 0.95)) {
            (Some(low), Some(high)) => Some(high - low),
            _ => None,
        };
        out.insert(
            speaker.to_string(),
            SpeakerProsody {
                f0_median_hz,
                f0_iqr_st,
                f0_range_st,
                energy_dbfs: median(&acc.energy_db),
                energy_iqr_db: match (
                    percentile(&acc.energy_db, 0.25),
                    percentile(&acc.energy_db, 0.75),
                ) {
                    (Some(low), Some(high)) => Some(high - low),
                    _ => None,
                },
                voiced_sec: acc.voiced_sec,
                pause_share: (acc.total_sec > 0.0).then(|| acc.pause_sec / acc.total_sec),
            },
        );
    }
    Ok(out)
}

fn analyze_turn(samples: &[f32], acc: &mut Acc) {
    let decimated: Vec<f32> = samples
        .chunks(2)
        .map(|pair| 0.5 * (pair[0] + pair.get(1).copied().unwrap_or(0.0)))
        .collect();
    if decimated.len() < FRAME {
        return;
    }

    let mut db: Vec<f64> = Vec::with_capacity(decimated.len() / HOP);
    let mut offset = 0;
    while offset + FRAME <= decimated.len() {
        db.push(db_of(&decimated[offset..offset + FRAME]));
        offset += HOP;
    }
    let Some(noise) = percentile(&db, 0.1) else {
        return;
    };
    let threshold = (noise + 10.0).clamp(-55.0, -30.0);
    // F0 считается только по чистым кадрам: тихие дают октавные ошибки и смещают медиану.
    let f0_threshold = threshold.max(-45.0);

    let frame_sec = HOP as f64 / RATE as f64;
    for (index, value) in db.iter().enumerate() {
        let offset = index * HOP;
        let frame = &decimated[offset..offset + FRAME];
        acc.total_sec += frame_sec;
        if *value <= threshold {
            acc.pause_sec += frame_sec;
            continue;
        }
        acc.voiced_sec += frame_sec;
        acc.energy_db.push(*value);
        if *value > f0_threshold {
            if let Some((hz, clarity)) = yin(frame) {
                if clarity >= CLARITY_MIN {
                    acc.f0_hz.push(hz);
                }
            }
        }
    }
}

fn db_of(samples: &[f32]) -> f64 {
    let mean = samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32;
    20.0 * (mean.sqrt().max(1e-9)).log10() as f64
}

fn yin(frame: &[f32]) -> Option<(f32, f32)> {
    let tau_min = (RATE / F0_MAX).floor() as usize;
    let tau_max = (RATE / F0_MIN).ceil() as usize;
    if frame.len() <= tau_max + 1 {
        return None;
    }
    let window = frame.len() - tau_max;
    let mut difference = vec![0.0_f32; tau_max + 1];
    for tau in 1..=tau_max {
        let mut sum = 0.0;
        for index in 0..window {
            let delta = frame[index] - frame[index + tau];
            sum += delta * delta;
        }
        difference[tau] = sum;
    }
    let mut normalized = vec![1.0_f32; tau_max + 1];
    let mut running = 0.0;
    for tau in 1..=tau_max {
        running += difference[tau];
        if running > 1e-12 {
            normalized[tau] = difference[tau] * tau as f32 / running;
        }
    }
    let best = (tau_min..tau_max)
        .find(|tau| normalized[*tau] < 0.1 && normalized[*tau] <= normalized[tau + 1])
        .or_else(|| (tau_min..=tau_max).min_by(|a, b| normalized[*a].total_cmp(&normalized[*b])))?;
    Some((
        RATE / refined_tau(&normalized, best),
        1.0 - normalized[best],
    ))
}

/// Минимум CMNDF между отсчётами: без параболы F0 квантуется шагом сетки (5 Гц на 200 Гц).
fn refined_tau(normalized: &[f32], tau: usize) -> f32 {
    if tau == 0 || tau + 1 >= normalized.len() {
        return tau as f32;
    }
    let (low, mid, high) = (normalized[tau - 1], normalized[tau], normalized[tau + 1]);
    let curvature = low - 2.0 * mid + high;
    if curvature.abs() < 1e-12 {
        return tau as f32;
    }
    tau as f32 + 0.5 * (low - high) / curvature
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, rate: f32, secs: f32) -> Vec<f32> {
        (0..(rate * secs) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate).sin() * 0.5)
            .collect()
    }

    #[test]
    fn yin_finds_tone() {
        let samples = tone(220.0, RATE, 0.1);
        let (hz, clarity) = yin(&samples[..FRAME]).expect("pitch");
        assert!((hz - 220.0).abs() < 1.0, "hz={hz}");
        assert!(clarity > 0.8, "clarity={clarity}");
    }

    #[test]
    fn pause_detected_in_silence() {
        let mut acc = Acc::default();
        analyze_turn(&vec![0.0; 16_000], &mut acc);
        assert_eq!(acc.voiced_sec, 0.0);
        assert!(acc.pause_sec > 0.5);
    }
}
