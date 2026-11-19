//! Общий потоковый писатель сырого аудио и измеритель уровня.
//!
//! Промежуточный формат — моно f32 little-endian: файл дописывается
//! непрерывно, поэтому жёсткая остановка не портит данные (в отличие от
//! WAV-контейнера с финализируемым заголовком). Итоговую дорожку
//! микрофона и системы собираем в стерео WAV при остановке записи.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct LevelMeter {
    rms_milli: AtomicU32,
    peak_milli: AtomicU32,
}

impl LevelMeter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Уровень 0..1 с запасом ×3.
    pub fn level_01(&self) -> f32 {
        let rms = self.rms_milli.load(Ordering::Relaxed) as f32 / 1000.0;
        (rms * 3.0).min(1.0)
    }

    /// Пик 0..1 — детектор «записана тишина».
    pub fn peak_01(&self) -> f32 {
        self.peak_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn update(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut sum = 0.0f64;
        let mut peak = 0.0f32;
        for &s in samples {
            sum += (s as f64) * (s as f64);
            if s.abs() > peak {
                peak = s.abs();
            }
        }
        let rms = (sum / samples.len() as f64).sqrt() as f32;
        self.rms_milli
            .store((rms * 1000.0) as u32, Ordering::Relaxed);
        self.peak_milli
            .fetch_max((peak * 1000.0) as u32, Ordering::Relaxed);
    }
}

pub struct RawWriter {
    inner: BufWriter<File>,
    /// Переиспользуемый байтовый буфер: запись идёт ~64 раза в секунду.
    buf: Vec<u8>,
    path: PathBuf,
}

impl RawWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        Ok(Self {
            inner: BufWriter::new(file),
            buf: Vec::new(),
            path: path.to_path_buf(),
        })
    }

    pub fn write_samples(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.buf.clear();
        self.buf.reserve(samples.len() * 4);
        for &s in samples {
            self.buf.extend_from_slice(&s.to_le_bytes());
        }
        self.inner.write_all(&self.buf)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn read_raw_f32(path: &Path) -> std::io::Result<Vec<f32>> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let (chunks, _) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

// ScreenCaptureKit отдаёт f32 LE; неполный хвост отбрасываем, чтобы не читать мусор.
pub fn f32le_to_samples(bytes: &[u8], out: &mut Vec<f32>) {
    let (chunks, _) = bytes.as_chunks::<4>();
    out.extend(chunks.iter().map(|c| f32::from_le_bytes(*c)));
}

pub fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<2>();
    chunks
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}

const SINC_HALF_LOBES: usize = 4;
const SINC_SUBSTEPS: usize = 256;
const SINC_TABLE_LEN: usize = SINC_HALF_LOBES * SINC_SUBSTEPS + 1;

fn sinc_hann_table() -> &'static [f32; SINC_TABLE_LEN] {
    static TABLE: std::sync::OnceLock<[f32; SINC_TABLE_LEN]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0f32; SINC_TABLE_LEN];
        table[0] = 1.0;
        for (k, slot) in table.iter_mut().enumerate().skip(1) {
            let u = k as f64 / SINC_SUBSTEPS as f64;
            let sinc = (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u);
            let window = 0.5 * (1.0 + (std::f64::consts::PI * u / SINC_HALF_LOBES as f64).cos());
            *slot = (sinc * window) as f32;
        }
        table
    })
}

pub fn resample_to(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if samples.is_empty() || from == to || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let target_len = (samples.len() as u64 * to as u64 / from as u64).max(1) as usize;
    let step = samples.len() as f64 / target_len as f64;
    let mut out = Vec::with_capacity(target_len);
    if from > to {
        // Понижение частоты без ФНЧ заворачивает полосу выше Nyquist в речевую.
        let table = sinc_hann_table();
        let cutoff = 0.9 * (to as f64 / from as f64);
        let radius = (SINC_HALF_LOBES as f64 / cutoff).ceil() as isize;
        let last = samples.len() as isize - 1;
        for i in 0..target_len {
            let center = i as f64 * step;
            let lo = (center.floor() as isize - radius).max(0) as usize;
            let hi = (center.ceil() as isize + radius).min(last) as usize;
            let mut acc = 0.0f32;
            let mut wsum = 0.0f32;
            for (j, &sample) in samples.iter().enumerate().take(hi + 1).skip(lo) {
                let u = (j as f64 - center).abs() * cutoff;
                let scaled = u * SINC_SUBSTEPS as f64;
                let idx = scaled as usize;
                if idx + 1 >= SINC_TABLE_LEN {
                    continue;
                }
                let frac = (scaled - idx as f64) as f32;
                let w = table[idx] + (table[idx + 1] - table[idx]) * frac;
                acc += sample * w;
                wsum += w;
            }
            out.push(if wsum.abs() > 1e-6 { acc / wsum } else { 0.0 });
        }
        return out;
    }
    for i in 0..target_len {
        let pos = i as f64 * step;
        let lo = pos.floor() as usize;
        let hi = (lo + 1).min(samples.len() - 1);
        let frac = (pos - lo as f64) as f32;
        out.push(samples[lo] * (1.0 - frac) + samples[hi] * frac);
    }
    out
}

/// Потоковый ресемплер для живого захвата: устройство отдаёт 44.1/48 кГц,
/// моделям нужно 16 кГц. Усреднение по окну подавляет высокие частоты перед
/// прореживанием, дробный шаг не даёт накопить дрейф за час записи.
pub struct StreamingResampler {
    step: f64,
    pos: f64,
    pending: Vec<f32>,
}

impl StreamingResampler {
    pub fn new(from: u32, to: u32) -> Option<Self> {
        (from != to && from > 0 && to > 0).then(|| Self {
            step: from as f64 / to as f64,
            pos: 0.0,
            pending: Vec::new(),
        })
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(input);
        let mut out = Vec::with_capacity((self.pending.len() as f64 / self.step) as usize + 2);
        while self.pos + self.step <= self.pending.len() as f64 {
            let start = self.pos.floor() as usize;
            let end = ((self.pos + self.step).ceil() as usize).min(self.pending.len());
            if end <= start {
                break;
            }
            if self.step > 1.5 {
                let sum: f32 = self.pending[start..end].iter().sum();
                out.push(sum / (end - start) as f32);
            } else {
                // Одного сэмпла мало для линейной интерполяции: хвост ждёт
                // следующего блока, иначе индекс pending[1] паникует.
                if self.pending.len() < 2 {
                    break;
                }
                let i = start.min(self.pending.len() - 2);
                // Шаг меньше сэмпла выводит окно за последнюю пару: frac > 1 —
                // это экстраполяция, её прижимаем к правому сэмплу.
                let frac = (self.pos - i as f64).clamp(0.0, 1.0) as f32;
                out.push(self.pending[i] * (1.0 - frac) + self.pending[i + 1] * frac);
            }
            self.pos += self.step;
        }
        // Оставляем дробный хвост и один сэмпл контекста для следующего блока.
        let consumed = self.pos.floor() as usize;
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mr-stream-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("t.f32");
        let mut w = RawWriter::create(&p).unwrap();
        w.write_samples(&[0.25, -0.5]).unwrap();
        w.flush().unwrap();
        let back = read_raw_f32(&p).unwrap();
        assert_eq!(back.len(), 2);
        assert!((back[0] - 0.25).abs() < 1e-6);
        assert!((back[1] + 0.5).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_meter_tracks_rms() {
        let m = LevelMeter::new();
        m.update(&[0.5, -0.5]);
        assert!(m.level_01() > 0.99);
        assert!((m.peak_01() - 0.5).abs() < 0.01);
    }

    #[test]
    fn s16_conversion() {
        let bytes = vec![0x00, 0x40, 0x00, 0xC0];
        let f = s16le_to_f32(&bytes);
        assert_eq!(f.len(), 2);
        assert!((f[0] - 0.5).abs() < 0.01 && (f[1] + 0.5).abs() < 0.01);
    }

    #[test]
    fn f32le_to_samples_ignores_incomplete_tail() {
        let bytes = [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0, 0xaa];
        let mut out = Vec::new();
        f32le_to_samples(&bytes, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!((out[1] + 2.0).abs() < 1e-6);
    }

    #[test]
    fn resample_48k_to_16k_and_zero_guard() {
        let s: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_to(&s, 48000, 16000);
        assert_eq!(out.len(), 1600);
        // Нулевые частоты (битый заголовок) не паникуют — возвращаем как есть.
        assert_eq!(resample_to(&s, 0, 16000).len(), s.len());
        assert_eq!(resample_to(&s, 48000, 0).len(), s.len());
    }

    #[test]
    fn resample_downsampling_attenuates_aliasing_above_nyquist() {
        let high: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 12_000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let down_high = resample_to(&high, 48_000, 16_000);
        let mid_high = &down_high[200..down_high.len() - 200];
        let peak_high = mid_high.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(peak_high < 0.03, "алиасинг 12 кГц не подавлен: {peak_high}");

        let voice: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 1_000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let down_voice = resample_to(&voice, 48_000, 16_000);
        let mid_voice = &down_voice[200..down_voice.len() - 200];
        let peak_voice = mid_voice.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(
            peak_voice > 0.95,
            "полезный тон 1 кГц ослаблен: {peak_voice}"
        );
    }

    #[test]
    fn streaming_resampler_48k_to_16k_keeps_duration_and_level() {
        let mut resampler = StreamingResampler::new(48_000, 16_000).expect("48k→16k");
        let mut out = Vec::new();
        for _ in 0..100 {
            out.extend(resampler.push(&vec![0.5f32; 480]));
        }
        assert!((out.len() as i64 - 16_000).abs() <= 3, "{}", out.len());
        assert!(out.iter().all(|x| (*x - 0.5).abs() < 1e-6));
    }

    #[test]
    fn streaming_resampler_44k1_to_16k_keeps_duration() {
        let mut resampler = StreamingResampler::new(44_100, 16_000).expect("44.1k→16k");
        let mut out = Vec::new();
        for _ in 0..100 {
            out.extend(resampler.push(&vec![0.25f32; 441]));
        }
        assert!((out.len() as i64 - 16_000).abs() <= 3, "{}", out.len());
    }

    #[test]
    fn streaming_resampler_is_monotonic_across_blocks() {
        let mut resampler = StreamingResampler::new(48_000, 16_000).expect("48k→16k");
        let mut out = Vec::new();
        for block in 0..100 {
            let start = block * 480;
            let samples: Vec<f32> = (0..480).map(|i| (start + i) as f32).collect();
            out.extend(resampler.push(&samples));
        }
        assert!(out.windows(2).all(|w| w[0] <= w[1] + 1e-3));
        assert!(out.first().copied().unwrap_or(0.0) < 2.0);
    }

    #[test]
    fn streaming_resampler_8k_to_16k_survives_one_sample_blocks() {
        let mut resampler = StreamingResampler::new(8_000, 16_000).expect("8k→16k");
        let mut out = Vec::new();
        for i in 0..100 {
            out.extend(resampler.push(&[i as f32 / 100.0]));
        }
        // Одиночный сэмпл не паникует и не теряется: 100 входов дают ~200 выходов.
        assert_eq!(out.len(), 200);
        // Экстраполяция (frac > 1) вывела бы значение за диапазон входов.
        assert!(out.iter().all(|&x| (0.0..=0.99 + 1e-6).contains(&x)));
    }
}
