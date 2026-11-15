use std::path::Path;

use hound::WavReader;
use mr_audio::decode::decode_wav_range_16k;
use mr_audio::echo::fft::{fft, Cx};

use super::{percentile, Grade, Quality, Turn};

const FRAME: usize = 320;
const HOP: usize = 160;
const FFT_SIZE: usize = 512;
const SAMPLE_RATE: f64 = 16_000.0;
const CHUNK_SEC: f64 = 30.0;
const BANDWIDTH_FRAMES: usize = 40;

pub fn analyze(
    path: &Path,
    turns: &[Turn],
    channels: usize,
) -> mr_audio::Result<(Quality, Vec<String>)> {
    let reader = WavReader::open(path).map_err(wav_err)?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 {
        return Err(mr_audio::AudioError::Other("WAV не 16 кГц".to_string()));
    }
    let total_sec = reader.duration() as f64 / SAMPLE_RATE;
    let usable = channels.min(spec.channels as usize).max(1);
    drop(reader);

    let channels_known = turns.iter().any(|turn| turn.channel.is_some());
    let intervals: Vec<Vec<(f64, f64)>> = (0..usable)
        .map(|channel| {
            let selected = if channels_known { Some(channel) } else { None };
            speech_intervals(turns, selected)
        })
        .collect();
    let mut clipping = vec![0usize; usable];
    let mut sample_count = vec![0usize; usable];
    let mut speech_db: Vec<Vec<f64>> = vec![Vec::new(); usable];
    let mut silence_db: Vec<Vec<f64>> = vec![Vec::new(); usable];
    let mut spectra: Vec<Vec<f64>> = vec![vec![0.0_f64; FFT_SIZE / 2 + 1]; usable];
    let mut spectrum_frames = vec![0usize; usable];

    let mut start = 0.0;
    while start < total_sec {
        let end = (start + CHUNK_SEC).min(total_sec);
        for channel in 0..usable {
            let Ok(samples) = decode_wav_range_16k(path, start, end, Some(channel)) else {
                continue;
            };
            for (index, db) in frame_db(&samples).into_iter().enumerate() {
                let center = start + (index * HOP + FRAME / 2) as f64 / SAMPLE_RATE;
                if in_speech(&intervals[channel], center) {
                    speech_db[channel].push(db);
                } else {
                    silence_db[channel].push(db);
                }
            }
            clipping[channel] += samples.iter().filter(|x| x.abs() >= 0.999).count();
            sample_count[channel] += samples.len();
            let limit = BANDWIDTH_FRAMES.saturating_sub(spectrum_frames[channel]);
            spectrum_frames[channel] += collect_spectrum(
                &samples,
                start,
                &intervals[channel],
                &mut spectra[channel],
                limit,
            );
        }
        start = end;
    }

    let mut snr_values = Vec::new();
    for channel in 0..usable {
        if speech_db[channel].len() < 25 || silence_db[channel].len() < 5 {
            continue;
        }
        let Some(speech) = percentile(&speech_db[channel], 0.9) else {
            continue;
        };
        let Some(noise) = percentile(&silence_db[channel], 0.1) else {
            continue;
        };
        // Цифровая тишина не даёт опорного шума; предел измеримого -90 dBFS.
        let noise = noise.max(-90.0);
        snr_values.push((speech - noise).clamp(0.0, 60.0));
    }
    let snr_db = snr_values.into_iter().reduce(f64::min);
    let clipping_ratio = (0..usable)
        .filter(|channel| sample_count[*channel] > 0)
        .map(|channel| clipping[channel] as f64 / sample_count[channel] as f64)
        .reduce(f64::max);
    let bandwidth_hz = (0..usable)
        .filter(|channel| spectra[*channel].iter().any(|power| *power > 0.0))
        .map(|channel| band_edge(&spectra[channel]))
        .reduce(f64::min);
    let channel_balance_db = if usable >= 2 && speech_db[0].len() >= 25 && speech_db[1].len() >= 25
    {
        match (
            percentile(&speech_db[0], 0.5),
            percentile(&speech_db[1], 0.5),
        ) {
            (Some(first), Some(second)) => Some(first - second),
            _ => None,
        }
    } else {
        None
    };

    let mut warnings = Vec::new();
    if snr_db.is_some_and(|v| v < 15.0) {
        warnings.push("low_snr".to_string());
    }
    if clipping_ratio.is_some_and(|v| v > 0.001) {
        warnings.push("clipping".to_string());
    }
    if bandwidth_hz.is_some_and(|v| v < 6_400.0) {
        warnings.push("narrow_band".to_string());
    }
    if channel_balance_db.is_some_and(|v| v.abs() > 12.0) {
        warnings.push("channel_balance".to_string());
    }

    let mut quality = Quality {
        grade: Grade::Degraded,
        snr_db,
        clipping_ratio,
        bandwidth_hz,
        channel_balance_db,
    };
    quality.grade = if !quality.permits_prosody() {
        if quality.snr_db.is_some_and(|snr| snr < 10.0) {
            Grade::Unavailable
        } else {
            Grade::Degraded
        }
    } else if quality.clipping_ratio.is_none_or(|value| value <= 0.001) {
        Grade::Good
    } else {
        Grade::Degraded
    };

    Ok((quality, warnings))
}

fn wav_err(error: hound::Error) -> mr_audio::AudioError {
    mr_audio::AudioError::Other(format!("wav: {error}"))
}

fn frame_db(samples: &[f32]) -> Vec<f64> {
    let mut out = Vec::with_capacity(samples.len() / HOP);
    let mut index = 0;
    while index + FRAME <= samples.len() {
        out.push(db_of(&samples[index..index + FRAME]));
        index += HOP;
    }
    if out.is_empty() && !samples.is_empty() {
        out.push(db_of(samples));
    }
    out
}

fn db_of(samples: &[f32]) -> f64 {
    let mean = samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32;
    20.0 * (mean.sqrt().max(1e-9)).log10() as f64
}

fn speech_intervals(turns: &[Turn], channel: Option<usize>) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = turns
        .iter()
        .filter(|t| t.end > t.start)
        .filter(|t| channel.is_none_or(|expected| t.channel == Some(expected)))
        .map(|t| (t.start, t.end))
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (start, end) in sorted {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

fn in_speech(intervals: &[(f64, f64)], time: f64) -> bool {
    let index = intervals.partition_point(|(_, end)| *end < time);
    intervals
        .get(index)
        .is_some_and(|(start, end)| *start <= time && time <= *end)
}

fn collect_spectrum(
    samples: &[f32],
    chunk_start: f64,
    intervals: &[(f64, f64)],
    acc: &mut [f64],
    limit: usize,
) -> usize {
    let mut used = 0;
    let mut offset = 0;
    while offset + FFT_SIZE <= samples.len() && used < limit {
        let center = chunk_start + (offset + FFT_SIZE / 2) as f64 / SAMPLE_RATE;
        if in_speech(intervals, center) {
            let mut buffer: Vec<Cx> = (0..FFT_SIZE)
                .map(|i| {
                    let window = 0.5
                        - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos();
                    Cx::new(samples[offset + i] * window, 0.0)
                })
                .collect();
            fft(&mut buffer);
            for (bin, value) in buffer.iter().take(FFT_SIZE / 2 + 1).enumerate() {
                acc[bin] += value.norm2() as f64;
            }
            used += 1;
        }
        offset += HOP;
    }
    used
}

/// Верхняя граница полосы: последний бин не ниже -50 дБ от пика.
/// Доля энергии ниже 95 % не подходит: у чистой речи она уже на ~700 Гц.
fn band_edge(spectrum: &[f64]) -> f64 {
    let peak = spectrum.iter().copied().fold(0.0, f64::max);
    if peak <= 0.0 {
        return 0.0;
    }
    let threshold = peak * 1e-5;
    let last = spectrum
        .iter()
        .rposition(|power| *power >= threshold)
        .unwrap_or(0);
    last as f64 * SAMPLE_RATE / FFT_SIZE as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_edge_of_tone_is_low() {
        let mut spectrum = vec![0.0; FFT_SIZE / 2 + 1];
        spectrum[10] = 100.0;
        assert!(band_edge(&spectrum) < 1_000.0);
    }

    #[test]
    fn speech_interval_search() {
        let intervals = vec![(1.0, 2.0), (5.0, 6.0)];
        assert!(in_speech(&intervals, 1.5));
        assert!(!in_speech(&intervals, 3.0));
        assert!(in_speech(&intervals, 5.5));
    }

    #[test]
    fn grade_follows_prosody_gate() {
        let clean = Quality {
            snr_db: Some(30.0),
            bandwidth_hz: Some(7_000.0),
            ..Quality::default()
        };
        assert!(clean.permits_prosody());
        let clipped = Quality {
            clipping_ratio: Some(0.2),
            ..clean.clone()
        };
        assert!(clipped.permits_prosody(), "перегрузка не мешает F0");
        let noisy = Quality {
            snr_db: Some(12.0),
            ..clean.clone()
        };
        assert!(!noisy.permits_prosody());
        let narrow = Quality {
            bandwidth_hz: Some(3_400.0),
            ..clean
        };
        assert!(!narrow.permits_prosody());
        assert!(!Quality::default().permits_prosody());
    }

    #[test]
    fn frame_db_handles_short_slice() {
        let values = frame_db(&[0.5, -0.5, 0.5, -0.5]);
        assert_eq!(values.len(), 1);
    }
}
