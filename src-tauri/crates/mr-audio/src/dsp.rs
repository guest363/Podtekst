struct HighPassFilter {
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
    fn new(cutoff_hz: f32, sample_rate_hz: f32) -> Self {
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

    fn process(&mut self, samples: &mut [f32]) {
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

#[derive(Debug, Clone, Copy)]
pub struct AgcStats {
    pub noise_floor: f32,
    pub speech_rms: f32,
    pub gain_p50: f32,
    pub gain_p95: f32,
    pub amplified_fraction: f32,
}

const WINDOW: usize = 1600;
const TARGET_RMS: f32 = 0.1;
const MAX_GAIN: f32 = 100.0;

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    sorted
        .get(((sorted.len().saturating_sub(1)) as f32 * fraction).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

pub fn agc_rms_16k(samples: &mut [f32]) -> AgcStats {
    HighPassFilter::new(80.0, 16_000.0).process(samples);
    let levels: Vec<f32> = samples
        .chunks(WINDOW)
        .map(|chunk| {
            (chunk.iter().map(|value| value * value).sum::<f32>() / chunk.len() as f32).sqrt()
        })
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let noise_floor = percentile(&sorted, 0.1);
    let speech_rms = percentile(&sorted, 0.9);
    let gate = noise_floor * 2.5;
    let desired = (TARGET_RMS / speech_rms.max(f32::EPSILON)).clamp(1.0, MAX_GAIN);
    let active: Vec<bool> = levels
        .iter()
        .map(|&rms| rms > gate && rms > f32::EPSILON)
        .collect();
    let mut gains = Vec::with_capacity(levels.len());
    let mut amplified = 0;
    for (index, chunk) in samples.chunks_mut(WINDOW).enumerate() {
        let first = active[index] && (index == 0 || !active[index - 1]);
        let last = active[index] && !active.get(index + 1).copied().unwrap_or(false);
        let denominator = chunk.len().saturating_sub(1).max(1) as f32;
        let mut gain_squared = 0.0;
        for (position, sample) in chunk.iter_mut().enumerate() {
            let progress = position as f32 / denominator;
            let envelope = match (active[index], first, last) {
                (false, _, _) => 0.0,
                (true, true, true) => 1.0 - (2.0 * progress - 1.0).abs(),
                (true, true, false) => progress,
                (true, false, true) => 1.0 - progress,
                (true, false, false) => 1.0,
            };
            let gain = 1.0 + (desired - 1.0) * envelope;
            gain_squared += gain * gain;
            *sample = if gain > 1.0 {
                (*sample * gain).tanh()
            } else {
                sample.clamp(-1.0, 1.0)
            };
        }
        let applied_gain = (gain_squared / chunk.len() as f32).sqrt();
        amplified += usize::from(applied_gain > 1.01);
        gains.push(applied_gain);
    }
    gains.sort_unstable_by(f32::total_cmp);
    AgcStats {
        noise_floor,
        speech_rms,
        gain_p50: percentile(&gains, 0.5).max(1.0),
        gain_p95: percentile(&gains, 0.95).max(1.0),
        amplified_fraction: amplified as f32 / gains.len().max(1) as f32,
    }
}
