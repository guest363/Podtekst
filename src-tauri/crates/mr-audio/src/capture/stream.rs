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

/// Измеритель уровня звука: RMS и пик в тысячных долях.
#[derive(Debug, Default)]
pub struct LevelMeter {
    rms_milli: AtomicU32,
    peak_milli: AtomicU32,
}

impl LevelMeter {
    /// Создать общий измеритель для потока захвата.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Уровень 0..1 с запасом ×3 (совпадает со старой шкалой `tail_level_01`).
    pub fn level_01(&self) -> f32 {
        let rms = self.rms_milli.load(Ordering::Relaxed) as f32 / 1000.0;
        (rms * 3.0).min(1.0)
    }

    /// Пик 0..1 — детектор «записана тишина».
    pub fn peak_01(&self) -> f32 {
        self.peak_milli.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// Пересчитать уровень по очередному блоку сэмплов.
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

/// Потоковый писатель сырых сэмплов (моно f32 little-endian).
pub struct RawWriter {
    inner: BufWriter<File>,
    /// Переиспользуемый байтовый буфер: запись идёт ~64 раза в секунду.
    buf: Vec<u8>,
    path: PathBuf,
}

impl RawWriter {
    /// Создать (или обрезать) файл записи.
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

    /// Дописать блок сэмплов.
    pub fn write_samples(&mut self, samples: &[f32]) -> std::io::Result<()> {
        self.buf.clear();
        self.buf.reserve(samples.len() * 4);
        for &s in samples {
            self.buf.extend_from_slice(&s.to_le_bytes());
        }
        self.inner.write_all(&self.buf)
    }

    /// Сбросить буфер на диск.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }

    /// Путь к файлу.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Прочитать сырой файл моно f32.
pub fn read_raw_f32(path: &Path) -> std::io::Result<Vec<f32>> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let (chunks, _) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// Перевести блок сэмплов s16le (little-endian) в f32.
pub fn s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<2>();
    chunks
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}

/// Линейный ресемплинг в целевую частоту (48→16 кГц и конверсия контейнеров).
pub fn resample_to(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if samples.is_empty() || from == to || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let target_len = (samples.len() as u64 * to as u64 / from as u64).max(1) as usize;
    let mut out = Vec::with_capacity(target_len);
    for i in 0..target_len {
        let pos = i as f64 * (samples.len() as f64 / target_len as f64);
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
    /// `None`, если частоты совпадают (ресемплинг не нужен).
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
                let i = start.min(self.pending.len().saturating_sub(2));
                let frac = (self.pos - i as f64) as f32;
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
        // RMS 0.5 → с запасом ×3 уровень обрезан в 1.0
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
    fn resample_48k_to_16k_and_zero_guard() {
        let s: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_to(&s, 48000, 16000);
        assert_eq!(out.len(), 1600);
        // Нулевые частоты (битый заголовок) не паникуют — возвращаем как есть.
        assert_eq!(resample_to(&s, 0, 16000).len(), s.len());
        assert_eq!(resample_to(&s, 48000, 0).len(), s.len());
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
}
