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
const BLOCK_FRAMES: usize = 200;
pub const TARGET_RMS: f32 = 0.1;
pub const MAX_GAIN: f32 = 100.0;

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    sorted
        .get(((sorted.len().saturating_sub(1)) as f32 * fraction).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

pub struct AgcPlan {
    noise_floor: f32,
    speech_rms: f32,
    desired: Vec<f32>,
    active: Vec<bool>,
}

fn agc_plan(levels: &[f32]) -> AgcPlan {
    let mut sorted = levels.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    let noise_floor = percentile(&sorted, 0.1);
    let speech_rms = percentile(&sorted, 0.9);
    if levels.len() <= BLOCK_FRAMES {
        let gate = noise_floor * 2.5;
        let desired = (TARGET_RMS / speech_rms.max(f32::EPSILON)).clamp(1.0, MAX_GAIN);
        return AgcPlan {
            noise_floor,
            speech_rms,
            desired: vec![desired; levels.len()],
            active: levels
                .iter()
                .map(|&rms| rms > gate && rms > f32::EPSILON)
                .collect(),
        };
    }

    // ЗАЧЕМ: на длинных записях громкий участник в начале завышает глобальный p90
    // и не даёт поднять тихую речь в хвосте — при отклонении > 20% от фона берём локальную цель.
    let global_desired = (TARGET_RMS / speech_rms.max(f32::EPSILON)).clamp(1.0, MAX_GAIN);
    let global_floor = if noise_floor > 1e-6 {
        noise_floor
    } else {
        let mut sorted_nz: Vec<f32> = levels.iter().copied().filter(|&x| x > 1e-6).collect();
        sorted_nz.sort_unstable_by(f32::total_cmp);
        if sorted_nz.is_empty() {
            0.0
        } else {
            percentile(&sorted_nz, 0.1)
        }
    };

    let mut blocks: Vec<(usize, usize)> = (0..levels.len())
        .step_by(BLOCK_FRAMES)
        .map(|s| (s, (s + BLOCK_FRAMES).min(levels.len())))
        .collect();
    if blocks.len() >= 2 {
        let last = blocks[blocks.len() - 1];
        if last.1 - last.0 < BLOCK_FRAMES / 4 {
            blocks.pop();
            if let Some(prev) = blocks.last_mut() {
                prev.1 = levels.len();
            }
        }
    }

    let mut active = vec![false; levels.len()];
    let mut anchors: Vec<(f32, f32)> = Vec::new();
    for (s, e) in blocks {
        let mut bsorted = levels[s..e].to_vec();
        bsorted.sort_unstable_by(f32::total_cmp);
        let bp10 = percentile(&bsorted, 0.1);
        let bp90 = percentile(&bsorted, 0.9);
        let bnoise = if global_floor > 0.0 && bp90 > global_floor * 2.5 {
            bp10.min(global_floor)
        } else {
            bp10
        };
        let bgate = bnoise * 2.5;
        for i in s..e {
            active[i] = levels[i] > bgate && levels[i] > f32::EPSILON;
        }
        if bp90 > bgate && bp90 > f32::EPSILON {
            let bdesired = if (bp90 - speech_rms).abs() <= speech_rms * 0.20 {
                global_desired
            } else {
                (TARGET_RMS / bp90).clamp(1.0, MAX_GAIN)
            };
            anchors.push(((s + e) as f32 * 0.5, bdesired));
        }
    }

    let desired = if anchors.is_empty() {
        vec![1.0; levels.len()]
    } else {
        (0..levels.len())
            .map(|i| {
                let pos = i as f32 + 0.5;
                if pos <= anchors[0].0 {
                    anchors[0].1
                } else if pos >= anchors[anchors.len() - 1].0 {
                    anchors[anchors.len() - 1].1
                } else {
                    let k = anchors.partition_point(|&(c, _)| c < pos).saturating_sub(1);
                    let (c0, d0) = anchors[k];
                    let (c1, d1) = anchors[k + 1];
                    let t = ((pos - c0) / (c1 - c0).max(1.0)).clamp(0.0, 1.0);
                    d0 + (d1 - d0) * t
                }
            })
            .collect()
    };

    AgcPlan {
        noise_floor,
        speech_rms,
        desired,
        active,
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
    1.0 + (plan.desired[index] - 1.0) * envelope
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
pub fn agc_analysis_16k(samples: &[f32]) -> (AgcPlan, AgcStats) {
    analyze(samples)
}

pub fn agc_apply_16k(samples: &mut [f32], plan: &AgcPlan) {
    let mut filter = HighPassFilter::new(80.0, 16_000.0);
    for (index, chunk) in samples.chunks_mut(WINDOW).enumerate() {
        let len = chunk.len();
        for (position, sample) in chunk.iter_mut().enumerate() {
            let filtered = filter.push(*sample);
            let gain = window_gain(plan, index, position, len);
            *sample = if gain > 1.0 {
                (filtered * gain).tanh()
            } else {
                filtered.clamp(-1.0, 1.0)
            };
        }
    }
}

pub fn agc_rms_16k(samples: &mut [f32]) -> AgcStats {
    let (plan, stats) = analyze(samples);
    agc_apply_16k(samples, &plan);
    stats
}
