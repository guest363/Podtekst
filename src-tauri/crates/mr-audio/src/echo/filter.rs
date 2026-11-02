//! Частотный адаптивный фильтр эхопути (PBFDAF) с двухпутевой схемой
//! (two-path, как в Speex MDF).
//!
//! Фоновый фильтр адаптируется всегда; к сигналу применяется форграунд,
//! который перенимает фон только при устойчиво меньшем остатке. Это
//! защищает выход от расходимости адаптации на двойном разговоре:
//! случайное блуждание фона до сигнала не доходит по построению.
//! Нелинейное подавление остатка не применяется: винеровский гейн на
//! прямоугольном кадре размазывает энергию между блоками, а хвост эха
//! добивается длиной фильтра; к замерам на живых записях вернуться.

use super::fft::{self, Cx};

/// Блок обработки: 16 мс на 16 кГц.
pub(crate) const BLOCK: usize = 256;
const FFT_LEN: usize = BLOCK * 2;
/// Партиции по 16 мс: 20 × 16 = 320 мс хвоста эхопути (RT60 офиса 0.3–0.5 с).
const PARTITIONS: usize = 20;
// shortcut: дрейф часов (до 100 ppm) фильтр отслеживает адаптацией с
// потерей ~5 дБ ERLE; дробная передискретизация референса по оценке
// дрейфа вернула бы статические 12+ дБ — апгрейд при живых замерах,
// где двухкристальные тракты (USB-микрофон + встроенный выход) часты.
const MU_FAST: f32 = 0.18;
const REG: f32 = 1e-3;
/// Когерентность микрофон↔референс: шаг адаптации фона полон от ~0.5,
/// ниже ~0.25 блок считается двойным разговором (пол гейна NEAR).
/// Мёртвая зона шага: ниже этой когерентности (тишина референса, шум
/// дыхания) фон не адаптируется вовсе — иначе он за секунды «подгоняется»
/// под взаимный шум каналов, и этот мусор потом взрывается на громком эхе.
const COHERENCE_DEAD: f32 = 0.3;
/// Когерентность микрофон↔референс, от которой шаг адаптации фона полон.
const COHERENCE_FULL: f32 = 0.5;
/// Сглаживание MSC по кадрам: 5 блоков = 80 мс реакции на двойной разговор.
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

    /// Обрабатывает один блок: d на входе — микрофон, на выходе — очищенный
    /// сигнал; ref_block — уже задержанный на оценённую задержку референс.
    /// `mu_mult` — вес адаптации этого блока: 1.0 на блоках «только дальний»,
    /// ~0.02 на остальных (смещение от ближней речи падает в 50 раз,
    /// ремонт на чистых блоках выигрывает столько же).
    pub(crate) fn process(&mut self, d: &mut [f32], ref_block: &[f32], mu_mult: f32) -> BlockStats {
        debug_assert_eq!(d.len(), BLOCK);
        debug_assert_eq!(ref_block.len(), BLOCK);
        self.ref_hist.drain(..BLOCK);
        self.ref_hist.extend_from_slice(ref_block);

        // Анализ референса: окно из FFT_LEN последних сэмплов.
        for (i, slot) in self.work.iter_mut().enumerate() {
            *slot = Cx::new(self.ref_hist[i], 0.0);
        }
        fft::fft(&mut self.work);
        let newest = self.ring_head;
        self.x_ring[newest * FFT_LEN..(newest + 1) * FFT_LEN].copy_from_slice(&self.work);
        self.ring_head = (self.ring_head + 1) % PARTITIONS;

        // NLMS-нормировка суммой квадратов по всем партициям (энергия
        // полного входного вектора, Soo & Pang): суммарный шаг по бину
        // равен ровно μ, схема устойчива.
        for k in 0..FFT_LEN {
            let mut sum = 0.0f32;
            for p in 0..PARTITIONS {
                sum += self.x_ring[p * FFT_LEN + k].norm2();
            }
            self.p_ref[k] = sum;
        }

        // Оценка эха: свёртка фильтра с историей референса.
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
        // Кадр ошибки для адаптации: [прошлый блок ошибки | новый блок].
        self.e_hist.drain(..BLOCK);

        let mut mic_power = 0.0f32;
        let mut echo_power = 0.0f32;
        let mut see_fg = 0.0f32;
        for (i, x) in d.iter_mut().enumerate() {
            let y = self.y_time[FFT_LEN - BLOCK + i];
            echo_power += y * y;
            mic_power += *x * *x;
            see_fg += (*x - y) * (*x - y);
            self.e_hist.push(*x - y);
            *x -= y;
        }
        let norm = BLOCK as f32;
        mic_power /= norm;
        echo_power /= norm;
        see_fg /= norm;

        // Потерянный фильтр: если остаток катастрофически хуже сигнала
        // (блуждание после аномалии), обнуляем и переучиваемся с нуля —
        // сходится за секунды, как из старта.
        if see_fg > 20.0 * mic_power + 1e-9 {
            self.bg_lost += 1;
            if self.bg_lost > 62 {
                self.w.iter_mut().for_each(|x| *x = Cx::default());
                self.bg_lost = 0;
            }
        } else {
            self.bg_lost = 0;
        }

        // DTD по пер-бинной когерентности (MSC), сглаженной по кадрам:
        // эхо когерентно с референсом в каждом бине, ближняя речь — нет.
        // MSC не зависит от состояния фильтров, поэтому не создаёт deadlock.
        // Фазы бинов стабильны между кадрами (оба окна скользят синхронно),
        // поэтому накопление ведётся по бинам, а не скалярно по кадру.
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
            // Вес бина — энергия референса: в тихих бинах отношение pd·px
            // сравнимо с eps и MSC там — чистый шум, захламляющий среднее.
            let w = *px;
            coh_sum += w * c.norm2() / (*pd * *px + 1e-6 * w * w + 1e-12);
            coh_w += w;
        }
        let coherence = if coh_w > 1e-9 { coh_sum / coh_w } else { 0.0 };

        // Адаптация фона: шаг полон на когерентном (эхо) сигнале и гаснет
        // на двойном разговоре; блуждание фона до выхода не доходит, а
        // чанки в вызывающем коде не дают ему накапливаться.
        let mu = MU_FAST
            * mu_mult
            * ((coherence - COHERENCE_DEAD) / (COHERENCE_FULL - COHERENCE_DEAD)).clamp(0.0, 1.0);
        for (i, slot) in self.work.iter_mut().enumerate() {
            *slot = Cx::new(self.e_hist[i], 0.0);
        }
        fft::fft(&mut self.work);
        self.e_spec.copy_from_slice(&self.work);
        if mu > 0.0 {
            // Бины с пренебрежимой мощностью референса не адаптируются:
            // после DC-блокировки их шаг μ/REG в сотни раз больше речевых,
            // и низкочастотная энергия ближней речи разгоняет их до взрыва.
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
            // Градиентный контраст по всем партициям (как в float-пути Speex
            // MDF): каждая партиция отвечает только за свои B тапов, вторая
            // половина отклика — циклическая часть свёртки и обнуляется.
            // Без контраста частотный LMS на окрашенном сигнале расходится.
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
        // Эхопуть: один тап 0.4 с задержкой 480 сэмплов (30 мс). Референс —
        // нестационарная смесь тонов, как речь. Фильтр сам не выравнивает
        // задержку — подаём уже совмещённые сигналы.
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
