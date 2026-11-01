use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use super::devices::{resolve_device, resolve_system_device};
use super::ffi::*;
use crate::capture::stream::{LevelMeter, RawWriter, StreamingResampler};
use crate::capture::{AudioError, CaptureStream, Result, StreamKind};

struct CallbackContext {
    unit: AudioUnit,
    tx: SyncSender<Vec<f32>>,
    buffer: UnsafeCell<Vec<f32>>,
}

unsafe extern "C" fn input_callback(
    ref_con: *mut c_void,
    action_flags: *mut u32,
    timestamp: *const AudioTimeStamp,
    _bus: u32,
    frames: u32,
    _data: *mut AudioBufferList,
) -> OSStatus {
    let ctx = &*(ref_con as *const CallbackContext);
    let n = frames as usize;
    // SAFETY: колбэк CoreAudio вызывает один поток за раз; Vec не перемещается.
    let buffer = &mut *ctx.buffer.get();
    if n == 0 || n > buffer.len() {
        return 0;
    }
    let mut list = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: 1,
            data_byte_size: (n * std::mem::size_of::<f32>()) as u32,
            data: buffer.as_mut_ptr() as *mut c_void,
        }],
    };
    // SAFETY: забираем ровно n кадров из входной шины 1 в наш буфер.
    let status = AudioUnitRender(ctx.unit, action_flags, timestamp, 1, frames, &mut list);
    if status != 0 {
        return status;
    }
    // Очередь переполнена — лучше потерять блок, чем блокировать аудиопоток.
    let _ = ctx.tx.try_send(buffer[..n].to_vec());
    0
}

struct UnitGuard(AudioUnit);

impl Drop for UnitGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: экземпляр создан AudioComponentInstanceNew и ещё не освобождён.
            unsafe {
                let _ = AudioOutputUnitStop(self.0);
                let _ = AudioUnitUninitialize(self.0);
                let _ = AudioComponentInstanceDispose(self.0);
            }
        }
    }
}

struct CoreAudioCapture {
    unit: UnitGuard,
    rx: Receiver<Vec<f32>>,
    source_rate: u32,
    target_rate: u32,
    /// Держим контекст живым: указатель на него уходит в CoreAudio.
    _ctx: Box<CallbackContext>,
}

fn check(status: OSStatus, what: &str) -> std::result::Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what}: код CoreAudio {status}"))
    }
}

impl CoreAudioCapture {
    fn open(device: AudioDeviceId, rate: u32) -> std::result::Result<Self, String> {
        let desc = AudioComponentDescription {
            component_type: UNIT_TYPE_OUTPUT,
            component_sub_type: UNIT_SUBTYPE_HAL_OUTPUT,
            component_manufacturer: UNIT_MANUFACTURER_APPLE,
            component_flags: 0,
            component_flags_mask: 0,
        };
        // SAFETY: поиск системного компонента по описанию.
        let component = unsafe { AudioComponentFindNext(null_mut(), &desc) };
        if component.is_null() {
            return Err(
                "CoreAudio не нашёл HALOutput — звуковая подсистема недоступна".to_string(),
            );
        }
        let mut unit: AudioUnit = null_mut();
        // SAFETY: создаём экземпляр найденного компонента.
        check(
            unsafe { AudioComponentInstanceNew(component, &mut unit) },
            "не создан аудиоюнит",
        )?;
        let guard = UnitGuard(unit);

        let enable: u32 = 1;
        // SAFETY: входная шина 1 включается, выходная 0 выключается.
        check(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    OUTPUT_UNIT_PROPERTY_ENABLE_IO,
                    UNIT_SCOPE_INPUT,
                    1,
                    &enable as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                )
            },
            "не включён вход CoreAudio",
        )?;
        let disable: u32 = 0;
        check(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    OUTPUT_UNIT_PROPERTY_ENABLE_IO,
                    UNIT_SCOPE_OUTPUT,
                    0,
                    &disable as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                )
            },
            "не выключен выход CoreAudio",
        )?;
        check(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
                    UNIT_SCOPE_GLOBAL,
                    0,
                    &device as *const u32 as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                )
            },
            "не выбрано устройство CoreAudio",
        )?;

        // Частоту устройства читаем с input-скоупа элемента 1: AUHAL умеет
        // менять формат и каналы, но не частоту дискретизации. Клиентский
        // формат задаётся на output-скоупе того же элемента.
        let mut device_format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
        let mut device_format_size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        check(
            unsafe {
                AudioUnitGetProperty(
                    unit,
                    UNIT_PROPERTY_STREAM_FORMAT,
                    UNIT_SCOPE_INPUT,
                    1,
                    &mut device_format as *mut AudioStreamBasicDescription as *mut c_void,
                    &mut device_format_size,
                )
            },
            "не прочитан формат микрофона",
        )?;
        let source_rate = if device_format.sample_rate > 0.0 {
            device_format.sample_rate.round() as u32
        } else {
            rate
        };
        let format = AudioStreamBasicDescription {
            sample_rate: source_rate as f64,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FORMAT_FLAG_IS_FLOAT | FORMAT_FLAG_IS_PACKED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 1,
            bits_per_channel: 32,
            reserved: 0,
        };
        check(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    UNIT_PROPERTY_STREAM_FORMAT,
                    UNIT_SCOPE_OUTPUT,
                    1,
                    &format as *const AudioStreamBasicDescription as *const c_void,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                )
            },
            "устройство не поддерживает моно f32",
        )?;

        let (tx, rx) = sync_channel(32);
        let ctx = Box::new(CallbackContext {
            unit,
            tx,
            buffer: UnsafeCell::new(vec![0.0f32; 8192]),
        });
        let callback = RenderCallbackStruct {
            input_proc: Some(input_callback),
            input_proc_ref_con: &*ctx as *const CallbackContext as *mut c_void,
        };
        check(
            unsafe {
                AudioUnitSetProperty(
                    unit,
                    OUTPUT_UNIT_PROPERTY_SET_INPUT_CALLBACK,
                    UNIT_SCOPE_GLOBAL,
                    0,
                    &callback as *const RenderCallbackStruct as *const c_void,
                    std::mem::size_of::<RenderCallbackStruct>() as u32,
                )
            },
            "не установлен аудиоколбэк",
        )?;
        check(
            unsafe { AudioUnitInitialize(unit) },
            "не инициализирован аудиоюнит",
        )?;
        check(
            unsafe { AudioOutputUnitStart(unit) },
            "не запущен захват CoreAudio",
        )?;
        Ok(Self {
            unit: guard,
            rx,
            source_rate,
            target_rate: rate,
            _ctx: ctx,
        })
    }

    fn run(&mut self, path: &Path, level: &LevelMeter, stop: &AtomicBool) {
        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать файл записи: {e}");
                return;
            }
        };
        let mut resampler = StreamingResampler::new(self.source_rate, self.target_rate);
        let mut io_error: Option<std::io::Error> = None;
        while !stop.load(Ordering::Relaxed) {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(samples) => {
                    if samples.is_empty() {
                        continue;
                    }
                    let samples = match resampler.as_mut() {
                        Some(r) => r.push(&samples),
                        None => samples,
                    };
                    if samples.is_empty() {
                        continue;
                    }
                    level.update(&samples);
                    if let Err(e) = writer.write_samples(&samples) {
                        io_error.get_or_insert(e);
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // Сначала останавливаем CoreAudio, затем дописываем то, что успело
        // прийти в очередь: иначе терялся бы хвост последней фразы.
        let unit = std::mem::replace(&mut self.unit.0, null_mut());
        if !unit.is_null() {
            // SAFETY: юнит ещё не освобождён; освободит Drop после подмены на null.
            unsafe {
                let _ = AudioOutputUnitStop(unit);
                let _ = AudioUnitUninitialize(unit);
                let _ = AudioComponentInstanceDispose(unit);
            }
        }
        while let Ok(samples) = self.rx.try_recv() {
            if !samples.is_empty() {
                let samples = match resampler.as_mut() {
                    Some(r) => r.push(&samples),
                    None => samples,
                };
                if !samples.is_empty() {
                    level.update(&samples);
                    let _ = writer.write_samples(&samples);
                }
            }
        }
        let _ = writer.flush();
        if let Some(e) = io_error {
            eprintln!("Ошибка диска при записи звука: {e}");
        }
    }
}

pub fn start_stream(
    kind: StreamKind,
    device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    let device = match kind {
        StreamKind::Microphone => resolve_device(device)?,
        StreamKind::System => resolve_system_device(device)?,
    };
    let rate = if rate == 0 { 16_000 } else { rate };
    let path = path.to_path_buf();
    let (start_tx, start_rx) = sync_channel::<std::result::Result<(), String>>(1);
    let stream =
        CaptureStream::spawn(
            path.clone(),
            move |path, level, stop| match CoreAudioCapture::open(device, rate) {
                Ok(mut capture) => {
                    let _ = start_tx.try_send(Ok(()));
                    capture.run(path, level, stop);
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
                "CoreAudio не подтвердил запуск записи за 3 секунды".to_string(),
            ))
        }
    }
}
