use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use screencapturekit::cm::{AudioBufferList, CMTime};
use screencapturekit::prelude::*;
use screencapturekit::stream::delegate_trait::StreamCallbacks;

use crate::capture::stream::{f32le_to_samples, LevelMeter, RawWriter, StreamingResampler};
use crate::capture::{AudioError, CaptureStream, Result};

// 48 кГц: 16 кГц хуже обкатано на Bluetooth/HFP; в 16 кГц уходим ресемплером.
const CAPTURE_RATE: u32 = 48_000;

struct AudioSink {
    writer: RawWriter,
    level: Arc<LevelMeter>,
    resampler: Option<StreamingResampler>,
    capture_rate: u32,
    out_rate: u32,
    // Колбэк вызывается десятки раз в секунду: переиспользуем буферы, чтобы
    // не аллоцировать на каждом кадре.
    samples: Vec<f32>,
    resampled: Vec<f32>,
    silence: Vec<f32>,
    /// По PTS добиваем разрыв тишиной после смены AirPods, иначе дорожка съедет.
    last_end_pts: Option<f64>,
    write_failed: bool,
}

impl AudioSink {
    fn push(&mut self, list: &AudioBufferList, pts: f64, dur: f64) {
        if self.write_failed {
            return;
        }
        if let Some(last) = self.last_end_pts {
            let gap = pts - last;
            // Отрицательный скачок или разрыв больше 10 минут — новый клок
            // после переподключения/сна, не паддим.
            if gap < 0.0 || gap > 600.0 {
                self.last_end_pts = None;
            } else if gap > 0.01 {
                self.write_silence((gap * self.out_rate as f64) as usize);
                if self.write_failed {
                    return;
                }
            }
        }
        self.samples.clear();
        // channel_count(1): SCK отдаёт один моно-буфер.
        for buffer in list.iter() {
            f32le_to_samples(buffer.data(), &mut self.samples);
        }
        if self.samples.is_empty() {
            return;
        }
        let dur = if dur > 0.0 {
            dur
        } else {
            self.samples.len() as f64 / self.capture_rate as f64
        };
        self.last_end_pts = Some(pts + dur);
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

    fn write_silence(&mut self, mut samples: usize) {
        // Больше 10 минут разрыва — это сон/сбой, а не смена AirPods.
        let cap = 10 * 60 * self.out_rate as usize;
        samples = samples.min(cap);
        while samples > 0 {
            let n = samples.min(self.silence.len());
            if let Err(e) = self.writer.write_samples(&self.silence[..n]) {
                self.write_failed = true;
                eprintln!("Ошибка диска при записи тишины системного звука: {e}");
                return;
            }
            samples -= n;
        }
    }
}

#[derive(Clone)]
struct AudioHandler {
    sink: Arc<Mutex<AudioSink>>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, kind: SCStreamOutputType) {
        // Без Screen-output SCK сыпет предупреждениями о неподключённом выходе.
        if kind != SCStreamOutputType::Audio {
            return;
        }
        let Ok(list) = sample.audio_buffer_list() else {
            return;
        };
        let pts = seconds(sample.presentation_timestamp());
        let dur = seconds(sample.duration());
        if let Ok(mut sink) = self.sink.lock() {
            sink.push(&list, pts, dur);
        }
    }
}

fn seconds(time: CMTime) -> f64 {
    if time.timescale > 0 {
        time.value as f64 / time.timescale as f64
    } else {
        0.0
    }
}

struct SckStream {
    stream: SCStream,
    failed: Arc<AtomicBool>,
}

fn open_stream(sink: &Arc<Mutex<AudioSink>>, rate: u32) -> std::result::Result<SckStream, String> {
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
        .with_sample_rate(rate as i32)
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

    Ok(SckStream { stream, failed })
}

struct SckCapture {
    stream: SCStream,
    failed: Arc<AtomicBool>,
    sink: Arc<Mutex<AudioSink>>,
}

impl SckCapture {
    fn open(path: &Path, rate: u32, level: &Arc<LevelMeter>) -> std::result::Result<Self, String> {
        let writer = RawWriter::create(path)
            .map_err(|e| format!("системный звук macOS: не создан файл записи ({e})"))?;
        let sink = Arc::new(Mutex::new(AudioSink {
            writer,
            level: level.clone(),
            resampler: StreamingResampler::new(CAPTURE_RATE, rate),
            capture_rate: CAPTURE_RATE,
            out_rate: rate,
            samples: Vec::with_capacity(4096),
            resampled: Vec::new(),
            silence: vec![0.0; 4096],
            last_end_pts: None,
            write_failed: false,
        }));
        let SckStream { stream, failed } = open_stream(&sink, CAPTURE_RATE)?;
        Ok(Self {
            stream,
            failed,
            sink,
        })
    }

    fn run(self, stop: &AtomicBool) {
        let Self {
            mut stream,
            mut failed,
            sink,
        } = self;
        loop {
            while !stop.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = stream.stop_capture();
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if !super::permission::screen_recording_granted() {
                eprintln!("Системный звук macOS: разрешение отозвано, переподключение остановлено");
                break;
            }
            // Смена AirPods останавливает SCK; 5 попыток по 0.5 с закрывают
            // штатное переключение A2DP → HFP (govorilka ждёт до 5 секунд).
            let mut reopened = None;
            for attempt in 1..=5 {
                std::thread::sleep(Duration::from_millis(500));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match open_stream(&sink, CAPTURE_RATE) {
                    Ok(next) => {
                        reopened = Some(next);
                        break;
                    }
                    Err(e) => eprintln!(
                        "Системный звук macOS: переподключение {attempt}/5 не удалось: {e}"
                    ),
                }
            }
            match reopened {
                Some(next) => {
                    stream = next.stream;
                    failed = next.failed;
                }
                None => {
                    eprintln!(
                        "Системный звук macOS: переподключение не удалось, пишу только микрофон"
                    );
                    break;
                }
            }
        }
        drop(stream);
        if let Ok(mut sink) = sink.lock() {
            let _ = sink.writer.flush();
        };
    }
}

pub fn start_stream(path: &Path, rate: u32) -> Result<CaptureStream> {
    let rate = if rate == 0 { 16_000 } else { rate };
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
    // 60 секунд: первый `SCShareableContent::get()` показывает системный запрос
    // на запись экрана и ждёт ответа пользователя.
    match start_rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(stream),
        Ok(Err(e)) => {
            drop(stream);
            Err(AudioError::Record(e))
        }
        Err(_) => {
            drop(stream);
            Err(AudioError::Record(
                "ScreenCaptureKit не подтвердил запуск записи за 60 секунд".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn capture_rate_is_sck_native() {
        assert_eq!(super::CAPTURE_RATE, 48_000);
    }
}
