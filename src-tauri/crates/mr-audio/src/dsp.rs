pub struct HighPassFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl HighPassFilter {
    pub fn new(cutoff_hz: f32, sample_rate_hz: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate_hz;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b0 = (1.0 + cos_w0) / 2.0 / a0;
        let b1 = -(1.0 + cos_w0) / a0;
        let b2 = (1.0 + cos_w0) / 2.0 / a0;
        let a1 = -2.0 * cos_w0 / a0;
        let a2 = (1.0 - alpha) / a0;

        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            let x = *sample;
            let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
                - self.a1 * self.y1
                - self.a2 * self.y2;
            self.x2 = self.x1;
            self.x1 = x;
            self.y2 = self.y1;
            self.y1 = y;
            *sample = y;
        }
    }
}

pub fn normalize_speech(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mut filtered = samples.to_vec();
    let mut hp = HighPassFilter::new(80.0, 16000.0);
    hp.process(&mut filtered);

    const FRAME_SIZE: usize = 1600;
    let mut frame_rms = Vec::with_capacity(filtered.len() / FRAME_SIZE + 1);
    for chunk in filtered.chunks(FRAME_SIZE) {
        let sum_sq: f32 = chunk.iter().map(|&x| x * x).sum();
        frame_rms.push((sum_sq / chunk.len() as f32).sqrt());
    }

    let mut sorted_rms = frame_rms.clone();
    sorted_rms.sort_by(f32::total_cmp);
    let p5_idx = (sorted_rms.len() as f32 * 0.05).round() as usize;
    let p5_rms = sorted_rms.get(p5_idx).copied().unwrap_or(0.0);

    let noise_floor = (p5_rms * 0.8).clamp(0.0003, 0.0012);
    const TARGET_RMS: f32 = 0.045;
    const MAX_GAIN: f32 = 14.0;
    const HOP_LEN: usize = 4000;
    const WINDOW_LEN: usize = 8000;

    let num_hops = filtered.len().div_ceil(HOP_LEN);
    let mut hop_gains = Vec::with_capacity(num_hops);

    for h in 0..num_hops {
        let center = h * HOP_LEN;
        let start = center.saturating_sub(WINDOW_LEN / 2);
        let end = (center + WINDOW_LEN / 2).min(filtered.len());
        let chunk = &filtered[start..end];
        let rms = if chunk.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = chunk.iter().map(|&x| x * x).sum();
            (sum_sq / chunk.len() as f32).sqrt()
        };

        let gain = if rms < noise_floor {
            1.0
        } else if rms < noise_floor * 1.5 {
            let t = (rms - noise_floor) / (noise_floor * 0.5);
            let des = (TARGET_RMS / rms).min(MAX_GAIN);
            1.0 + t * (des - 1.0)
        } else {
            (TARGET_RMS / rms).min(MAX_GAIN)
        };
        hop_gains.push(gain);
    }

    let mut out = Vec::with_capacity(filtered.len());
    let mut cur_gain = hop_gains.first().copied().unwrap_or(1.0);

    for (i, &sample) in filtered.iter().enumerate() {
        let hop_idx = (i / HOP_LEN).min(hop_gains.len().saturating_sub(1));
        let target_g = hop_gains[hop_idx];
        let rate = if target_g < cur_gain { 0.005 } else { 0.0008 };
        cur_gain += (target_g - cur_gain) * rate;

        let boosted = sample * cur_gain;
        let limited = if boosted.abs() > 0.95 {
            boosted.signum() * (0.95 + 0.045 * ((boosted.abs() - 0.95) / 0.045).tanh())
        } else {
            boosted
        };
        out.push(limited.clamp(-1.0, 1.0));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highpass_attenuates_subbass() {
        let mut hp = HighPassFilter::new(80.0, 16000.0);
        let mut subbass: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 20.0 * i as f32 / 16000.0).sin())
            .collect();
        hp.process(&mut subbass);
        let sub_rms = (subbass[8000..].iter().map(|&x| x * x).sum::<f32>() / 8000.0).sqrt();
        assert!(sub_rms < 0.15, "80Hz фильтр должен давить 20Hz сигнал");

        let mut hp_mid = HighPassFilter::new(80.0, 16000.0);
        let mut mid: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 16000.0).sin())
            .collect();
        hp_mid.process(&mut mid);
        let mid_rms = (mid[8000..].iter().map(|&x| x * x).sum::<f32>() / 8000.0).sqrt();
        assert!(mid_rms > 0.65, "1000Hz сигнал должен проходить без потерь");
    }

    #[test]
    fn normalize_speech_amplifies_quiet_signal() {
        let quiet: Vec<f32> = (0..32000)
            .map(|i| 0.003 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        let normalized = normalize_speech(&quiet);
        let out_rms = (normalized[8000..24000].iter().map(|&x| x * x).sum::<f32>() / 16000.0).sqrt();
        assert!(out_rms > 0.02, "Тихий сигнал должен быть усилен AGC");
        assert!(normalized.iter().all(|&x| x.abs() <= 1.0));
    }

    #[test]
    fn normalize_speech_does_not_explode_on_silence() {
        let silence = vec![0.0001f32; 16000];
        let normalized = normalize_speech(&silence);
        let max_val = normalized.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(max_val < 0.01, "Шумовой пол не должен разгоняться компрессором");
    }
}
