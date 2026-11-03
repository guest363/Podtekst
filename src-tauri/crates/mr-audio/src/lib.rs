use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("устройство не найдено: {0}")]
    NoDevice(String),
    #[error("ошибка записи: {0}")]
    Record(String),
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AudioError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevicesInfo {
    pub platform: String,
    pub source: String,
    pub sink_monitor: Option<String>,
    pub mic_name: String,
    pub mic_muted: bool,
    pub has_system_audio: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Split,
    Mix,
}

impl ChannelMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "mix" => ChannelMode::Mix,
            _ => ChannelMode::Split,
        }
    }
}

/// Эвристика: какие устройства использовать на текущей ОС без внешних драйверов.
/// Результат кэшируется на 2 секунды: список устройств — обращение к
/// системному аудиосерверу, а не к локальной памяти.
pub fn get_audio_devices_info() -> AudioDevicesInfo {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<Option<(Instant, AudioDevicesInfo)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((at, info)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(2) {
                return info.clone();
            }
        }
    }
    let info = probe_audio_devices_info();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), info.clone()));
    }
    info
}

fn probe_audio_devices_info() -> AudioDevicesInfo {
    let catalog = capture::devices();
    let mic_id = catalog.default_input.clone();
    let mic = catalog
        .inputs
        .iter()
        .find(|i| Some(&i.id) == mic_id.as_ref())
        .or_else(|| catalog.inputs.first());
    AudioDevicesInfo {
        platform: catalog.platform.clone(),
        source: mic_id.clone().unwrap_or_default(),
        sink_monitor: catalog.default_system.clone(),
        mic_name: mic
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "Микрофон не найден".to_string()),
        mic_muted: mic.map(|m| m.muted).unwrap_or(false),
        has_system_audio: catalog.default_system.is_some(),
    }
}

/// Программный микшер двух моно-дорожек 16 кГц:
/// * Split: левый = микрофон, правый = система (стерео, без наложений в тексте)
/// * Mix: среднее арифметическое (обычное стерео для прослушивания)
pub fn mix_channels(mic: &[f32], sys: &[f32], mode: ChannelMode) -> (Vec<f32>, Vec<f32>) {
    let n = mic.len().min(sys.len());
    match mode {
        ChannelMode::Split => (mic[..n].to_vec(), sys[..n].to_vec()),
        ChannelMode::Mix => {
            // Мёртвый канал (SCK не дал звука) не должен глушить живой на 6 дБ.
            let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
            let mono: Vec<f32> = if peak(&sys[..n]) < 1e-5 {
                mic[..n].to_vec()
            } else if peak(&mic[..n]) < 1e-5 {
                sys[..n].to_vec()
            } else {
                mic[..n]
                    .iter()
                    .zip(sys[..n].iter())
                    .map(|(a, b)| (a + b) * 0.5)
                    .collect()
            };
            (mono.clone(), mono)
        }
    }
}

pub fn write_mono_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| AudioError::Record(e.to_string()))?;
    for &x in samples {
        writer
            .write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16)
            .map_err(|e| AudioError::Record(e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| AudioError::Record(e.to_string()))?;
    Ok(path.to_path_buf())
}

pub fn write_stereo_wav(
    path: &Path,
    left: &[f32],
    right: &[f32],
    sample_rate: u32,
) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| AudioError::Record(e.to_string()))?;
    // Каналы живут в независимых потоках и расходятся по длине: общий хвост
    // обрезаем по длинному каналу, короткий добиваем тишиной.
    // shortcut: разрыв больше 10 мин при 16 кГц — признак сбоя одного канала;
    // обрезаем до min + cap, чтобы не раздувать WAV нулями. Апгрейд: чинить
    // рассинхрон в захвате, если декодирование начнёт терять такой хвост.
    const PAD_CAP_SAMPLES: usize = 10 * 60 * 16_000;
    let n = left
        .len()
        .max(right.len())
        .min(left.len().min(right.len()) + PAD_CAP_SAMPLES);
    for i in 0..n {
        let l = (left.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16;
        let r = (right.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i16;
        writer
            .write_sample(l)
            .map_err(|e| AudioError::Record(e.to_string()))?;
        writer
            .write_sample(r)
            .map_err(|e| AudioError::Record(e.to_string()))?;
    }
    writer
        .finalize()
        .map_err(|e| AudioError::Record(e.to_string()))?;
    Ok(path.to_path_buf())
}

pub mod capture;
pub mod decode;
pub mod echo;
pub mod tools;
pub use decode::has_audio_track;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_audio_info_is_consistent_with_monitor_device() {
        let info = get_audio_devices_info();
        if info.source.is_empty() {
            return; // нет аудиосервера — зондировать нечего
        }
        // На macOS системный канал требует BlackHole/Loopback, на Linux —
        // монитор выхода PulseAudio. Флаг и устройство должны совпадать.
        assert_eq!(info.has_system_audio, info.sink_monitor.is_some());
        #[cfg(target_os = "linux")]
        assert!(info.has_system_audio);
    }

    #[test]
    fn split_keeps_channels_separate_mix_averages() {
        let mic = vec![1.0, 1.0, 1.0];
        let sys = vec![-1.0, -1.0, -1.0];
        let (l, r) = mix_channels(&mic, &sys, ChannelMode::Split);
        assert_eq!(l, mic);
        assert_eq!(r, sys);
        let (lm, rm) = mix_channels(&mic, &sys, ChannelMode::Mix);
        assert!(lm.iter().all(|x| x.abs() < 1e-6));
        assert_eq!(lm, rm);
    }

    #[test]
    fn mix_keeps_live_channel_when_other_is_silent() {
        let mic = vec![0.5, -0.5];
        let silence = vec![0.0, 0.0];
        let (m, _) = mix_channels(&mic, &silence, ChannelMode::Mix);
        assert_eq!(m, mic, "мёртвый системный канал не глушит микрофон");
        let (m, _) = mix_channels(&silence, &mic, ChannelMode::Mix);
        assert_eq!(m, mic, "мёртвый микрофон не глушит системный канал");
    }

    #[test]
    fn wav_roundtrip() {
        let dir = std::env::temp_dir().join(format!("mr-audio-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("t.wav");
        write_stereo_wav(&p, &[0.0, 0.5, -0.5], &[0.0, -0.5, 0.5], 16000).unwrap();
        assert!(p.exists());
        let m = dir.join("m.wav");
        write_mono_wav(&m, &[0.0, 0.5, -0.5], 16000).unwrap();
        let reader = hound::WavReader::open(&m).unwrap();
        assert_eq!(reader.spec().channels, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stereo_pads_shorter_channel_with_zeros() {
        let dir = std::env::temp_dir().join(format!("mr-audio-pad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("pad.wav");
        let left = [0.25, -0.25, 0.5];
        let right = [0.1, -0.1, 0.2, 0.3, -0.4];
        write_stereo_wav(&p, &left, &right, 16000).unwrap();
        let mut reader = hound::WavReader::open(&p).unwrap();
        assert_eq!(reader.spec().channels, 2);
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), right.len() * 2, "длина по длинному каналу");
        for (i, &x) in right.iter().enumerate() {
            assert_eq!(samples[i * 2 + 1], (x * 32767.0) as i16, "правый кадр {i}");
        }
        let left_tail: Vec<i16> = samples[left.len() * 2..]
            .iter()
            .step_by(2)
            .copied()
            .collect();
        assert_eq!(left_tail, [0, 0], "левый хвост должен быть тишиной");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
