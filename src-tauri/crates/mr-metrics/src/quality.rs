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

    let intervals = speech_intervals(turns);
    let mut clipping = 0usize;
    let mut sample_count = 0usize;
    let mut speech_db: Vec<f64> = Vec::new();
    let mut silence_db: Vec<f64> = Vec::new();
    let mut channel_db: Vec<Vec<f64>> = vec![Vec::new(); usable];
    let mut spectrum = vec![0.0_f64; FFT_SIZE / 2 + 1];
    let mut spectrum_frames = 0usize;

    let mut start = 0.0;
    while start < total_sec {
        let end = (start + CHUNK_SEC).min(total_sec);
        for (channel, bucket) in channel_db.iter_mut().enumerate() {
            let Ok(samples) = decode_wav_range_16k(path, start, end, Some(channel)) else {
                continue;
            };
            for (index, db) in frame_db(&samples).into_iter().enumerate() {
                let center = start + (index * HOP + FRAME / 2) as f64 / SAMPLE_RATE;
                if in_speech(&intervals, center) {
                    speech_db.push(db);
                    bucket.push(db);
                } else {
                    silence_db.push(db);
                }
            }
            if channel == 0 {
                clipping += samples.iter().filter(|x| x.abs() >= 0.999).count();
                sample_count += samples.len();
                spectrum_frames += collect_spectrum(
                    &samples,
                    start,
                    &intervals,
                    &mut spectrum,
                    BANDWIDTH_FRAMES - spectrum_frames,
                );
            }
        }
        start = end;
    }

    let snr_db = match (percentile(&speech_db, 0.9), percentile(&silence_db, 0.1)) {
        (Some(speech), Some(noise)) if silence_db.len() >= 5 => Some(speech - noise),
        _ => None,
    };
    let clipping_ratio = (sample_count > 0).then(|| clipping as f64 / sample_count as f64);
    let bandwidth_hz = spectrum
        .iter()
        .any(|power| *power > 0.0)
        .then(|| band_edge(&spectrum));
    let channel_balance_db = if usable >= 2 {
        match (
            percentile(&channel_db[0], 0.5),
            percentile(&channel_db[1], 0.5),
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
    if bandwidth_hz.is_some_and(|v| v < 4_000.0) {
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

fn speech_intervals(turns: &[Turn]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = turns
        .iter()
        .filter(|t| t.end > t.start)
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
