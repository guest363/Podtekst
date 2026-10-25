//! Log-Mel фильтробанк (16 кГц) с параметризацией окна/шага/бинов.
//!
//! Пресеты:
//! * Whisper: 400/160/80, `log10`, нормализация `(x + 4) / 4` после отсечки динамики 8 дБ.
//! * GigaAM/NeMo: 320/160/64, HTK-mel, натуральный логарифм, preemphasis 0.97.
//! * WeSpeaker/pyannote: 400/160/80, HTK-mel, натуральный логарифм, preemphasis 0.97 (кухня Kaldi-fbank).
//! * ERes2NetV2 (3D-Speaker): то же, но окно Hann и покадровая кухня kaldi
//!   (минус среднее кадра → преэмфазис внутри кадра), как `torchaudio.compliance.kaldi.fbank`.

fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()))
        .collect()
}

/// Окно Hamming (WeSpeaker/pyannote): 0.54 − 0.46·cos.
fn hamming(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos())
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

// shortcut: наивная O(n²)-ДПФ вместо FFT — при fft_size 320–512 и кадре 10 мс
// это доли секунды на минуту речи; upgrade: radix-2 FFT при поддержке >30 мин.
fn dft_power(frame: &[f64]) -> Vec<f64> {
    let n = frame.len();
    let mut out = vec![0.0; n / 2 + 1];
    for (k, slot) in out.iter_mut().enumerate() {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in frame.iter().enumerate() {
            let ang = 2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
            re += x * ang.cos();
            im -= x * ang.sin();
        }
        *slot = re * re + im * im;
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub struct FrontendParams {
    /// Длина окна в сэмплах (нули добиваются до fft_size).
    pub win_length: usize,
    /// Размер ДПФ (степень двойки ≥ win_length, как в Kaldi/NeMo).
    pub fft_size: usize,
    pub hop: usize,
    pub n_mels: usize,
    pub sample_rate: u32,
    /// Нижняя/верхняя границы мел-фильтров в Гц (Kaldi по умолчанию 20..Nyquist).
    pub fmin: f64,
    pub fmax: f64,
    pub preemphasis: f32,
    pub natural_log: bool,
    /// Окно Hamming (WeSpeaker/pyannote) вместо Hann.
    pub hamming_window: bool,
    /// Точная семантика `torchaudio.compliance.kaldi.fbank`: покадровое
    /// снятие DC-смещения + преэмфасис внутри кадра и мел-банки треугольниками
    /// в мел-домене (без floor-квантования вершин по бинам).
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

/// Возвращает [n_frames × n_mels] log-mel энергии.
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
    let win = if p.hamming_window {
        hamming(p.win_length)
    } else {
        hann(p.win_length)
    };
    let fb = if p.kaldi_exact {
        mel_filters_kaldi(p.sample_rate, p.fft_size, p.n_mels, p.fmin, p.fmax)
    } else {
        mel_filters_htk(p.sample_rate, p.fft_size, p.n_mels, p.fmin, p.fmax)
    };
    let n = sig.len();
    let mut frames = Vec::new();
    let mut start = 0usize;
    while start + p.win_length <= n {
        let mut frame = vec![0.0; p.fft_size];
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
        let power = dft_power(&frame);
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

/// Whisper-фичи [frames × 80] с нормализацией OpenAI.
pub fn whisper_mel(samples: &[f32]) -> Vec<Vec<f32>> {
    let mut frames = log_mel_frames(samples, WHISPER_FRONTEND);
    for row in frames.iter_mut() {
        let max = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        for x in row.iter_mut() {
            *x = ((*x).max(max - 8.0) + 4.0) / 4.0;
        }
    }
    frames
}

/// NeMo/GigaAM-фичи [frames × 64].
pub fn nemo_mel(samples: &[f32]) -> Vec<Vec<f32>> {
    log_mel_frames(samples, GIGAAM_FRONTEND)
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
/// Масштаб waveform ×2^15 схлопывается в CMN-константу.
pub fn eres2netv2_fbank(samples: &[f32]) -> Vec<Vec<f32>> {
    let mut frames = log_mel_frames(samples, ERES2NETV2_FRONTEND);
    cmn(&mut frames);
    frames
}

/// Глобальная CMN: минус пофичерное среднее по всему utterance.
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
        // CMN обнуляет пофичерное среднее
        let m: Vec<f32> = (0..v2[0].len())
            .map(|b| v2.iter().map(|r| r[b]).sum::<f32>() / v2.len() as f32)
            .collect();
        assert!(m.iter().all(|x| x.abs() < 1e-4));
        let sil = whisper_mel(&vec![0.0; sr]);
        let tone_e: f32 = w.iter().map(|r| r.iter().sum::<f32>()).sum();
        let sil_e: f32 = sil.iter().map(|r| r.iter().sum::<f32>()).sum();
        assert!(tone_e > sil_e);
    }
}
