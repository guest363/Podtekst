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

/// Оконный AGC (RMS-левеллер) для 16 кГц PCM: вытягивает тихую речь до ~ -20 dBFS
/// перед VAD и ASR, не разгоняя шум в паузах.
// shortcut: фиксированные пороги под речь 16kHz, при клиппинге шумных записей вынести max_gain в конфиг
pub fn agc_rms_16k(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }

    let mut hp = HighPassFilter::new(80.0, 16000.0);
    hp.process(samples);

    const WIN: usize = 6400; // 400 ms @ 16kHz
    const TARGET_RMS: f32 = 0.10; // ~ -20 dBFS
    const NOISE_FLOOR: f32 = 0.002; // ~ -54 dBFS (не тянем цифровой ноль)
    const MAX_GAIN: f32 = 15.0; // до +23.5 dB для шёпота/тихих фраз

    let mut gain = 1.0f32;
    for chunk in samples.chunks_mut(WIN / 4) {
        let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
        let rms = (sum_sq / chunk.len().max(1) as f32).sqrt();
        let desired = if rms > NOISE_FLOOR {
            (TARGET_RMS / rms).clamp(0.5, MAX_GAIN)
        } else {
            1.0
        };
        // Плавная атака/релиз без щелчков
        let alpha = if desired < gain { 0.4 } else { 0.15 };
        gain += alpha * (desired - gain);
        for s in chunk.iter_mut() {
            *s = (*s * gain).tanh(); // soft-clip вместо жёсткого среза
        }
    }
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
    fn agc_rms_16k_amplifies_quiet_signal() {
        let mut quiet: Vec<f32> = (0..32000)
            .map(|i| 0.005 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        agc_rms_16k(&mut quiet);
        let out_rms =
            (quiet[8000..24000].iter().map(|&x| x * x).sum::<f32>() / 16000.0).sqrt();
        assert!(out_rms > 0.03, "Тихий сигнал должен быть усилен AGC (out_rms={out_rms})");
        assert!(quiet.iter().all(|&x| x.abs() <= 1.0));
    }

    #[test]
    fn agc_rms_16k_does_not_explode_on_silence() {
        let mut silence = vec![0.0001f32; 16000];
        agc_rms_16k(&mut silence);
        let max_val = silence.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(
            max_val < 0.01,
            "Шумовой пол не должен разгоняться компрессором"
        );
    }
}
