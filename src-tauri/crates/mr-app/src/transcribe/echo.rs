use mr_ml::onnx::vad::VAD_WINDOW;

use super::channel_mix::{percentile, vad_envelope, window_corr};

const SAMPLE_RATE: usize = 16_000;
pub(crate) const PROBE_SEC: usize = 30;
const PROBE_COUNT: usize = 5;
const MIN_INFORMATIVE_PROBES: usize = 3;
const MIN_PROMINENCE: f32 = 8.0;
/// 4 мс: ниже этой задержки микрофон ближе к цифровому дублю канала, а не к эху из динамиков.
const MIN_DELAY_SAMPLES: usize = 64;
const MAX_DELAY_SAMPLES: usize = 8_000;
const MAX_DELAY_SPREAD: usize = 32;
const CORR_HALF_FRAMES: usize = 10;
const GAIN_MIN: f32 = 0.05;
const GAIN_MAX: f32 = 4.0;
/// Калибровка на синтетике (тон + медленная огибающая): чистому эху хватает
/// корреляции огибающих 0.65, двойная речь видна по остатку уже с 0.45.
const ECHO_CORR: f64 = 0.65;
const DOUBLE_TALK_CORR: f64 = 0.45;
const ECHO_LEVEL_MARGIN: f32 = 1.15;
const RESIDUAL_MARGIN: f32 = 1.5;
/// Остаток sqrt(m² − p²) второго порядка: расхождение огибающих 0.2% уже даёт
/// ~6% остатка из-за нецелой задержки в кадрах. Поэтому шумовой порог
/// дополняется относительным — локальная речь ниже 15% уровня эха не речь.
const RESIDUAL_RELATIVE: f32 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameClass {
    EchoOnly,
    LocalOnly,
    DoubleTalk,
    Uncertain,
}

#[derive(Debug, Clone)]
pub(crate) struct EchoEvidence {
    pub(crate) delay_samples: usize,
    pub(crate) delay_sec: f64,
    pub(crate) prominence: f32,
    pub(crate) stability: f32,
    pub(crate) gain: f32,
    pub(crate) noise_floor: f32,
    pub(crate) frames: Vec<FrameClass>,
    pub(crate) residual: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct SegmentProfile {
    pub(crate) echo_only: f32,
    pub(crate) local_only: f32,
    pub(crate) double_talk: f32,
    pub(crate) uncertain: f32,
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() / 2]
}

fn frame_sec() -> f64 {
    VAD_WINDOW as f64 / SAMPLE_RATE as f64
}

fn agreeing_delays(strong: &[(usize, f32)]) -> Option<Vec<(usize, f32)>> {
    let mut sorted = strong.to_vec();
    sorted.sort_by_key(|&(delay, _)| delay);
    let mut lo = 0usize;
    let mut best: &[(usize, f32)] = &[];
    for hi in 0..sorted.len() {
        while sorted[hi].0 - sorted[lo].0 > MAX_DELAY_SPREAD {
            lo += 1;
        }
        if hi - lo + 1 > best.len() {
            best = &sorted[lo..=hi];
        }
    }
    (best.len() >= MIN_INFORMATIVE_PROBES).then(|| best.to_vec())
}

fn lagged_envelope(env: &[f32], frame: usize, delay: f32) -> f32 {
    let pos = frame as f32 - delay;
    let base = pos.floor();
    if base < 0.0 {
        return 0.0;
    }
    let index = base as usize;
    let next = env.get(index + 1).copied().unwrap_or(0.0);
    env.get(index).copied().unwrap_or(0.0) * (1.0 - (pos - base)) + next * (pos - base)
}

fn best_corr(mic_env: &[f32], sys_env: &[f32], frame: usize, other: usize) -> f64 {
    let mut best = 0.0f64;
    for offset in [-1isize, 0, 1] {
        let candidate = other as isize + offset;
        if candidate < 0 || candidate as usize >= sys_env.len() {
            continue;
        }
        best = best.max(window_corr(
            mic_env,
            sys_env,
            frame,
            candidate as usize,
            CORR_HALF_FRAMES,
        ));
    }
    best
}

/// `mic_probs` — исходные вероятности Silero до эхо-гейта: без них тишина и
/// реверберация в паузах выглядят как «локальная речь» и запрещают отсев эха.
pub(crate) fn detect(
    mic: &[f32],
    system: &[f32],
    mic_probs: Option<&[f32]>,
) -> Option<EchoEvidence> {
    let len = mic.len().min(system.len());
    if len < VAD_WINDOW {
        return None;
    }
    let window = (PROBE_SEC * SAMPLE_RATE).min(len);
    let rms = |a: usize, b: usize| -> f32 {
        let slice = &system[a..b.min(system.len())];
        if slice.is_empty() {
            return 0.0;
        }
        (slice.iter().map(|x| x * x).sum::<f32>() / slice.len() as f32).sqrt()
    };
    let mut candidates: Vec<(f32, usize)> = (0..=8)
        .map(|step| {
            let start = ((len - window) as f64 * step as f64 / 8.0) as usize;
            (rms(start, start + window), start)
        })
        .collect();
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let probes: Vec<(usize, f32)> = candidates
        .into_iter()
        .take(PROBE_COUNT)
        .filter_map(|(_, start)| {
            mr_audio::echo::delay::estimate(
                &mic[start..start + window],
                &system[start..start + window],
                MAX_DELAY_SAMPLES,
            )
        })
        .collect();
    if probes.len() < MIN_INFORMATIVE_PROBES {
        return None;
    }
    log::debug!(target: "audio", n = probes.len(), probes = format!("{probes:?}"); "ev=audio.echo_probes");
    let strong: Vec<(usize, f32)> = probes
        .iter()
        .copied()
        .filter(|&(delay, prominence)| {
            prominence >= MIN_PROMINENCE && (MIN_DELAY_SAMPLES..=MAX_DELAY_SAMPLES).contains(&delay)
        })
        .collect();
    let Some(agreeing) = agreeing_delays(&strong) else {
        log::debug!(target: "audio", strong = format!("{strong:?}"), spread = MAX_DELAY_SPREAD; "ev=audio.echo_unstable");
        return None;
    };
    let stability = agreeing.len() as f32 / probes.len() as f32;
    let delay_samples = agreeing[agreeing.len() / 2].0;
    let prominence = agreeing
        .iter()
        .map(|&(_, value)| value)
        .fold(0.0f32, f32::max);

    let mic_env = vad_envelope(mic);
    let sys_env = vad_envelope(system);
    if mic_env.is_empty() || sys_env.is_empty() {
        return None;
    }
    let noise_floor = percentile(&mic_env, 0.10).max(1e-4);
    let delay_frames = delay_samples as f32 / VAD_WINDOW as f32;
    let corr: Vec<f64> = (0..mic_env.len())
        .map(|frame| {
            let pos = frame as f32 - delay_frames;
            if pos < 0.0 {
                return 0.0;
            }
            best_corr(&mic_env, &sys_env, frame, pos.round() as usize)
        })
        .collect();

    let mut ratios: Vec<f32> = Vec::new();
    for (frame, &value) in corr.iter().enumerate() {
        if value < ECHO_CORR {
            continue;
        }
        let sys_level = lagged_envelope(&sys_env, frame, delay_frames);
        let mic_level = mic_env[frame];
        if sys_level <= noise_floor || mic_level <= noise_floor {
            continue;
        }
        let ratio = mic_level / sys_level;
        if (0.02..=2.0).contains(&ratio) {
            ratios.push(ratio);
        }
    }
    let gain = if ratios.is_empty() {
        1.0
    } else {
        median(&ratios).clamp(GAIN_MIN, GAIN_MAX)
    };

    let mut frames = Vec::with_capacity(mic_env.len());
    let mut residual = Vec::with_capacity(mic_env.len());
    for (frame, &value) in corr.iter().enumerate() {
        let mic_level = mic_env[frame];
        let sys_level = lagged_envelope(&sys_env, frame, delay_frames);
        let predicted = gain * sys_level;
        let rest = (mic_level * mic_level - predicted * predicted)
            .max(0.0)
            .sqrt();
        let rest_floor = (noise_floor * RESIDUAL_MARGIN).max(mic_level * RESIDUAL_RELATIVE);
        let speech = mic_probs
            .and_then(|probs| probs.get(frame).copied())
            .is_none_or(|prob| prob >= 0.30);
        let class = if sys_level < noise_floor && mic_level > noise_floor * 3.0 && speech {
            FrameClass::LocalOnly
        } else if value >= ECHO_CORR
            && mic_level <= predicted * ECHO_LEVEL_MARGIN
            && rest <= rest_floor
        {
            FrameClass::EchoOnly
        } else if value >= DOUBLE_TALK_CORR && rest > rest_floor && speech {
            FrameClass::DoubleTalk
        } else {
            FrameClass::Uncertain
        };
        frames.push(class);
        residual.push(rest);
    }

    Some(EchoEvidence {
        delay_samples,
        delay_sec: delay_samples as f64 / SAMPLE_RATE as f64,
        prominence,
        stability,
        gain,
        noise_floor,
        frames,
        residual,
    })
}

fn frame_range(ev: &EchoEvidence, start: f64, end: f64) -> (usize, usize) {
    let win = frame_sec();
    let lo = (start.max(0.0) / win).floor() as usize;
    let hi = (end.max(0.0) / win).ceil() as usize;
    (lo.min(ev.frames.len()), hi.min(ev.frames.len()))
}

pub(crate) fn segment_profile(ev: &EchoEvidence, start: f64, end: f64) -> SegmentProfile {
    let (lo, hi) = frame_range(ev, start, end);
    if hi <= lo {
        return SegmentProfile {
            uncertain: 1.0,
            ..SegmentProfile::default()
        };
    }
    let mut profile = SegmentProfile::default();
    for class in &ev.frames[lo..hi] {
        match class {
            FrameClass::EchoOnly => profile.echo_only += 1.0,
            FrameClass::LocalOnly => profile.local_only += 1.0,
            FrameClass::DoubleTalk => profile.double_talk += 1.0,
            FrameClass::Uncertain => profile.uncertain += 1.0,
        }
    }
    let total = (hi - lo) as f32;
    profile.echo_only /= total;
    profile.local_only /= total;
    profile.double_talk /= total;
    profile.uncertain /= total;
    profile
}

pub(crate) fn trim_echo_edges(
    ev: &EchoEvidence,
    start: f64,
    end: f64,
    min_keep_sec: f64,
) -> Option<(f64, f64)> {
    let (lo, hi) = frame_range(ev, start, end);
    if hi <= lo {
        return (end - start >= min_keep_sec).then_some((start, end));
    }
    let first = (lo..hi).find(|&i| ev.frames[i] != FrameClass::EchoOnly)?;
    let last = (lo..hi)
        .rev()
        .find(|&i| ev.frames[i] != FrameClass::EchoOnly)?;
    let win = frame_sec();
    let trimmed_start = if first > lo {
        (first as f64 * win).max(start)
    } else {
        start
    };
    let trimmed_end = if last + 1 < hi {
        ((last + 1) as f64 * win).min(end)
    } else {
        end
    };
    (trimmed_end - trimmed_start >= min_keep_sec).then_some((trimmed_start, trimmed_end))
}

/// Средний остаток по фрагменту: показывает, насколько чисто предсказано эхо
/// (калибровка порогов на живых записях).
pub(crate) fn mean_residual(ev: &EchoEvidence, start: f64, end: f64) -> f32 {
    let (lo, hi) = frame_range(ev, start, end);
    if hi <= lo {
        return 0.0;
    }
    ev.residual[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
}

/// Понадобится этапу пословной дедупликации: конвейер stage 1 его не вызывает.
#[allow(dead_code)]
pub(crate) fn longest_local_run(ev: &EchoEvidence, start: f64, end: f64) -> Option<(f64, f64)> {
    let (lo, hi) = frame_range(ev, start, end);
    let mut best: Option<(usize, usize)> = None;
    let mut run_start: Option<usize> = None;
    for index in lo..hi {
        let local = ev.frames[index] != FrameClass::EchoOnly;
        match (local, run_start) {
            (true, None) => run_start = Some(index),
            (false, Some(begin)) => {
                if best.is_none_or(|(a, b)| index - begin > b - a) {
                    best = Some((begin, index));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = run_start {
        if best.is_none_or(|(a, b)| hi - begin > b - a) {
            best = Some((begin, hi));
        }
    }
    let win = frame_sec();
    best.map(|(begin, stop)| (begin as f64 * win, stop as f64 * win))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: usize = 16_000 * 12;
    const DELAY: usize = 1000;

    fn block_factor(seed: u64, block: u64) -> f32 {
        let mut hash = seed ^ block.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        hash ^= hash >> 29;
        hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        hash ^= hash >> 32;
        0.3 + 0.7 * (hash % 1000) as f32 / 999.0
    }

    /// Тон с медленной (блок 0.25 с) огибающей: у плоского сигнала корреляция
    /// огибающих вырождена в ноль, и классификация кадров не проверяется.
    fn am_bursts(len: usize, spans: &[(f64, f64)], gain: f32, seed: u64, base_hz: f64) -> Vec<f32> {
        let mut phase = 0.0f64;
        (0..len)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                if !spans.iter().any(|&(a, b)| t >= a && t < b) {
                    return 0.0;
                }
                let factor = block_factor(seed, (t / 0.25) as u64);
                let freq = base_hz + 2000.0 * (t % 4.0) / 4.0;
                phase += std::f64::consts::TAU * freq / SAMPLE_RATE as f64;
                (phase.sin() as f32) * gain * factor
            })
            .collect()
    }

    fn delayed_echo(system: &[f32], delay: usize, gain: f32) -> Vec<f32> {
        let mut mic = vec![0.0f32; delay];
        mic.extend(system[..system.len() - delay].iter().map(|x| x * gain));
        mic
    }

    fn evidence(frames: Vec<FrameClass>) -> EchoEvidence {
        EchoEvidence {
            delay_samples: DELAY,
            delay_sec: DELAY as f64 / SAMPLE_RATE as f64,
            prominence: 12.0,
            stability: 1.0,
            gain: 0.1,
            noise_floor: 1e-4,
            residual: vec![0.0; frames.len()],
            frames,
        }
    }

    #[test]
    fn echo_only_recording_is_classified_as_echo() {
        let system = am_bursts(LEN, &[(0.5, 3.5), (4.5, 7.5), (8.5, 11.5)], 0.4, 7, 300.0);
        let mic = delayed_echo(&system, DELAY, 0.1);
        let ev = detect(&mic, &system, None).expect("эхо должно быть найдено");
        assert!(
            (ev.delay_samples as i64 - DELAY as i64).abs() <= MAX_DELAY_SPREAD as i64,
            "{ev:?}"
        );
        assert!(ev.prominence >= MIN_PROMINENCE, "{ev:?}");
        assert!((0.05..0.2).contains(&ev.gain), "{ev:?}");
        assert!(ev.stability >= 0.6, "{ev:?}");
        let echo = ev
            .frames
            .iter()
            .filter(|&&class| class == FrameClass::EchoOnly)
            .count();
        assert!(
            echo * 2 > ev.frames.len(),
            "echo_only {echo} из {}",
            ev.frames.len()
        );
    }

    #[test]
    fn local_speech_over_silent_system_stays_local() {
        let system = am_bursts(LEN, &[(0.5, 4.0), (8.0, 11.5)], 0.4, 11, 300.0);
        let mut mic = delayed_echo(&system, DELAY, 0.1);
        let local = am_bursts(LEN, &[(5.0, 6.0)], 0.3, 23, 900.0);
        for (sample, speech) in mic.iter_mut().zip(local) {
            *sample += speech;
        }
        let ev = detect(&mic, &system, None).expect("эхо должно быть найдено");
        let profile = segment_profile(&ev, 5.1, 5.9);
        assert!(profile.local_only >= 0.8, "{profile:?}");
        assert!(profile.echo_only <= 0.1, "{profile:?}");
    }

    #[test]
    fn simultaneous_speech_is_classified_as_double_talk() {
        let system = am_bursts(LEN, &[(0.5, 11.5)], 0.4, 31, 300.0);
        let mut mic = delayed_echo(&system, DELAY, 0.1);
        let local = am_bursts(LEN, &[(4.0, 6.0)], 0.12, 31, 900.0);
        for (sample, speech) in mic.iter_mut().zip(local) {
            *sample += speech;
        }
        let ev = detect(&mic, &system, None).expect("эхо должно быть найдено");
        let profile = segment_profile(&ev, 4.2, 5.8);
        assert!(profile.double_talk >= 0.5, "{profile:?}");
        assert!(profile.echo_only <= 0.2, "{profile:?}");
    }

    #[test]
    fn uncorrelated_channels_have_no_evidence() {
        let system = am_bursts(LEN, &[(0.5, 11.5)], 0.4, 41, 300.0);
        let mic = am_bursts(LEN, &[(0.5, 11.5)], 0.3, 43, 900.0);
        assert!(detect(&mic, &system, None).is_none());
    }

    #[test]
    fn short_and_empty_channels_have_no_evidence() {
        assert!(detect(&[], &[], None).is_none());
        assert!(detect(&vec![0.0; 100], &vec![0.0; 100], None).is_none());
        assert!(detect(&vec![0.0; LEN], &[0.0; 10], None).is_none());
    }

    #[test]
    fn segment_profile_counts_classes() {
        let mut frames = vec![FrameClass::EchoOnly; 80];
        frames.extend(vec![FrameClass::LocalOnly; 10]);
        frames.extend(vec![FrameClass::Uncertain; 10]);
        let ev = evidence(frames);
        let profile = segment_profile(&ev, 0.0, 100.0 * frame_sec());
        assert!((profile.echo_only - 0.8).abs() < 1e-6, "{profile:?}");
        assert!((profile.local_only - 0.1).abs() < 1e-6, "{profile:?}");
        assert!((profile.uncertain - 0.1).abs() < 1e-6, "{profile:?}");
        let empty = segment_profile(&ev, 1000.0, 1001.0);
        assert!((empty.uncertain - 1.0).abs() < 1e-6, "{empty:?}");
    }

    #[test]
    fn trim_echo_edges_keeps_local_part() {
        let mut frames = vec![FrameClass::EchoOnly; 10];
        frames.extend(vec![FrameClass::LocalOnly; 20]);
        frames.extend(vec![FrameClass::EchoOnly; 5]);
        let ev = evidence(frames);
        let span = 35.0 * frame_sec();
        let (start, end) = trim_echo_edges(&ev, 0.0, span, 0.3).expect("локальная часть есть");
        assert!((start - 10.0 * frame_sec()).abs() < 1e-6, "{start}");
        assert!((end - 30.0 * frame_sec()).abs() < 1e-6, "{end}");

        let mut short = vec![FrameClass::EchoOnly];
        short.push(FrameClass::LocalOnly);
        short.push(FrameClass::EchoOnly);
        assert!(trim_echo_edges(&evidence(short), 0.0, 3.0 * frame_sec(), 0.3).is_none());
    }

    #[test]
    fn longest_local_run_ignores_echo_frames() {
        let mut frames = vec![FrameClass::EchoOnly; 5];
        frames.extend(vec![FrameClass::DoubleTalk; 3]);
        frames.push(FrameClass::EchoOnly);
        frames.extend(vec![FrameClass::LocalOnly; 7]);
        frames.extend(vec![FrameClass::EchoOnly; 4]);
        let ev = evidence(frames);
        let (start, end) = longest_local_run(&ev, 0.0, 20.0 * frame_sec()).expect("есть прогон");
        assert!((start - 9.0 * frame_sec()).abs() < 1e-6, "{start}");
        assert!((end - 16.0 * frame_sec()).abs() < 1e-6, "{end}");
        assert!(longest_local_run(
            &evidence(vec![FrameClass::EchoOnly; 4]),
            0.0,
            4.0 * frame_sec()
        )
        .is_none());
    }
}
