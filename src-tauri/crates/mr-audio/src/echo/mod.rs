mod delay;
mod fft;
mod filter;

use filter::EchoFilter;

const MAX_DELAY: usize = 8000;
const ECHO_MIN_PROMINENCE: f32 = 4.5;
const POWER_FLOOR: f32 = 1e-7;

#[derive(Debug)]
pub struct EchoReport {
    pub echo_windows: usize,
    pub erle_db: Option<f32>,
}

pub fn suppress_echo(mic: &mut [f32], reference: &[f32], sample_rate: u32) -> EchoReport {
    let n = mic.len().min(reference.len());
    if sample_rate != 16000 || n < delay::MIN_WINDOW_SAMPLES {
        return EchoReport {
            echo_windows: 0,
            erle_db: None,
        };
    }
    let mut echo_present = false;
    let window_len = 10 * 16000;
    let probe_end = n.min(180 * 16000);
    let mut w = 0usize;
    while w + delay::MIN_WINDOW_SAMPLES <= probe_end {
        let w1 = (w + window_len).min(probe_end);
        if let Some((_, prom)) = delay::estimate(&mic[w..w1], &reference[w..w1], MAX_DELAY) {
            if prom >= ECHO_MIN_PROMINENCE {
                echo_present = true;
                break;
            }
        }
        w = w1;
    }
    dc_block(mic);

    let echo_windows = usize::from(echo_present);
    let mut erle: Vec<f32> = Vec::new();

    if echo_present {
        let original = mic[..n].to_vec();
        let mic_before: f32 = original.iter().map(|x| x * x).sum();
        let mut line = DelayLine::new(reference);
        let mut filter = EchoFilter::new();
        let mut scratch = vec![0.0f32; filter::BLOCK];
        let mut t = 0usize;
        while t + filter::BLOCK <= n {
            let mut ref_block = vec![0.0f32; filter::BLOCK];
            line.pull(0, t, &mut ref_block);
            scratch.copy_from_slice(&mic[t..t + filter::BLOCK]);
            filter.process(&mut scratch, &ref_block, 1.0);
            t += filter::BLOCK;
        }
        filter.reset_state();
        let mut line = DelayLine::new(reference);
        let mut t = 0usize;
        while t + filter::BLOCK <= n {
            let mut ref_block = vec![0.0f32; filter::BLOCK];
            line.pull(0, t, &mut ref_block);
            let stats = filter.process(&mut mic[t..t + filter::BLOCK], &ref_block, 1.0);
            if stats.echo_power > POWER_FLOOR && stats.mic_power <= 2.0 * stats.echo_power {
                erle.push(10.0 * (stats.mic_power / (stats.out_power + 1e-10)).log10());
            }
            t += filter::BLOCK;
        }
        let erle_db = median(&mut erle);
        // Зачем: расходящийся фильтр без пользы калечит микрофон и роняет
        // ASR/кластеризацию — честнее оставить исходник, чем «давить» в минус.
        let mic_after: f32 = mic[..n].iter().map(|x| x * x).sum();
        let bad_erle = erle_db.map(|v| v < 1.0).unwrap_or(false);
        let bad_energy = mic_before > 0.0
            && (mic_after / mic_before > 1.5 || mic_after / mic_before < 0.25);
        if bad_erle || bad_energy {
            mic[..n].copy_from_slice(&original);
            eprintln!("эхо: подавление без пользы (ERLE={erle_db:?} дБ) — оставлен исходник");
            return EchoReport {
                echo_windows: 0,
                erle_db: None,
            };
        }
    }

    EchoReport {
        echo_windows,
        erle_db: median(&mut erle),
    }
}

fn dc_block(samples: &mut [f32]) {
    let (mut x_prev, mut y_prev) = (0.0f32, 0.0f32);
    for s in samples.iter_mut() {
        let x = *s;
        *s = x - x_prev + 0.97 * y_prev;
        x_prev = x;
        y_prev = *s;
    }
}

struct DelayLine<'a> {
    reference: &'a [f32],
    buf: Vec<f32>,
    pos: usize,
    produced: usize,
    x_prev: f32,
    y_prev: f32,
}

impl<'a> DelayLine<'a> {
    fn new(reference: &'a [f32]) -> Self {
        Self {
            reference,
            buf: vec![0.0; MAX_DELAY + filter::BLOCK],
            pos: 0,
            produced: 0,
            x_prev: 0.0,
            y_prev: 0.0,
        }
    }

    fn push_one(&mut self) {
        let s = self.reference.get(self.produced).copied().unwrap_or(0.0);
        let y = s - self.x_prev + 0.97 * self.y_prev;
        self.x_prev = s;
        self.y_prev = y;
        self.buf[self.pos] = y;
        self.pos = (self.pos + 1) % self.buf.len();
        self.produced += 1;
    }

    fn pull(&mut self, delay: usize, t: usize, out: &mut [f32]) {
        let end = t + out.len();
        while self.produced < end {
            self.push_one();
        }
        for (j, slot) in out.iter_mut().enumerate() {
            let i = (t + j).saturating_sub(delay);
            *slot = if i < self.produced {
                self.buf[i % self.buf.len()]
            } else {
                0.0
            };
        }
    }
}

fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_block_removes_offset_keeps_wave() {
        let mut s: Vec<f32> = (0..16000)
            .map(|i| 0.5 + (std::f32::consts::TAU * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        dc_block(&mut s);
        let tail_mean: f32 = s[8000..].iter().sum::<f32>() / 8000.0;
        assert!(tail_mean.abs() < 0.05, "среднее {tail_mean}");
        let amp = s[8000..].iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        assert!((amp - 1.0).abs() < 0.2, "амплитуда {amp}");
    }

    #[test]
    fn delay_line_returns_shifted_reference() {
        let reference: Vec<f32> = (0..30000).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut blocked = reference.clone();
        dc_block(&mut blocked);
        let mut line = DelayLine::new(&reference);
        let delay = 5000usize;
        let mut out = vec![0.0f32; filter::BLOCK];
        line.pull(delay, 6000, &mut out);
        for (j, &got) in out.iter().enumerate() {
            let want = blocked[6000 + j - delay];
            assert!((got - want).abs() < 1e-4, "j={j} {got} vs {want}");
        }
    }
}
