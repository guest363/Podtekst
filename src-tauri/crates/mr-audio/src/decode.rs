//! Декодирование любых аудио/видео-контейнеров в 16 кГц float32 (чистый Rust, `symphonia`).
//! Заменяет внешний `ffmpeg` для Drag & Drop расшифровки (MP3, M4A, OGG, FLAC, WAV, MP4…).

use std::path::Path;

use opus_decoder::OpusDecoder;
use symphonia::core::codecs::{CODEC_TYPE_AAC, CODEC_TYPE_OPUS};

use super::capture::stream::resample_to;
use super::{AudioError, Result};

/// Реалистичные границы частоты дискретизации: всё вне — битый заголовок.
const MIN_RATE: u32 = 4000;
const MAX_RATE: u32 = 384_000;
/// Верхняя граница каналов: файл с сотней каналов — это атака на память, не музыка.
const MAX_CHANNELS: usize = 8;

/// Декодирует аудиодорожку в поканальные моно-дорожки 16 кГц `[-1, 1]`.
/// Возвращает Vec по каналам (обычно 1–2). Стерео-записи встреч: [0] = микрофон, [1] = система.
pub fn decode_to_channels_16k(path: &Path) -> Result<Vec<Vec<f32>>> {
    let file = std::fs::File::open(path).map_err(AudioError::Io)?;
    let mss = symphonia::core::io::MediaSourceStream::new(Box::new(file), Default::default());
    let hint = {
        let mut h = symphonia::core::probe::Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            h.with_extension(ext);
        }
        h
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| AudioError::Other(format!("probe: {e}")))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Other("Файл не содержит аудиодорожки".to_string()))?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let src_rate = codec_params
        .sample_rate
        .filter(|r| (MIN_RATE..=MAX_RATE).contains(r))
        .ok_or_else(|| {
            AudioError::Other("в файле неизвестная частота дискретизации".to_string())
        })?;
    let n_ch = codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .clamp(1, MAX_CHANNELS);
    // OGG/Opus: symphonia отдаёт пакеты, но своего декодера Opus в 0.5 нет.
    let mut opus = if codec_params.codec == CODEC_TYPE_OPUS {
        Some(
            OpusDecoder::new(src_rate, n_ch)
                .map_err(|e| AudioError::Other(format!("opus: {e}")))?,
        )
    } else {
        None
    };
    let mut pre_skip = codec_params.delay.unwrap_or(0) as usize;
    // Symphonia не достаёт AAC priming из edit list MP4: без отсечения
    // речь в VAD смещается на ~21 мс, и тихие короткие слова теряются.
    // shortcut: AAC-LC priming 1024; для HE-AAC (редко) может быть 2048.
    // upgrade: читать edit list / iTunSMPB.
    if pre_skip == 0 && codec_params.codec == CODEC_TYPE_AAC {
        pre_skip = 1024;
    }
    let mut decoder = if opus.is_some() {
        None
    } else {
        Some(
            symphonia::default::get_codecs()
                .make(&codec_params, &Default::default())
                .map_err(|e| AudioError::Other(format!("codec: {e}")))?,
        )
    };

    let mut chans: Vec<Vec<f32>> = (0..n_ch).map(|_| Vec::new()).collect();
    // Буферы вне цикла: аллокация на каждый пакет — это тысячи malloc на файл.
    let mut opus_pcm = vec![0.0f32; 0usize];
    let mut pcm_buf: Option<symphonia::core::audio::SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) if p.track_id() == track_id => p,
            Ok(_) => continue,
            Err(_) => break,
        };
        if let Some(op) = opus.as_mut() {
            let need = op.max_frame_size_per_channel() * n_ch;
            if opus_pcm.len() < need {
                opus_pcm.resize(need, 0.0);
            }
            let pcm = &mut opus_pcm[..need];
            let frames = op
                .decode_float(&packet.data, pcm, false)
                .map_err(|e| AudioError::Other(format!("opus: {e}")))?;
            let skip = pre_skip.min(frames);
            pre_skip -= skip;
            for frame in pcm.chunks(n_ch).take(frames).skip(skip) {
                for (c, ch) in chans.iter_mut().enumerate() {
                    ch.push(frame.get(c % n_ch).copied().unwrap_or(0.0).clamp(-1.0, 1.0));
                }
            }
            continue;
        }
        let Some(dec) = decoder.as_mut() else {
            break;
        };
        let decoded = match dec.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        let need = decoded
            .capacity()
            .max(decoded.frames() * spec.channels.count());
        if pcm_buf.as_ref().is_none_or(|b| b.capacity() < need) {
            pcm_buf = Some(symphonia::core::audio::SampleBuffer::new(need as u64, spec));
        }
        let buf = pcm_buf.as_mut().expect("bufer только что создан");
        buf.copy_interleaved_ref(decoded);
        let got = spec.channels.count().max(1);
        // AAC/MP3 отдают priming-сэмплы в начале: без их отсечения VAD
        // слышит речь на 20+ мс позже и теряет тихие короткие слова.
        let frames = buf.samples().len() / got;
        let skip = pre_skip.min(frames);
        pre_skip -= skip;
        for frame in buf.samples().chunks(got).skip(skip) {
            for (c, ch) in chans.iter_mut().enumerate() {
                ch.push(frame.get(c % got).copied().unwrap_or(0.0).clamp(-1.0, 1.0));
            }
        }
    }
    if chans.iter().all(|c| c.is_empty()) {
        return Err(AudioError::Other("пустой аудиопоток".to_string()));
    }
    Ok(chans
        .into_iter()
        .map(|c| resample_to(&c, src_rate, 16000))
        .collect())
}

pub fn decode_to_mono_16k(path: &Path) -> Result<Vec<f32>> {
    let chans = decode_to_channels_16k(path)?;
    let n = chans.iter().map(|c| c.len()).max().unwrap_or(0);
    let mut mono = vec![0.0f32; n];
    for ch in &chans {
        for (i, &x) in ch.iter().enumerate() {
            mono[i] += x / chans.len() as f32;
        }
    }
    Ok(mono)
}

pub fn is_video_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v")
}

/// Длительность и число каналов без полного декодирования: WAV — из заголовка,
/// остальные контейнеры — `ffprobe` (он уже нужен для MKV/WebM/MOV/AVI).
/// Возвращает `None`, если метаданные недоступны.
pub fn media_info(path: &Path) -> Option<(f64, usize)> {
    let is_wav = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        if let Ok(reader) = hound::WavReader::open(path) {
            let spec = reader.spec();
            let rate = u64::from(spec.sample_rate).max(1);
            let ch = u64::from(spec.channels).max(1);
            let dur = reader.len() as f64 / ch as f64 / rate as f64;
            return Some((dur, ch as usize));
        }
    }
    // shortcut: ffprobe — внешний бинарник; без него длительность не-WAV
    // догрузит плеер из метаданных. upgrade: symphonia для MP3/M4A/OGG.
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "format=duration:stream=channels",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut duration, mut channels) = (None, 0usize);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("duration=") {
            duration = v.trim().parse::<f64>().ok();
        } else if let Some(v) = line.strip_prefix("channels=") {
            channels = v.trim().parse().unwrap_or(0);
        }
    }
    Some((duration?, channels))
}

pub fn has_audio_track(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let is_wav = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        return hound::WavReader::open(path)
            .map(|r| r.spec().channels > 0 && r.len() > 0)
            .unwrap_or(false);
    }

    // Сначала пробуем нативный symphonia-зонд (MP3, M4A, OGG, FLAC, WAV, MP4 с AAC).
    if let Ok(file) = std::fs::File::open(path) {
        let mss = symphonia::core::io::MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = symphonia::core::probe::Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        if let Ok(probed) = symphonia::default::get_probe().format(
            &hint,
            mss,
            &Default::default(),
            &Default::default(),
        ) {
            if probed
                .format
                .tracks()
                .iter()
                .any(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            {
                return true;
            }
            if !is_video_path(path) {
                return false;
            }
        }
    }

    // Для видео-контейнеров (MKV, WebM, MOV, AVI) или MP4 с кодеками вне symphonia — ffprobe:
    if let Ok(out) = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            return text.contains("codec_type=audio");
        }
    }

    // Фолбэк на ffmpeg, если ffprobe не установлен в системе:
    if let Ok(out) = std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-map",
            "0:a:0",
            "-f",
            "null",
            "-",
        ])
        .output()
    {
        if out.status.success() {
            return true;
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("matches no streams")
            || err.contains("Output file does not contain any stream")
            || err.contains("does not contain any stream")
        {
            return false;
        }
    }

    false
}

pub fn media_duration_sec(path: &Path) -> Option<f64> {
    media_info(path).map(|(d, _)| d)
}

/// Декодирование с fallback: сначала чистый Rust (symphonia), при неудаче —
/// извлечение аудиодорожки через `ffmpeg` во временный WAV (нужно для MKV/WebM/MOV/AVI,
/// у которых нет symphonia-демuxера).
/// shortcut: внешний ffmpeg только как fallback для контейнеров без symphonia-поддержки; upgrade: нативный demuxer при появлении в symphonia.
pub fn decode_to_channels_16k_auto(path: &Path) -> Result<Vec<Vec<f32>>> {
    match decode_to_channels_16k(path) {
        Ok(ch) => Ok(ch),
        Err(e) if !is_video_path(path) => Err(e),
        Err(_) => ffmpeg_extract_to_wav(path).and_then(|tmp| {
            let res = decode_to_channels_16k(&tmp);
            let _ = std::fs::remove_file(&tmp);
            res
        }),
    }
}

fn ffmpeg_extract_to_wav(path: &Path) -> Result<std::path::PathBuf> {
    let tmp = std::env::temp_dir().join(format!(
        "mr-extract-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-i",
            &path.to_string_lossy(),
            "-vn",
            "-ac",
            "2",
            "-ar",
            "16000",
            "-f",
            "wav",
            &tmp.to_string_lossy(),
        ])
        .output()
        .map_err(|e| {
            AudioError::Other(format!(
                "ffmpeg не найден ({e}). Для видео MKV, WebM, MOV, AVI установите ffmpeg."
            ))
        })?;
    if !out.status.success() || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        let err_raw = String::from_utf8_lossy(&out.stderr);
        if err_raw.contains("does not contain any stream")
            || err_raw.contains("Output file does not contain any stream")
            || err_raw.contains("matches no streams")
        {
            return Err(AudioError::Other(
                "Файл не содержит аудиодорожки".to_string(),
            ));
        }
        let err = err_raw.chars().take(200).collect::<String>();
        return Err(AudioError::Other(format!(
            "ffmpeg не смог извлечь звук из видео: {}",
            err.trim()
        )));
    }
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_missing_file_errors() {
        assert!(decode_to_mono_16k(Path::new("/nonexistent-xyz/file.mp3")).is_err());
        assert!(decode_to_channels_16k(Path::new("/nonexistent-xyz/file.mp3")).is_err());
    }

    /// Видео требует ffmpeg-извлечения: MKV symphonia не берёт, `auto` — берёт.
    /// Пропускается без ffmpeg в PATH (как onnx_real без моделей).
    #[test]
    fn video_needs_ffmpeg_fallback() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "mr-av-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mkv = dir.join("probe.mkv");
        let mkv_mp4 = dir.join("probe.mp4");
        // 1 с тона + 64x64 видео; mpeg4/aac — всегда встроенные кодировщики ffmpeg.
        for out in [&mkv, &mkv_mp4] {
            let st = std::process::Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=black:s=64x64:d=1:r=5",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "aac",
                    "-shortest",
                    &out.to_string_lossy(),
                ])
                .status();
            if st.map(|s| s.success()).unwrap_or(false) {
                let chans = decode_to_channels_16k_auto(out)
                    .expect("auto-декодер обязан взять видео через ffmpeg");
                let n = chans.iter().map(|c| c.len()).max().unwrap_or(0);
                assert!(
                    (15_000..=17_500).contains(&n),
                    "ожидалась ~1 с @16кГц, получено {n}"
                );
                assert!(
                    chans.iter().any(|c| c.iter().any(|&x| x.abs() > 0.01)),
                    "тишина вместо тона 440 Гц"
                );
            }
        }
        assert!(
            decode_to_channels_16k(&mkv).is_err(),
            "symphonia не должен брать MKV напрямую"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OGG Vorbis и OGG Opus декодируются нативно (второй — без ffmpeg-фолбэка).
    /// Пропускается без ffmpeg или без нужного кодировщика.
    #[test]
    fn ogg_vorbis_and_opus_decode_natively() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "mr-ogg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, codec) in [
            ("probe-vorbis.ogg", "libvorbis"),
            ("probe-opus.ogg", "libopus"),
        ] {
            let path = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                continue;
            }
            let chans =
                decode_to_channels_16k_auto(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            let n = chans.iter().map(|c| c.len()).max().unwrap_or(0);
            assert!(
                (14_500..=17_500).contains(&n),
                "{name}: ожидалась ~1 с @16кГц, получено {n}"
            );
            assert!(
                chans.iter().any(|c| c.iter().any(|&x| x.abs() > 0.01)),
                "{name}: тишина вместо тона 440 Гц"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn video_without_audio_detected_cleanly() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "mr-no-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let no_audio_mp4 = dir.join("no_audio.mp4");
        let with_audio_mp4 = dir.join("with_audio.mp4");

        let ok_no = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=10",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&no_audio_mp4)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        let ok_with = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&with_audio_mp4)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok_no && ok_with {
            assert!(!has_audio_track(&no_audio_mp4));
            assert!(has_audio_track(&with_audio_mp4));

            let err = decode_to_channels_16k_auto(&no_audio_mp4).unwrap_err();
            assert_eq!(err.to_string(), "Файл не содержит аудиодорожки");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
