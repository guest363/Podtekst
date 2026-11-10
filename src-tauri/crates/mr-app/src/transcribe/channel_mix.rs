const FRAME_SAMPLES: usize = 8_000;
const MAX_DUP_LAG: i64 = 64;
const DUP_LAG_PROBE_SEC: usize = 30;
const DUP_SAMPLE_CORR: f64 = 0.70;

fn envelope(samples: &[f32]) -> Vec<f32> {
    samples
        .chunks(FRAME_SAMPLES)
        .map(|frame| (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt())
        .collect()
}

fn smooth(values: &[f32]) -> Vec<f32> {
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 2).min(values.len());
            values[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
        })
        .collect()
}

fn percentile(values: &[f32], fraction: f32) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let index = ((sorted.len().saturating_sub(1)) as f32 * fraction).round() as usize;
    sorted.get(index).copied().unwrap_or(0.0)
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let mean_a = a[..n].iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mean_b = b[..n].iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for (&x, &y) in a[..n].iter().zip(&b[..n]) {
        let dx = x as f64 - mean_a;
        let dy = y as f64 - mean_b;
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    if var_a < 1e-12 || var_b < 1e-12 {
        return 0.0;
    }
    cov / (var_a.sqrt() * var_b.sqrt())
}

fn sample_correlation_at_lag(a: &[f32], b: &[f32], lag: i64) -> f64 {
    let n = a.len().min(b.len());
    let (a_start, b_start) = if lag >= 0 {
        (lag as usize, 0)
    } else {
        (0, (-lag) as usize)
    };
    if a_start >= n || b_start >= n {
        return 0.0;
    }
    let len = n - a_start.max(b_start);
    if len < 1000 {
        return 0.0;
    }
    let a = &a[a_start..a_start + len];
    let b = &b[b_start..b_start + len];
    let mut sum_a = 0.0;
    let mut sum_b = 0.0;
    let mut count = 0usize;
    for (&x, &y) in a.iter().zip(b).step_by(8) {
        sum_a += x as f64;
        sum_b += y as f64;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    let mean_a = sum_a / count as f64;
    let mean_b = sum_b / count as f64;
    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for (&x, &y) in a.iter().zip(b).step_by(8) {
        let dx = x as f64 - mean_a;
        let dy = y as f64 - mean_b;
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    if var_a < 1e-12 || var_b < 1e-12 {
        return 0.0;
    }
    cov / (var_a.sqrt() * var_b.sqrt())
}

fn best_lag_sample_correlation(a: &[f32], b: &[f32]) -> (f64, i64) {
    let probe = a.len().min(b.len()).min(DUP_LAG_PROBE_SEC * 16000);
    let mut best = (sample_correlation_at_lag(&a[..probe], &b[..probe], 0), 0);
    for lag in 1..=MAX_DUP_LAG {
        for candidate in [lag, -lag] {
            let corr = sample_correlation_at_lag(&a[..probe], &b[..probe], candidate);
            if corr > best.0 {
                best = (corr, candidate);
            }
        }
    }
    let full = sample_correlation_at_lag(a, b, best.1);
    (full, best.1)
}

/// Метрики решения «каналы дублируют речь»: без них не видно, какой порог
/// свернул стерео в моно.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct MixStats {
    pub len: usize,
    pub corr: f64,
    pub containment: f64,
    pub jaccard: f64,
    pub sample_corr: f64,
    pub lag: i64,
    pub th0: f32,
    pub th1: f32,
    pub chosen: Option<usize>,
}

#[cfg(test)]
pub(crate) fn mixed_stereo(channels: &[Vec<f32>]) -> Option<usize> {
    mixed_stereo_with_stats(channels).0
}

pub(crate) fn mixed_stereo_with_stats(channels: &[Vec<f32>]) -> (Option<usize>, MixStats) {
    let mut stats = MixStats::default();
    if channels.len() != 2 {
        return (None, stats);
    }
    let len = channels[0].len().min(channels[1].len());
    stats.len = len;
    if len < FRAME_SAMPLES * 6 {
        return (None, stats);
    }
    let env0 = smooth(&envelope(&channels[0][..len]));
    let env1 = smooth(&envelope(&channels[1][..len]));
    stats.corr = correlation(&env0, &env1);
    stats.th0 = (percentile(&env0, 0.9) * 0.2).max(0.005);
    stats.th1 = (percentile(&env1, 0.9) * 0.2).max(0.005);
    let active0: Vec<bool> = env0.iter().map(|&x| x > stats.th0).collect();
    let active1: Vec<bool> = env1.iter().map(|&x| x > stats.th1).collect();
    let overlap = active0
        .iter()
        .zip(&active1)
        .filter(|(a, b)| **a && **b)
        .count();
    let active_count0 = active0.iter().filter(|&&a| a).count();
    let active_count1 = active1.iter().filter(|&&b| b).count();
    let min_active = active_count0.min(active_count1);
    let max_active = active_count0.max(active_count1);
    if min_active < active0.len() / 20 || min_active * 5 < max_active * 4 {
        return (None, stats);
    }
    stats.containment = overlap as f64 / min_active as f64;
    let union =
        active0.iter().filter(|&&a| a).count() + active1.iter().filter(|&&b| b).count() - overlap;
    stats.jaccard = overlap as f64 / union.max(1) as f64;
    // shortcut: 0.70/0.85/0.75 — стартовые огибающие пороги отсечения независимой активности.
    if stats.corr < 0.70 || stats.containment < 0.85 || stats.jaccard < 0.75 {
        return (None, stats);
    }
    let probe = len.min(BLEED_PROBE_SEC * 16000);
    if let Some((delay, prominence)) =
        mr_audio::echo::delay::estimate(&channels[0][..probe], &channels[1][..probe], 8000)
    {
        // shortcut: delay >= 64 (4 мс) и prominence >= 8.0 фиксируют акустическое
        // эхо из динамиков в микрофон (не моно-дубль); каналы сохраняются для suppress_mic_bleed.
        if prominence >= 8.0 && delay >= 64 {
            return (None, stats);
        }
    }
    // Нулевой лаг не ловит цифровой дубль со сдвигом в несколько сэмплов:
    // ищем максимум в пределах 4 мс, дальше это уже акустическое эхо.
    let (sample_corr, lag) = best_lag_sample_correlation(&channels[0][..len], &channels[1][..len]);
    stats.sample_corr = sample_corr;
    stats.lag = lag;
    if sample_corr < DUP_SAMPLE_CORR {
        return (None, stats);
    }
    stats.chosen = Some(if stats.th1 > stats.th0 { 1 } else { 0 });
    (stats.chosen, stats)
}

const VAD_FRAME: usize = 512;
const BLEED_PROBE_SEC: usize = 60;

fn vad_envelope(samples: &[f32]) -> Vec<f32> {
    samples
        .chunks(VAD_FRAME)
        .map(|frame| (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt())
        .collect()
}

fn window_corr(a: &[f32], b: &[f32], center: usize, other: usize, half: usize) -> f64 {
    let left = center.min(other).min(half);
    let right = (a.len().saturating_sub(center + 1))
        .min(b.len().saturating_sub(other + 1))
        .min(half);
    let n = left + right + 1;
    if n < 4 {
        return 0.0;
    }
    let aa = &a[center - left..=center + right];
    let bb = &b[other - left..=other + right];
    let mean_a = aa.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mean_b = bb.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;
    for (&x, &y) in aa.iter().zip(bb) {
        let dx = x as f64 - mean_a;
        let dy = y as f64 - mean_b;
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    if var_a < 1e-12 || var_b < 1e-12 {
        return 0.0;
    }
    cov / (var_a.sqrt() * var_b.sqrt())
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct BleedStats {
    pub delay: usize,
    pub prominence: f32,
    pub suppressed: usize,
    pub frames: usize,
    pub probe_sec: f64,
}

#[cfg(test)]
pub(crate) fn suppress_mic_bleed(mic: &[f32], system: &[f32], vad_probs: &mut [f32]) -> usize {
    suppress_mic_bleed_with_stats(mic, system, vad_probs).suppressed
}

pub(crate) fn suppress_mic_bleed_with_stats(
    mic: &[f32],
    system: &[f32],
    vad_probs: &mut [f32],
) -> BleedStats {
    let mut stats = BleedStats::default();
    let probe = mic.len().min(system.len()).min(BLEED_PROBE_SEC * 16000);
    stats.probe_sec = probe as f64 / 16000.0;
    if probe < 16000 * 2 {
        return stats;
    }
    let Some((delay, prominence)) =
        mr_audio::echo::delay::estimate(&mic[..probe], &system[..probe], 8000)
    else {
        return stats;
    };
    stats.delay = delay;
    stats.prominence = prominence;
    if prominence < 8.0 {
        return stats;
    }
    let mic_env = vad_envelope(mic);
    let sys_env = vad_envelope(system);
    let frames = mic_env.len().min(sys_env.len()).min(vad_probs.len());
    stats.frames = frames;
    let delay_frames = delay / VAD_FRAME;
    let mut suppressed = 0;
    for frame in 0..frames {
        if vad_probs[frame] < 0.1 {
            continue;
        }
        let Some(other) = frame.checked_sub(delay_frames) else {
            continue;
        };
        if other >= sys_env.len() {
            continue;
        }
        let mic_level = mic_env[frame];
        let sys_level = sys_env[other];
        if sys_level < 0.005 || mic_level < 0.001 {
            continue;
        }
        const SUPPRESSION_FLOOR: f32 = 0.05;
        let corr = window_corr(&mic_env, &sys_env, frame, other, 10);
        let corr_prev = if other > 0 {
            window_corr(&mic_env, &sys_env, frame, other - 1, 10)
        } else {
            0.0
        };
        let corr_next = if other + 1 < sys_env.len() {
            window_corr(&mic_env, &sys_env, frame, other + 1, 10)
        } else {
            0.0
        };
        let high_corr = corr.max(corr_prev).max(corr_next);
        let max_ratio = if high_corr > 0.75 { 3.0 } else { 1.2 };
        if mic_level > sys_level * max_ratio || high_corr <= 0.65 {
            continue;
        }
        vad_probs[frame] = vad_probs[frame].min(SUPPRESSION_FLOOR);
        suppressed += 1;
    }
    stats.suppressed = suppressed;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((*seed >> 33) as f32 / u32::MAX as f32) - 0.5
    }

    fn bursts(len: usize, spans: &[(f64, f64)], gain: f32, seed: u64) -> Vec<f32> {
        let mut noise = seed;
        (0..len)
            .map(|i| {
                let t = i as f64 / 16000.0;
                let speech = spans.iter().any(|&(a, b)| t >= a && t < b);
                let tone = (2.0 * std::f64::consts::PI * 220.0 * t).sin() as f32;
                if speech {
                    tone * gain
                } else {
                    lcg(&mut noise) * 0.002
                }
            })
            .collect()
    }

    fn chirp_bursts(len: usize, spans: &[(f64, f64)], gain: f32) -> Vec<f32> {
        let mut phase = 0.0f64;
        (0..len)
            .map(|i| {
                let t = i as f64 / 16000.0;
                if spans.iter().any(|&(a, b)| t >= a && t < b) {
                    let freq = 300.0 + 2000.0 * t / 4.0;
                    phase += std::f64::consts::TAU * freq / 16000.0;
                    (phase.sin() as f32) * gain
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn bleed_from_system_is_gated_in_mic_vad() {
        let len = 16_000 * 4;
        let spans = [(0.5, 2.0), (2.5, 4.0)];
        let system = chirp_bursts(len, &spans, 0.2);
        let delay = 1000usize;
        let mut mic = vec![0.0f32; delay];
        mic.extend(system[..system.len() - delay].iter().map(|x| x * 0.1));
        let mut probs = vec![1.0f32; len.div_ceil(VAD_FRAME)];
        let suppressed = suppress_mic_bleed(&mic, &system, &mut probs);
        assert!(suppressed > 0, "пролезание не найдено");
        let quiet = 20usize;
        assert!(probs[quiet] <= 0.20, "дальний спикер остался в микрофоне");
    }

    #[test]
    fn double_talk_keeps_mic_speech() {
        let len = 16_000 * 4;
        let spans = [(0.5, 4.0)];
        let system = chirp_bursts(len, &spans, 0.2);
        let delay = 1000usize;
        let mut mic = vec![0.0f32; delay];
        mic.extend(system[..system.len() - delay].iter().map(|x| x * 0.1));
        let user = bursts(len, &[(1.0, 2.0)], 0.3, 13);
        for (m, u) in mic.iter_mut().zip(user) {
            *m += u;
        }
        let mut probs = vec![1.0f32; len.div_ceil(VAD_FRAME)];
        suppress_mic_bleed(&mic, &system, &mut probs);
        let frame = 16_000 / VAD_FRAME + 2;
        assert!(probs[frame] > 0.0, "реплика пользователя подавлена");
    }

    #[test]
    fn window_corr_boundary_alignment_keeps_exact_lag() {
        let env_a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let env_b = vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let corr = window_corr(&env_a, &env_b, 3, 5, 5);
        assert!((corr - 1.0).abs() < 1e-6, "corr: {corr}");
    }

    #[test]
    fn identical_mixes_are_detected() {
        let len = 16_000 * 8;
        let spans = [(0.0, 2.0), (2.5, 4.0), (5.0, 7.5)];
        let a = bursts(len, &spans, 0.2, 1);
        let b: Vec<f32> = bursts(len, &spans, 0.35, 2)
            .iter()
            .map(|x| x * 0.8)
            .collect();
        assert_eq!(mixed_stereo(&[a, b]), Some(1));
    }

    #[test]
    fn delayed_gained_duplicate_is_detected() {
        let len = 16_000 * 8;
        let spans = [(0.0, 2.0), (2.5, 4.0), (5.0, 7.5)];
        let a = bursts(len, &spans, 0.2, 21);
        let delay = 6usize;
        let mut b = vec![0.0f32; len];
        for i in delay..len {
            b[i] = a[i - delay] * 1.8;
        }
        let (chosen, stats) = mixed_stereo_with_stats(&[a, b]);
        assert_eq!(chosen, Some(1));
        assert_eq!(stats.lag.abs(), delay as i64);
        assert!(stats.sample_corr >= DUP_SAMPLE_CORR, "{stats:?}");
    }

    #[test]
    fn mix_stats_explain_decision() {
        let len = 16_000 * 8;
        let spans = [(0.0, 2.0), (2.5, 4.0), (5.0, 7.5)];
        let a = bursts(len, &spans, 0.2, 11);
        let b = bursts(len, &spans, 0.3, 12);
        let (chosen, stats) = mixed_stereo_with_stats(&[a, b]);
        assert_eq!(chosen, Some(1));
        assert!(stats.corr >= 0.7 && stats.containment >= 0.85 && stats.jaccard >= 0.75);
        assert_eq!(stats.chosen, Some(1));
        assert!(stats.sample_corr >= DUP_SAMPLE_CORR, "{stats:?}");

        let (chosen, stats) = mixed_stereo_with_stats(&[vec![0.0; 100], vec![0.0; 100]]);
        assert!(chosen.is_none());
        assert_eq!(stats.len, 100);
    }

    #[test]
    fn bleed_stats_carry_delay_and_prominence() {
        let len = 16_000 * 4;
        let spans = [(0.5, 2.0), (2.5, 4.0)];
        let system = chirp_bursts(len, &spans, 0.2);
        let delay = 1000usize;
        let mut mic = vec![0.0f32; delay];
        mic.extend(system[..system.len() - delay].iter().map(|x| x * 0.1));
        let mut probs = vec![1.0f32; len.div_ceil(VAD_FRAME)];
        let stats = suppress_mic_bleed_with_stats(&mic, &system, &mut probs);
        assert!(stats.suppressed > 0);
        assert_eq!(stats.delay, delay);
        assert!(stats.prominence >= 8.0);
        assert!(stats.probe_sec > 3.0);
    }

    #[test]
    fn disjoint_speech_is_not_mixed() {
        let len = 16_000 * 8;
        let a = bursts(len, &[(0.0, 2.0), (5.0, 6.0)], 0.2, 3);
        let b = bursts(len, &[(2.5, 4.0), (6.5, 8.0)], 0.2, 4);
        assert!(mixed_stereo(&[a, b]).is_none());
    }

    #[test]
    fn bleed_with_extra_speech_is_not_a_duplicate() {
        let len = 16_000 * 8;
        let a = bursts(len, &[(0.0, 2.0), (4.0, 6.0)], 0.2, 7);
        let mut b = bursts(len, &[(0.0, 1.0), (2.5, 5.5)], 0.3, 8);
        for (i, &x) in a.iter().enumerate() {
            b[i] += x * 0.3;
        }
        assert!(mixed_stereo(&[a, b]).is_none());
    }

    #[test]
    fn mono_and_three_channels_are_not_mixed() {
        let len = 16_000 * 8;
        let a = bursts(len, &[(0.0, 2.0)], 0.2, 5);
        assert!(mixed_stereo(std::slice::from_ref(&a)).is_none());
        assert!(mixed_stereo(&[a.clone(), a.clone(), a]).is_none());
    }

    #[test]
    fn delayed_bleed_is_not_mixed() {
        let len = 16_000 * 8;
        let spans = [(0.5, 2.5), (3.0, 5.0), (5.5, 7.5)];
        let system = chirp_bursts(len, &spans, 0.3);
        let delay = 900usize;
        let mut mic = vec![0.0f32; delay];
        mic.extend(system[..system.len() - delay].iter().map(|x| x * 0.4));
        assert!(mixed_stereo(&[mic, system]).is_none());
    }
}
