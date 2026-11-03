//! Системный звук macOS через ScreenCaptureKit (без виртуального драйвера).
//!
//! SCK отдаёт звук вместе с видеокадрами: `with_captures_audio(true)` включаем,
//! а сами кадры не используем. Разрешение на запись экрана проверяет система
//! при первом `SCShareableContent::get()` — отдельного запроса нет.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use screencapturekit::cm::{AudioBufferList, CMSampleBuffer, CMTime};
use screencapturekit::prelude::*;
use screencapturekit::stream::delegate_trait::StreamCallbacks;

use crate::capture::stream::{f32le_to_samples, LevelMeter, RawWriter, StreamingResampler};
use crate::capture::{AudioError, CaptureStream, Result};

/// SCK принимает только 8/16/24/48 кГц; остальное молча заменяет на 48 кГц.
fn supported_rate(rate: u32) -> u32 {
    match rate {
        8_000 | 16_000 | 24_000 | 48_000 => rate,
        _ => 48_000,
    }
}

struct AudioSink {
    writer: RawWriter,
    level: Arc<LevelMeter>,
    resampler: Option<StreamingResampler>,
    /// Переиспользуемые буферы: колбэк вызывается десятки раз в секунду.
    samples: Vec<f32>,
    resampled: Vec<f32>,
    write_failed: bool,
}

impl AudioSink {
    fn push(&mut self, list: &AudioBufferList) {
        if self.write_failed {
            return;
        }
        self.samples.clear();
        // При channel_count(1) SCK кладёт моно в один буфер; лишние буферы
        // (если система их всё же отдаст) дописываем подряд.
        for buffer in list.iter() {
            f32le_to_samples(buffer.data(), &mut self.samples);
        }
        if self.samples.is_empty() {
            return;
        }
        let samples = match self.resampler.as_mut() {
            Some(resampler) => {
                self.resampled = resampler.push(&self.samples);
                &self.resampled
            }
            None => &self.samples,
        };
        if samples.is_empty() {
            return;
        }
        self.level.update(samples);
        if let Err(e) = self.writer.write_samples(samples) {
            self.write_failed = true;
            eprintln!("Ошибка диска при записи системного звука: {e}");
        }
    }
}

#[derive(Clone)]
struct AudioHandler {
    sink: Arc<Mutex<AudioSink>>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        // Видеокадры не нужны: регистрация Screen-output только убирает
        // предупреждения SCK о неподключённом выходе.
        if kind != SCStreamOutputType::Audio {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else {
            return;
        };
        if let Ok(mut sink) = self.sink.lock() {
            sink.push(&list);
        }
    }
}

struct SckCapture {
    stream: SCStream,
    failed: Arc<AtomicBool>,
    sink: Arc<Mutex<AudioSink>>,
}

impl SckCapture {
    fn open(path: &Path, rate: u32, level: &Arc<LevelMeter>) -> std::result::Result<Self, String> {
        let capture_rate = supported_rate(rate);
        let writer = RawWriter::create(path)
            .map_err(|e| format!("системный звук macOS: не создан файл записи ({e})"))?;
        let content = SCShareableContent::get()
            .map_err(|e| format!("системный звук macOS: нет доступа к записи экрана ({e})"))?;
        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| "системный звук macOS: не найден дисплей для захвата".to_string())?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()
            .map_err(|e| format!("системный звук macOS: не создан фильтр захвата ({e})"))?;
        // Кадр 2×2 нужен только как формальность: конфигурация SCK не бывает
        // чисто аудио, а минимальный интервал кадров снижает нагрузку.
        let config = SCStreamConfiguration::new()
            .with_width(2)
            .with_height(2)
            .with_captures_audio(true)
            .with_excludes_current_process_audio(true)
            .with_sample_rate(capture_rate as i32)
            .with_channel_count(1)
            .with_minimum_frame_interval(&CMTime::new(1, 1));

        let failed = Arc::new(AtomicBool::new(false));
        let callbacks = StreamCallbacks::new()
            .on_error({
                let failed = failed.clone();
                move |error| {
                    failed.store(true, Ordering::Relaxed);
                    eprintln!("Системный звук macOS остановлен с ошибкой: {error}");
                }
            })
            .on_stop({
                let failed = failed.clone();
                move |message| {
                    failed.store(true, Ordering::Relaxed);
                    if let Some(message) = message {
                        eprintln!("Системный звук macOS остановлен: {message}");
                    }
                }
            });
        let mut stream = SCStream::new_with_delegate(&filter, &config, callbacks)
            .map_err(|e| format!("системный звук macOS: не создан поток захвата ({e})"))?;

        let sink = Arc::new(Mutex::new(AudioSink {
            writer,
            level: level.clone(),
            resampler: StreamingResampler::new(capture_rate, rate),
            samples: Vec::with_capacity(4096),
            resampled: Vec::new(),
            write_failed: false,
        }));
        let handler = AudioHandler { sink: sink.clone() };
        stream
            .add_output_handler(handler.clone(), SCStreamOutputType::Audio)
            .map_err(|e| format!("системный звук macOS: не подключён аудиовыход ({e})"))?;
        stream
            .add_output_handler(handler, SCStreamOutputType::Screen)
            .map_err(|e| format!("системный звук macOS: не подключён видеовыход ({e})"))?;
        stream
            .start_capture()
            .map_err(|e| format!("системный звук macOS: не запущен захват ({e})"))?;

        Ok(Self {
            stream,
            failed,
            sink,
        })
    }

    fn run(self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) && !self.failed.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let Self {
            stream,
            failed: _,
            sink,
        } = self;
        if let Err(e) = stream.stop_capture() {
            eprintln!("Системный звук macOS: не остановлен захват ({e})");
        }
        // Без явного drop SCStream жил бы до конца рабочего потока и продолжал
        // держать колбэки (а с ними — наш sink).
        drop(stream);
        if let Ok(mut sink) = sink.lock() {
            let _ = sink.writer.flush();
        }
    }
}

pub fn start_stream(path: &Path, rate: u32) -> Result<CaptureStream> {
    let rate = if rate == 0 { 16_000 } else { rate };
    if !super::permission::screen_recording_granted() {
        return Err(AudioError::Record(
            "системный звук macOS: разрешите «Запись экрана и системного аудио» в «Системные настройки → Конфиденциальность и безопасность» и перезапустите приложение"
                .to_string(),
        ));
    }
    let path = path.to_path_buf();
    let (start_tx, start_rx) = sync_channel::<std::result::Result<(), String>>(1);
    let stream = CaptureStream::spawn(
        path.clone(),
        move |path, level, stop| match SckCapture::open(path, rate, level) {
            Ok(capture) => {
                let _ = start_tx.try_send(Ok(()));
                capture.run(stop);
            }
            Err(e) => {
                let _ = start_tx.try_send(Err(e));
            }
        },
    )?;
    match start_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(stream),
        Ok(Err(e)) => {
            drop(stream);
            Err(AudioError::Record(e))
        }
        Err(_) => {
            drop(stream);
            Err(AudioError::Record(
                "ScreenCaptureKit не подтвердил запуск записи за 3 секунды".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::supported_rate;

    #[test]
    fn keeps_supported_rates_and_falls_back_to_48k() {
        assert_eq!(supported_rate(16_000), 16_000);
        assert_eq!(supported_rate(48_000), 48_000);
        assert_eq!(supported_rate(44_100), 48_000);
        assert_eq!(supported_rate(0), 48_000);
    }
}
