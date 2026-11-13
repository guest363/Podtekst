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

    fn push(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
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

struct AgcPlan {
    noise_floor: f32,
    speech_rms: f32,
    desired: f32,
    active: Vec<bool>,
}

fn agc_plan(levels: &[f32]) -> AgcPlan {
    let mut sorted = levels.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    let noise_floor = percentile(&sorted, 0.1);
    let speech_rms = percentile(&sorted, 0.9);
    let gate = noise_floor * 2.5;
    AgcPlan {
        noise_floor,
        speech_rms,
        desired: (TARGET_RMS / speech_rms.max(f32::EPSILON)).clamp(1.0, MAX_GAIN),
        active: levels
            .iter()
            .map(|&rms| rms > gate && rms > f32::EPSILON)
            .collect(),
    }
}

fn filtered_levels(samples: &[f32]) -> Vec<f32> {
    let mut filter = HighPassFilter::new(80.0, 16_000.0);
    samples
        .chunks(WINDOW)
        .map(|chunk| {
            let sum = chunk
                .iter()
                .map(|&x| {
                    let y = filter.push(x);
                    y * y
                })
                .sum::<f32>();
            (sum / chunk.len() as f32).sqrt()
        })
        .collect()
}

fn window_gain(plan: &AgcPlan, index: usize, position: usize, len: usize) -> f32 {
    let active = plan.active[index];
    let first = active && (index == 0 || !plan.active[index - 1]);
    let last = active && !plan.active.get(index + 1).copied().unwrap_or(false);
    let denominator = len.saturating_sub(1).max(1) as f32;
    let progress = position as f32 / denominator;
    let envelope = match (active, first, last) {
        (false, _, _) => 0.0,
        (true, true, true) => 1.0 - (2.0 * progress - 1.0).abs(),
        (true, true, false) => progress,
        (true, false, true) => 1.0 - progress,
        (true, false, false) => 1.0,
    };
    1.0 + (plan.desired - 1.0) * envelope
}

fn stats_from(plan: &AgcPlan, mut gains: Vec<f32>, amplified: usize) -> AgcStats {
    gains.sort_unstable_by(f32::total_cmp);
    AgcStats {
        noise_floor: plan.noise_floor,
        speech_rms: plan.speech_rms,
        gain_p50: percentile(&gains, 0.5).max(1.0),
        gain_p95: percentile(&gains, 0.95).max(1.0),
        amplified_fraction: amplified as f32 / gains.len().max(1) as f32,
    }
}

fn analyze(samples: &[f32]) -> (AgcPlan, AgcStats) {
    let plan = agc_plan(&filtered_levels(samples));
    let mut gains = Vec::with_capacity(plan.active.len());
    let mut amplified = 0;
    for (index, chunk) in samples.chunks(WINDOW).enumerate() {
        let mut gain_squared = 0.0;
        for position in 0..chunk.len() {
            let gain = window_gain(&plan, index, position, chunk.len());
            gain_squared += gain * gain;
        }
        let applied_gain = (gain_squared / chunk.len() as f32).sqrt();
        amplified += usize::from(applied_gain > 1.01);
        gains.push(applied_gain);
    }
    let stats = stats_from(&plan, gains, amplified);
    (plan, stats)
}

/// Преданализ AGC без изменения сэмплов: amplified_fraction == 0.0 —
/// конвейер берёт исходный канал, усиленная копия не выделяется.
pub fn agc_analysis_16k(samples: &[f32]) -> AgcStats {
    analyze(samples).1
}

pub fn agc_rms_16k(samples: &mut [f32]) -> AgcStats {
    let (plan, stats) = analyze(samples);
    let mut filter = HighPassFilter::new(80.0, 16_000.0);
    for (index, chunk) in samples.chunks_mut(WINDOW).enumerate() {
        let len = chunk.len();
        for (position, sample) in chunk.iter_mut().enumerate() {
            let filtered = filter.push(*sample);
            let gain = window_gain(&plan, index, position, len);
            *sample = if gain > 1.0 {
                (filtered * gain).tanh()
            } else {
                filtered.clamp(-1.0, 1.0)
            };
        }
    }
    stats
}
