use super::fft::{self, Cx};

pub(crate) const BLOCK: usize = 256;
const FFT_LEN: usize = BLOCK * 2;
const PARTITIONS: usize = 20;
// shortcut: дрейф часов (до 100 ppm) фильтр отслеживает адаптацией с
// потерей ~5 дБ ERLE; дробная передискретизация референса по оценке
// дрейфа вернула бы статические 12+ дБ — апгрейд при живых замерах,
// где двухкристальные тракты (USB-микрофон + встроенный выход) часты.
const MU_FAST: f32 = 0.18;
const REG: f32 = 1e-3;
const COHERENCE_DEAD: f32 = 0.3;
const COHERENCE_FULL: f32 = 0.5;
const MSC_LAMBDA: f32 = 0.8;

#[derive(Debug)]
pub(crate) struct BlockStats {
    pub mic_power: f32,
    pub echo_power: f32,
    pub out_power: f32,
}

#[derive(Clone)]
pub(crate) struct EchoFilter {
    w: Vec<Cx>,
    x_ring: Vec<Cx>,
    ring_head: usize,
    p_ref: Vec<f32>,
    ref_hist: Vec<f32>,
    d_hist: Vec<f32>,
    e_hist: Vec<f32>,
    coh_c: Vec<Cx>,
    coh_pd: Vec<f32>,
    coh_px: Vec<f32>,
    work: Vec<Cx>,
    e_spec: Vec<Cx>,
    y_spec: Vec<Cx>,
    y_time: Vec<f32>,
    bg_lost: usize,
}

impl EchoFilter {
    pub(crate) fn new() -> Self {
        Self {
            w: vec![Cx::default(); PARTITIONS * FFT_LEN],
            x_ring: vec![Cx::default(); PARTITIONS * FFT_LEN],
            ring_head: 0,
            p_ref: vec![0.0; FFT_LEN],
            ref_hist: vec![0.0; FFT_LEN],
            d_hist: vec![0.0; FFT_LEN],
            e_hist: vec![0.0; FFT_LEN],
            coh_c: vec![Cx::default(); FFT_LEN],
            coh_pd: vec![0.0; FFT_LEN],
            coh_px: vec![0.0; FFT_LEN],
            work: vec![Cx::default(); FFT_LEN],
            e_spec: vec![Cx::default(); FFT_LEN],
            y_spec: vec![Cx::default(); FFT_LEN],
            y_time: vec![0.0; FFT_LEN],
            bg_lost: 0,
        }
    }

    pub(crate) fn reset_state(&mut self) {
        self.x_ring.iter_mut().for_each(|x| *x = Cx::default());
        self.ring_head = 0;
        self.p_ref.fill(0.0);
        self.ref_hist.fill(0.0);
        self.d_hist.fill(0.0);
        self.e_hist.fill(0.0);
        self.work.iter_mut().for_each(|x| *x = Cx::default());
        self.e_spec.iter_mut().for_each(|x| *x = Cx::default());
        self.y_spec.iter_mut().for_each(|x| *x = Cx::default());
        self.y_time.fill(0.0);
        self.bg_lost = 0;
    }

    pub(crate) fn process(&mut self, d: &mut [f32], ref_block: &[f32], mu_mult: f32) -> BlockStats {
        debug_assert_eq!(d.len(), BLOCK);
        debug_assert_eq!(ref_block.len(), BLOCK);
        self.ref_hist.drain(..BLOCK);
        self.ref_hist.extend_from_slice(ref_block);

        for (i, slot) in self.work.iter_mut().enumerate() {
            *slot = Cx::new(self.ref_hist[i], 0.0);
        }
        fft::fft(&mut self.work);
        let newest = self.ring_head;
        self.x_ring[newest * FFT_LEN..(newest + 1) * FFT_LEN].copy_from_slice(&self.work);
        self.ring_head = (self.ring_head + 1) % PARTITIONS;

        for k in 0..FFT_LEN {
            let mut sum = 0.0f32;
            for p in 0..PARTITIONS {
                sum += self.x_ring[p * FFT_LEN + k].norm2();
            }
            self.p_ref[k] = sum;
        }

        for slot in self.y_spec.iter_mut() {
            *slot = Cx::default();
        }
        for p in 0..PARTITIONS {
            let age = (newest + PARTITIONS - p) % PARTITIONS;
            let x_off = age * FFT_LEN;
            let w_off = p * FFT_LEN;
            for k in 0..FFT_LEN {
                let prod = self.w[w_off + k].mul(self.x_ring[x_off + k]);
                let acc = &mut self.y_spec[k];
                *acc = Cx::new(acc.re + prod.re, acc.im + prod.im);
            }
        }
        self.work.copy_from_slice(&self.y_spec);
        fft::ifft(&mut self.work);
        for (i, y) in self.y_time.iter_mut().enumerate() {
            *y = self.work[i].re;
        }

        self.d_hist.drain(..BLOCK);
        self.d_hist.extend_from_slice(d);
        self.e_hist.drain(..BLOCK);

        let mut mic_power = 0.0f32;
        let mut echo_power = 0.0f32;
        let mut see_fg = 0.0f32;
        for (i, &x) in d.iter().enumerate() {
            let y = self.y_time[FFT_LEN - BLOCK + i];
            echo_power += y * y;
            mic_power += x * x;
            see_fg += (x - y) * (x - y);
        }
        let norm = BLOCK as f32;
        mic_power /= norm;
        echo_power /= norm;
        see_fg /= norm;

        if see_fg > 2.0 * mic_power + 1e-9 {
            self.bg_lost += 1;
            if self.bg_lost > 16 {
                self.w.iter_mut().for_each(|x| *x = Cx::default());
                self.bg_lost = 0;
            }
        } else {
            self.bg_lost = 0;
        }

        if see_fg >= mic_power {
            for &x in d.iter() {
                self.e_hist.push(x);
            }
        } else {
            for (i, x) in d.iter_mut().enumerate() {
                let y = self.y_time[FFT_LEN - BLOCK + i];
                self.e_hist.push(*x - y);
                *x -= y;
            }
        }

        for (i, slot) in self.work.iter_mut().enumerate() {
            *slot = Cx::new(self.d_hist[i], 0.0);
        }
        fft::fft(&mut self.work);
        let x_off = newest * FFT_LEN;
        let (mut coh_sum, mut coh_w) = (0.0f32, 0.0f32);
        for k in 1..FFT_LEN / 2 {
            let cross = self.work[k].mul_conj(self.x_ring[x_off + k]);
            let (c, pd, px) = (&mut self.coh_c[k], &mut self.coh_pd[k], &mut self.coh_px[k]);
            *c = Cx::new(
                MSC_LAMBDA * c.re + (1.0 - MSC_LAMBDA) * cross.re,
                MSC_LAMBDA * c.im + (1.0 - MSC_LAMBDA) * cross.im,
            );
            *pd = MSC_LAMBDA * *pd + (1.0 - MSC_LAMBDA) * self.work[k].norm2();
            *px = MSC_LAMBDA * *px + (1.0 - MSC_LAMBDA) * self.x_ring[x_off + k].norm2();
            let w = *px;
            coh_sum += w * c.norm2() / (*pd * *px + 1e-6 * w * w + 1e-12);
            coh_w += w;
        }
        let coherence = if coh_w > 1e-9 { coh_sum / coh_w } else { 0.0 };

        let mu = MU_FAST
            * mu_mult
            * ((coherence - COHERENCE_DEAD) / (COHERENCE_FULL - COHERENCE_DEAD)).clamp(0.0, 1.0);
        for (i, slot) in self.work.iter_mut().enumerate() {
            *slot = Cx::new(self.e_hist[i], 0.0);
        }
        fft::fft(&mut self.work);
        self.e_spec.copy_from_slice(&self.work);
        if mu > 0.0 {
            let p_ref_max = self.p_ref.iter().cloned().fold(0.0f32, f32::max);
            let p_ref_floor = p_ref_max * 1e-4;
            for p in 0..PARTITIONS {
                let age = (newest + PARTITIONS - p) % PARTITIONS;
                let x_off = age * FFT_LEN;
                let w_off = p * FFT_LEN;
                for k in 0..FFT_LEN {
                    if self.p_ref[k] < p_ref_floor {
                        continue;
                    }
                    let grad = self.work[k].mul_conj(self.x_ring[x_off + k]);
                    let step = mu / (self.p_ref[k] + REG);
                    let cur = self.w[w_off + k];
                    self.w[w_off + k] = Cx::new(cur.re + grad.re * step, cur.im + grad.im * step);
                }
            }
            for p in 0..PARTITIONS {
                let w_off = p * FFT_LEN;
                self.work.copy_from_slice(&self.w[w_off..w_off + FFT_LEN]);
                fft::ifft(&mut self.work);
                for slot in self.work[BLOCK..].iter_mut() {
                    *slot = Cx::default();
                }
                fft::fft(&mut self.work);
                self.w[w_off..w_off + FFT_LEN].copy_from_slice(&self.work);
            }
        }

        let mut out_power: f32 = d.iter().map(|x| x * x).sum();
        out_power /= norm;
        BlockStats {
            mic_power,
            echo_power,
            out_power,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, len: usize, phase: f32) -> Vec<f32> {
        (0..len)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / 16000.0 + phase).sin())
            .collect()
    }

    #[test]
    fn converges_on_synthetic_echo() {
        let delay = 480usize;
        let len = 16000 * 6;
        let slow = tone(523.0, len, 0.0);
        let mut reference = tone(220.0, len, 0.0);
        for (i, r) in reference.iter_mut().enumerate() {
            *r += 0.6 * slow[i] * (0.5 + 0.5 * (2.0 * i as f32 / 8000.0).sin());
        }
        let mut mic = vec![0.0f32; len];
        for i in delay..len {
            mic[i] = 0.4 * reference[i - delay];
        }
        let mut filter = EchoFilter::new();
        let mut erle = 0.0f32;
        let mut counted = 0.0f32;
        let skip = (len - delay) / 2;
        for start in (0..len - delay - BLOCK).step_by(BLOCK) {
            let mut d_block = mic[start + delay..start + delay + BLOCK].to_vec();
            let r_block = reference[start..start + BLOCK].to_vec();
            let stats = filter.process(&mut d_block, &r_block, 1.0);
            if start >= skip {
                erle += stats.mic_power / (stats.out_power + 1e-12);
                counted += 1.0;
            }
        }
        let erle_db = 10.0 * (erle / counted).log10();
        assert!(erle_db > 10.0, "ERLE {erle_db} дБ — фильтр не сошёлся");
    }

    #[test]
    fn processes_uncorrelated_input_without_panic() {
        let mut filter = EchoFilter::new();
        let mut d = tone(440.0, BLOCK, 0.0);
        let r = tone(880.0, BLOCK, 0.0);
        let stats = filter.process(&mut d, &r, 1.0);
        assert!(stats.mic_power.is_finite() && stats.out_power.is_finite());
    }
}
