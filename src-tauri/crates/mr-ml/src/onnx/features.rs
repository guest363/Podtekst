//! Параметры — контракты экспортированных моделей; менять только вместе с весами.
//!
//! * Whisper (librosa/slaney): 400/160/128, периодическое окно Hann, reflect-pad
//!   `center=True`, `log10`, отсечка динамики 8 дБ по глобальному максимуму окна,
//!   затем `(x + 4) / 4`.
//! * GigaAM/NeMo: 320/160/64, HTK-mel, натуральный логарифм, preemphasis 0.97.
//! * WeSpeaker/pyannote: 400/160/80, HTK-mel, натуральный логарифм, preemphasis 0.97 (кухня Kaldi-fbank).
//! * ERes2NetV2 (3D-Speaker): то же, но окно Hann и покадровая кухня kaldi
//!   (минус среднее кадра → преэмфазис внутри кадра), как `torchaudio.compliance.kaldi.fbank`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

fn hann(n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()))
        .collect()
}

fn hamming(n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![1.0; n];
    }
    (0..n)
        .map(|i| 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos())
        .collect()
}

/// Периодическое окно Hann (знаменатель `n`, как `torch.hann_window` и
/// librosa с `fftbins=True`) — эталон Whisper использует именно его.
fn hann_periodic(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()))
        .collect()
}

fn hz_to_mel_htk(hz: f64) -> f64 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz_htk(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel / 2595.0) - 1.0)
}

fn mel_filters_htk(
    sample_rate: u32,
    n_fft: usize,
    n_mels: usize,
    fmin: f64,
    fmax: f64,
) -> Vec<Vec<f64>> {
    let mel_min = hz_to_mel_htk(fmin);
    let mel_max = hz_to_mel_htk(fmax);
    let points: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz_htk(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64))
        .collect();
    let bins: Vec<f64> = points
        .iter()
        .map(|f| ((n_fft + 1) as f64 * f / sample_rate as f64).floor())
        .collect();
    let mut fb = vec![vec![0.0; n_fft / 2 + 1]; n_mels];
    for m in 1..=n_mels {
        let (f0, f1, f2) = (bins[m - 1], bins[m], bins[m + 1]);
        for k in 0..(n_fft / 2 + 1) {
            let k = k as f64;
            fb[m - 1][k as usize] = if k >= f0 && k <= f1 && (f1 - f0) > 0.0 {
                (k - f0) / (f1 - f0)
            } else if k >= f1 && k <= f2 && (f2 - f1) > 0.0 {
                (f2 - k) / (f2 - f1)
            } else {
                0.0
            };
        }
    }
    fb
}

/// Мел-банки с точной семантикой kaldi (как `torchaudio.compliance.kaldi`):
/// непрерывные треугольники в мел-домене, оценённые в центрах бинов БПФ.
/// В отличие от [`mel_filters_htk`], вершины не квантуются floor-ом по бинам —
/// на реальной речи это поднимает паритет fbank с эталоном с 0.976 до 1.000.
fn mel_filters_kaldi(
    sample_rate: u32,
    n_fft: usize,
    n_mels: usize,
    fmin: f64,
    fmax: f64,
) -> Vec<Vec<f64>> {
    let mel_lo = hz_to_mel_htk(fmin);
    let mel_hi = hz_to_mel_htk(fmax);
    let delta = (mel_hi - mel_lo) / (n_mels + 1) as f64;
    let mut fb = vec![vec![0.0; n_fft / 2 + 1]; n_mels];
    for (b, row) in fb.iter_mut().enumerate() {
        let (l, c, r) = (
            mel_lo + b as f64 * delta,
            mel_lo + (b + 1) as f64 * delta,
            mel_lo + (b + 2) as f64 * delta,
        );
        for (i, slot) in row.iter_mut().enumerate() {
            let m = hz_to_mel_htk(i as f64 * sample_rate as f64 / n_fft as f64);
            let left = (m - l) / (c - l);
            let right = (r - m) / (r - c);
            *slot = left.min(right).max(0.0);
        }
    }
    fb
}

/// Шкала Slaney/hearers (`librosa.filters.mel(htk=False)`): линейная до 1 кГц,
/// затем логарифмическая. Эталон Whisper считает банки именно в ней.
fn hz_to_mel_slaney(hz: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if hz >= MIN_LOG_HZ {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / (6.4_f64.ln() / 27.0)
    } else {
        hz / F_SP
    }
}

fn mel_to_hz_slaney(mel: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    if mel >= MIN_LOG_MEL {
        MIN_LOG_HZ * ((6.4_f64.ln() / 27.0) * (mel - MIN_LOG_MEL)).exp()
    } else {
        mel * F_SP
    }
}

/// Мел-банки librosa с `htk=False, norm="slaney"` — контракт Whisper.
/// Норма `2 / (f2 - f0)` выравнивает площадь треугольников: без неё широкие
/// верхние банки доминировали бы над узкими нижними.
fn mel_filters_slaney(
    sample_rate: u32,
    n_fft: usize,
    n_mels: usize,
    fmin: f64,
    fmax: f64,
) -> Vec<Vec<f64>> {
    let mel_lo = hz_to_mel_slaney(fmin);
    let mel_hi = hz_to_mel_slaney(fmax);
    let points: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz_slaney(mel_lo + (mel_hi - mel_lo) * i as f64 / (n_mels + 1) as f64))
        .collect();
    let bins = n_fft / 2 + 1;
    let mut fb = vec![vec![0.0; bins]; n_mels];
    for (m, row) in fb.iter_mut().enumerate() {
        let (f0, f1, f2) = (points[m], points[m + 1], points[m + 2]);
        let enorm = 2.0 / (f2 - f0);
        for (k, slot) in row.iter_mut().enumerate() {
            let freq = k as f64 * sample_rate as f64 / n_fft as f64;
            let lower = (freq - f0) / (f1 - f0);
            let upper = (f2 - freq) / (f2 - f1);
            *slot = lower.min(upper).max(0.0) * enorm;
        }
    }
    fb
}

/// reflect-padding как `torch.stft(center=True, pad_mode="reflect")`:
/// крайние сэмплы не дублируются.
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f32> {
    let n = samples.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    if n == 0 {
        out.resize(2 * pad, 0.0);
        return out;
    }
    if n <= pad {
        // Для сигнала короче pad reflect-режим не определён — повторяем край.
        out.resize(pad, samples[0]);
        out.extend_from_slice(samples);
        out.resize(pad + n + pad, samples[n - 1]);
        return out;
    }
    out.extend((1..=pad).rev().map(|i| samples[i]));
    out.extend_from_slice(samples);
    out.extend((1..=pad).map(|i| samples[n - 1 - i]));
    out
}

fn dft_into(
    src: &[(f64, f64)],
    stride: usize,
    n: usize,
    dst: &mut [(f64, f64)],
    tw: &[(f64, f64)],
    top_n: usize,
) {
    let step = top_n.is_multiple_of(n).then_some(top_n / n);
    for k in 0..n {
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for i in 0..n {
            let (xr, xi) = src[i * stride];
            let (c, s) = match step {
                Some(step) => tw[(k * i * step) % top_n],
                None => {
                    let ang = -2.0 * std::f64::consts::PI * (k * i % n) as f64 / n as f64;
                    ang.sin_cos()
                }
            };
            re += xr * c - xi * s;
            im += xr * s + xi * c;
        }
        dst[k] = (re, im);
    }
}

/// Поворотные коэффициенты зависят только от `n`: без кэша каждый кадр
/// заново считал тысячи `sin_cos` на всех уровнях рекурсии.
fn twiddles(n: usize) -> Option<&'static [(f64, f64)]> {
    static CACHE: OnceLock<HashMap<usize, Vec<(f64, f64)>>> = OnceLock::new();
    let table = CACHE.get_or_init(|| {
        let mut table = HashMap::new();
        for top in [512usize, 400, 320] {
            let mut n = top;
            while n > 1 {
                table.entry(n).or_insert_with(|| {
                    (0..n)
                        .map(|k| {
                            let ang = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
                            let (s, c) = ang.sin_cos();
                            (c, s)
                        })
                        .collect()
                });
                n = if n.is_multiple_of(2) { n / 2 } else { n / 5 };
            }
        }
        table
    });
    table.get(&n).map(Vec::as_slice)
}

#[allow(clippy::too_many_arguments)]
fn fft_rec(
    src: &[(f64, f64)],
    stride: usize,
    n: usize,
    dst: &mut [(f64, f64)],
    scratch: &mut [(f64, f64)],
    tw: &[(f64, f64)],
    top_n: usize,
) {
    if n <= 1 {
        dst[0] = src[0];
        return;
    }
    let r = if n.is_multiple_of(2) {
        2
    } else if n.is_multiple_of(5) {
        5
    } else {
        dft_into(src, stride, n, dst, tw, top_n);
        return;
    };
    let m = n / r;
    for j in 0..r {
        fft_rec(
            &src[j * stride..],
            stride * r,
            m,
            &mut dst[j * m..(j + 1) * m],
            scratch,
            tw,
            top_n,
        );
    }
    let step = top_n / n;
    for k in 0..n {
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for j in 0..r {
            let (yr, yi) = dst[j * m + k % m];
            let (c, s) = tw[(j * k * step) % top_n];
            re += yr * c - yi * s;
            im += yr * s + yi * c;
        }
        scratch[k] = (re, im);
    }
    dst[..n].copy_from_slice(&scratch[..n]);
}

fn fft_power_into(
    frame: &[f64],
    src: &mut [(f64, f64)],
    dst: &mut [(f64, f64)],
    scratch: &mut [(f64, f64)],
    power: &mut [f64],
) {
    let n = frame.len();
    let Some(tw) = twiddles(n) else {
        for (k, slot) in power.iter_mut().enumerate().take(n / 2 + 1) {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &x) in frame.iter().enumerate() {
                let ang = -2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
                let (s, c) = ang.sin_cos();
                re += x * c;
                im += x * s;
            }
            *slot = re * re + im * im;
        }
        return;
    };
    for (slot, &x) in src.iter_mut().zip(frame) {
        *slot = (x, 0.0);
    }
    fft_rec(src, 1, n, dst, scratch, tw, n);
    for k in 0..=n / 2 {
        power[k] = dst[k].0 * dst[k].0 + dst[k].1 * dst[k].1;
    }
}

#[cfg(test)]
fn fft_power(frame: &[f64]) -> Vec<f64> {
    let n = frame.len();
    let mut src = vec![(0.0, 0.0); n];
    let mut dst = vec![(0.0, 0.0); n];
    let mut scratch = vec![(0.0, 0.0); n];
    let mut power = vec![0.0; n / 2 + 1];
    fft_power_into(frame, &mut src, &mut dst, &mut scratch, &mut power);
    power
}

#[derive(Debug, Clone, Copy)]
pub struct FrontendParams {
    pub win_length: usize,
    pub fft_size: usize,
    pub hop: usize,
    pub n_mels: usize,
    pub sample_rate: u32,
    pub fmin: f64,
    pub fmax: f64,
    pub preemphasis: f32,
    pub natural_log: bool,
    /// Окно Hamming (WeSpeaker/pyannote) вместо Hann.
    pub hamming_window: bool,
    pub kaldi_exact: bool,
}

pub const WHISPER_FRONTEND: FrontendParams = FrontendParams {
    win_length: 400,
    fft_size: 400,
    hop: 160,
    // onnx-community whisper-large-v3-turbo собран под 128 мел-бинов
    n_mels: 128,
    sample_rate: 16000,
    fmin: 0.0,
    fmax: 8000.0,
    preemphasis: 0.0,
    natural_log: false,
    hamming_window: false,
    kaldi_exact: false,
};

pub const GIGAAM_FRONTEND: FrontendParams = FrontendParams {
    win_length: 320,
    fft_size: 320,
    hop: 160,
    n_mels: 64,
    sample_rate: 16000,
    fmin: 0.0,
    fmax: 8000.0,
    preemphasis: 0.97,
    natural_log: true,
    hamming_window: false,
    kaldi_exact: false,
};

pub const WESPEAKER_FRONTEND: FrontendParams = FrontendParams {
    win_length: 400,
    fft_size: 512,
    hop: 160,
    n_mels: 80,
    sample_rate: 16000,
    fmin: 20.0,
    fmax: 8000.0,
    preemphasis: 0.97,
    natural_log: true,
    hamming_window: true,
    kaldi_exact: false,
};

/// 3D-Speaker `FBank` (ERes2NetV2/CAM++): torchaudio kaldi-fbank с `window_type='hann'`,
/// преэмфазис 0.97 и remove_dc_offset покадрово, CMN поверх (`eres2netv2_fbank`).
pub const ERES2NETV2_FRONTEND: FrontendParams = FrontendParams {
    win_length: 400,
    fft_size: 512,
    hop: 160,
    n_mels: 80,
    sample_rate: 16000,
    fmin: 20.0,
    fmax: 8000.0,
    preemphasis: 0.97,
    natural_log: true,
    hamming_window: false,
    kaldi_exact: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum WindowKind {
    Hann,
    PeriodicHann,
    Hamming,
}

type WindowCache = Mutex<HashMap<(WindowKind, usize), Arc<Vec<f64>>>>;

fn cached_window(kind: WindowKind, n: usize) -> Arc<Vec<f64>> {
    static CACHE: OnceLock<WindowCache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(window) = cache.get(&(kind, n)) {
        return Arc::clone(window);
    }
    let window = match kind {
        WindowKind::Hann => hann(n),
        WindowKind::PeriodicHann => hann_periodic(n),
        WindowKind::Hamming => hamming(n),
    };
    let window = Arc::new(window);
    cache.insert((kind, n), Arc::clone(&window));
    window
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FilterKind {
    Htk,
    Kaldi,
    Slaney,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FilterKey {
    kind: FilterKind,
    sample_rate: u32,
    fft_size: usize,
    n_mels: usize,
    fmin_bits: u64,
    fmax_bits: u64,
}

type FilterCache = Mutex<HashMap<FilterKey, Arc<Vec<Vec<f64>>>>>;

fn cached_filters(p: FrontendParams, kind: FilterKind) -> Arc<Vec<Vec<f64>>> {
    static CACHE: OnceLock<FilterCache> = OnceLock::new();
    let key = FilterKey {
        kind,
        sample_rate: p.sample_rate,
        fft_size: p.fft_size,
        n_mels: p.n_mels,
        fmin_bits: p.fmin.to_bits(),
        fmax_bits: p.fmax.to_bits(),
    };
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(filters) = cache.get(&key) {
        return Arc::clone(filters);
    }
    let filters = match kind {
        FilterKind::Htk => mel_filters_htk(p.sample_rate, p.fft_size, p.n_mels, p.fmin, p.fmax),
        FilterKind::Kaldi => mel_filters_kaldi(p.sample_rate, p.fft_size, p.n_mels, p.fmin, p.fmax),
        FilterKind::Slaney => {
            mel_filters_slaney(p.sample_rate, p.fft_size, p.n_mels, p.fmin, p.fmax)
        }
    };
    let filters = Arc::new(filters);
    cache.insert(key, Arc::clone(&filters));
    filters
}

pub fn log_mel_frames(samples: &[f32], p: FrontendParams) -> Vec<Vec<f32>> {
    let mut sig: Vec<f32> = Vec::with_capacity(samples.len());
    if p.preemphasis > 0.0 && !p.kaldi_exact {
        let mut prev = 0.0f32;
        for &x in samples {
            sig.push(x - p.preemphasis * prev);
            prev = x;
        }
    } else {
        sig.extend_from_slice(samples);
    }
    let win = cached_window(
        if p.hamming_window {
            WindowKind::Hamming
        } else {
            WindowKind::Hann
        },
        p.win_length,
    );
    let fb = cached_filters(
        p,
        if p.kaldi_exact {
            FilterKind::Kaldi
        } else {
            FilterKind::Htk
        },
    );
    let n = sig.len();
    let mut frames = Vec::new();
    let mut frame = vec![0.0f64; p.fft_size];
    let mut fft_src = vec![(0.0f64, 0.0f64); p.fft_size];
    let mut fft_dst = vec![(0.0f64, 0.0f64); p.fft_size];
    let mut fft_scratch = vec![(0.0f64, 0.0f64); p.fft_size];
    let mut power = vec![0.0f64; p.fft_size / 2 + 1];
    let mut start = 0usize;
    while start + p.win_length <= n {
        frame.fill(0.0);
        if p.kaldi_exact {
            // Порядок kaldi: минус среднее кадра → преэмфазис внутри кадра → окно
            let raw = &sig[start..start + p.win_length];
            let mean = raw.iter().sum::<f32>() / p.win_length as f32;
            frame[0] = (raw[0] - mean) as f64;
            for i in 1..p.win_length {
                let cur = raw[i] - mean;
                let prev = raw[i - 1] - mean;
                frame[i] = (cur - p.preemphasis * prev) as f64;
            }
        } else {
            for i in 0..p.win_length {
                frame[i] = sig[start + i] as f64;
            }
        }
        for i in 0..p.win_length {
            frame[i] *= win[i];
        }
        fft_power_into(
            &frame,
            &mut fft_src,
            &mut fft_dst,
            &mut fft_scratch,
            &mut power,
        );
        let mut mel = vec![0f32; p.n_mels];
        for (m, row) in fb.iter().enumerate() {
            let e: f64 = row.iter().zip(power.iter()).map(|(w, q)| w * q).sum();
            mel[m] = if p.natural_log {
                (e.max(1e-9)).ln() as f32
            } else {
                (e.max(1e-10)).log10() as f32
            };
        }
        frames.push(mel);
        start += p.hop;
    }
    frames
}

/// Мел-спектрограмма Whisper с точной семантикой эталона:
/// `librosa.filters.mel(htk=False, norm="slaney")`, `torch.stft(center=True,
/// reflect)` и отбрасывание последнего кадра (`stft[..., :-1]`). Отсечка
/// динамики — по глобальному максимуму всего окна, а не по кадру.
pub fn whisper_mel(samples: &[f32]) -> Vec<Vec<f32>> {
    const NOISE_FLOOR: f32 = 0.002;
    const TARGET_PEAK: f32 = 0.9;
    let peak = samples.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    let normalized_buf: Vec<f32>;
    let samples: &[f32] = if peak > NOISE_FLOOR && (peak - TARGET_PEAK).abs() > 0.05 {
        let scale = TARGET_PEAK / peak;
        normalized_buf = samples
            .iter()
            .map(|&x| (x * scale).clamp(-1.0, 1.0))
            .collect();
        &normalized_buf
    } else {
        samples
    };

    let p = WHISPER_FRONTEND;
    let padded = reflect_pad(samples, p.win_length / 2);
    if padded.len() < p.win_length {
        return Vec::new();
    }
    let win = cached_window(WindowKind::PeriodicHann, p.win_length);
    let fb = cached_filters(p, FilterKind::Slaney);
    // center=True добавляет по half-window с каждой стороны; torch отдаёт на
    // один кадр больше, чем нужно модели, и эталон его отбрасывает.
    let n_frames = (1 + (padded.len() - p.win_length) / p.hop) - 1;
    let mut frames = Vec::with_capacity(n_frames);
    let mut global_max = f32::NEG_INFINITY;
    let mut frame = vec![0.0f64; p.win_length];
    let mut fft_src = vec![(0.0f64, 0.0f64); p.win_length];
    let mut fft_dst = vec![(0.0f64, 0.0f64); p.win_length];
    let mut fft_scratch = vec![(0.0f64, 0.0f64); p.win_length];
    let mut power = vec![0.0f64; p.win_length / 2 + 1];
    for t in 0..n_frames {
        let start = t * p.hop;
        for i in 0..p.win_length {
            frame[i] = padded[start + i] as f64 * win[i];
        }
        fft_power_into(
            &frame,
            &mut fft_src,
            &mut fft_dst,
            &mut fft_scratch,
            &mut power,
        );
        let mut mel = vec![0f32; p.n_mels];
        for (m, row) in fb.iter().enumerate() {
            let e: f64 = row.iter().zip(power.iter()).map(|(w, q)| w * q).sum();
            let v = (e.max(1e-10)).log10() as f32;
            mel[m] = v;
            global_max = global_max.max(v);
        }
        frames.push(mel);
    }
    for row in frames.iter_mut() {
        for x in row.iter_mut() {
            *x = ((*x).max(global_max - 8.0) + 4.0) / 4.0;
        }
    }
    frames
}

pub fn nemo_mel(samples: &[f32]) -> Vec<Vec<f32>> {
    const NOISE_FLOOR: f32 = 0.002;
    const TARGET_PEAK: f32 = 0.9;
    let peak = samples.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    if peak > NOISE_FLOOR && (peak - TARGET_PEAK).abs() > 0.05 {
        let scale = TARGET_PEAK / peak;
        let norm: Vec<f32> = samples
            .iter()
            .map(|&x| (x * scale).clamp(-1.0, 1.0))
            .collect();
        log_mel_frames(&norm, GIGAAM_FRONTEND)
    } else {
        log_mel_frames(samples, GIGAAM_FRONTEND)
    }
}

/// WeSpeaker/pyannote fbank [frames × 80] + глобальная CMN (минус среднее по utterance),
/// как `WeSpeakerResNet34.compute_fbank` (waveform ×2^15 схлопывается в CMN-константу).
pub fn wespeaker_fbank(samples: &[f32]) -> Vec<Vec<f32>> {
    let mut frames = log_mel_frames(samples, WESPEAKER_FRONTEND);
    cmn(&mut frames);
    frames
}

/// ERes2NetV2/CAM++ (3D-Speaker) fbank [frames × 80] + CMN: кухня
/// `torchaudio.compliance.kaldi.fbank` (hann, преэмфасис и DC-смещение покадрово)
/// и mean-нормализация из `speakerlab.process.postprocess.FBank`.
pub fn eres2netv2_fbank(samples: &[f32]) -> Vec<Vec<f32>> {
    let pcm16: Vec<f32> = samples.iter().map(|sample| sample * 32768.0).collect();
    let mut frames = log_mel_frames(&pcm16, ERES2NETV2_FRONTEND);
    cmn(&mut frames);
    frames
}

fn cmn(frames: &mut [Vec<f32>]) {
    if frames.is_empty() {
        return;
    }
    let n_bins = frames[0].len();
    let n = frames.len() as f32;
    let mut mean = vec![0.0f32; n_bins];
    for row in frames.iter() {
        for (i, &x) in row.iter().enumerate() {
            mean[i] += x;
        }
    }
    for m in mean.iter_mut() {
        *m /= n;
    }
    for row in frames.iter_mut() {
        for (i, x) in row.iter_mut().enumerate() {
            *x -= mean[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_shape_and_finite() {
        let sr = 16000;
        let tone: Vec<f32> = (0..sr)
            .map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sr as f64).sin() as f32 * 0.5)
            .collect();
        let w = whisper_mel(&tone);
        let n = nemo_mel(&tone);
        let e = wespeaker_fbank(&tone);
        let v2 = eres2netv2_fbank(&tone);
        assert_eq!(w[0].len(), 128);
        assert_eq!(n[0].len(), 64);
        assert_eq!(e[0].len(), 80);
        assert_eq!(v2[0].len(), 80);
        assert!(!w.is_empty() && !n.is_empty() && !e.is_empty() && !v2.is_empty());
        assert!(w.iter().all(|r| r.iter().all(|x| x.is_finite())));
        assert!(v2.iter().all(|r| r.iter().all(|x| x.is_finite())));
        let m: Vec<f32> = (0..v2[0].len())
            .map(|b| v2.iter().map(|r| r[b]).sum::<f32>() / v2.len() as f32)
            .collect();
        assert!(m.iter().all(|x| x.abs() < 1e-4));
        let sil = whisper_mel(&vec![0.0; sr]);
        let tone_e: f32 = w.iter().map(|r| r.iter().sum::<f32>()).sum();
        let sil_e: f32 = sil.iter().map(|r| r.iter().sum::<f32>()).sum();
        assert!(tone_e > sil_e);
    }

    #[test]
    fn frontend_tables_are_cached() {
        let first = cached_filters(ERES2NETV2_FRONTEND, FilterKind::Kaldi);
        let second = cached_filters(ERES2NETV2_FRONTEND, FilterKind::Kaldi);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.len(), ERES2NETV2_FRONTEND.n_mels);
        let window = cached_window(WindowKind::Hann, 400);
        assert_eq!(window.len(), 400);
    }

    /// 30 с при hop 160 и center=True дают ровно 3000 кадров, как у эталона.
    #[test]
    fn whisper_mel_reference_frame_count_and_finite() {
        let frames = whisper_mel(&vec![0.0f32; 16_000 * 30]);
        assert_eq!(frames.len(), 3000);
        assert!(frames.iter().all(|r| r.len() == 128));
        assert!(frames.iter().all(|r| r.iter().all(|x| x.is_finite())));
        // Нулевая мощность упирается в log10(1e-10) = -10, после (x + 4) / 4
        // это ровно -1.5 — жёсткая граница снизу для тишины.
        let max = frames
            .iter()
            .flatten()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((max + 1.5).abs() < 1e-5, "тишина дала max {max}");
        // Более короткий сигнал не должен раздувать число кадров.
        assert_eq!(whisper_mel(&vec![0.0f32; 16_000]).len(), 100);
    }

    /// Отсечка динамики глобальная: тихие кадры не поднимаются выше общего
    /// пика минус 8 дБ (2.0 после деления на 4), в отличие от покадровой.
    #[test]
    fn whisper_mel_clamps_by_global_max() {
        let sr = 16_000usize;
        let mut signal = vec![0.0f32; sr * 2];
        for (i, x) in signal.iter_mut().take(sr / 2).enumerate() {
            *x = (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sr as f64).sin() as f32;
        }
        let frames = whisper_mel(&signal);
        let max = frames
            .iter()
            .flatten()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let min = frames
            .iter()
            .flatten()
            .copied()
            .fold(f32::INFINITY, f32::min);
        assert!(max.is_finite());
        // Тишина упирается ровно в global_max - 8 дБ, то есть в max - 2.0
        // после `(x + 4) / 4`; при покадровой отсечке она была бы выше.
        assert!((min - (max - 2.0)).abs() < 1e-4, "min {min}, max {max}");
    }

    #[test]
    fn degenerate_windows_have_no_nan() {
        assert!(hann(0).is_empty());
        assert_eq!(hann(1), vec![1.0]);
        assert_eq!(hamming(0), Vec::<f64>::new());
        assert_eq!(hamming(1), vec![1.0]);
    }

    /// БПФ должно совпадать с прямым ДПФ для всех используемых размеров кадра.
    #[test]
    fn fft_matches_direct_dft() {
        let reference = |frame: &[f64]| -> Vec<f64> {
            let n = frame.len();
            (0..=n / 2)
                .map(|k| {
                    let (mut re, mut im) = (0.0f64, 0.0f64);
                    for (i, &x) in frame.iter().enumerate() {
                        let ang = 2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
                        re += x * ang.cos();
                        im -= x * ang.sin();
                    }
                    re * re + im * im
                })
                .collect()
        };
        for n in [320usize, 400, 512] {
            let frame: Vec<f64> = (0..n)
                .map(|i| {
                    let t = i as f64;
                    (t * 0.11).sin() * 0.4 + (t * 0.37).cos() * 0.2 + (t * 1.7).sin() * 0.05
                })
                .collect();
            let fast = fft_power(&frame);
            let slow = reference(&frame);
            assert_eq!(fast.len(), slow.len());
            for (k, (a, b)) in fast.iter().zip(slow.iter()).enumerate() {
                assert!(
                    (a - b).abs() <= 1e-6 * (1.0 + b.abs()),
                    "fft_size {n}, бин {k}: {a} != {b}"
                );
            }
        }
    }
}
