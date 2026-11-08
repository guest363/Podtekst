//! Детектор «смешанного стерео»: оба канала содержат один и тот же разговор,
//! а не раздельные микрофон/систему. Так бывает при выводе звука на колонки:
//! микрофон слышит собеседников, и split теряет смысл. Проверка идёт сразу
//! после декодирования, до VAD и ASR; при дубле остаётся микрофонный канал.

const FRAME_SAMPLES: usize = 8_000; // 0.5 с при 16 кГц

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

/// Возвращает индекс канала с лучшим уровнем, если оба канала — один микс.
pub(crate) fn mixed_stereo(channels: &[Vec<f32>]) -> Option<usize> {
    if channels.len() != 2 {
        return None;
    }
    let len = channels[0].len().min(channels[1].len());
    if len < FRAME_SAMPLES * 6 {
        return None;
    }
    let env0 = smooth(&envelope(&channels[0][..len]));
    let env1 = smooth(&envelope(&channels[1][..len]));
    let corr = correlation(&env0, &env1);
    let th0 = (percentile(&env0, 0.9) * 0.2).max(0.005);
    let th1 = (percentile(&env1, 0.9) * 0.2).max(0.005);
    let active0: Vec<bool> = env0.iter().map(|&x| x > th0).collect();
    let active1: Vec<bool> = env1.iter().map(|&x| x > th1).collect();
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
        return None;
    }
    let containment = overlap as f64 / min_active as f64;
    let union =
        active0.iter().filter(|&&a| a).count() + active1.iter().filter(|&&b| b).count() - overlap;
    let jaccard = overlap as f64 / union.max(1) as f64;
    // Высокий Jaccard означает, что почти вся речь есть в обоих каналах.
    // Для реального split с наушниками локальный и удалённый голоса
    // чередуются, поэтому пересечение активности заметно ниже.
    // shortcut: 0.70/0.85/0.75 — стартовые пороги без размеченного набора;
    // откалибровать на записях в наушниках и с колонками.
    if corr < 0.70 || containment < 0.85 || jaccard < 0.75 {
        return None;
    }
    // Канал 0 — микрофон: он слышит комнату и остаётся основным источником.
    // Системный канал при дубле — обработанный loopback той же речи.
    Some(0)
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

    #[test]
    fn identical_mixes_are_detected() {
        let len = 16_000 * 8;
        let spans = [(0.0, 2.0), (2.5, 4.0), (5.0, 7.5)];
        let a = bursts(len, &spans, 0.2, 1);
        let b: Vec<f32> = bursts(len, &spans, 0.35, 2)
            .iter()
            .map(|x| x * 0.8)
            .collect();
        assert_eq!(mixed_stereo(&[a, b]), Some(0));
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
}
