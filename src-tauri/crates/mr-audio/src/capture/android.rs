//! Android: живой захват микрофона через системную библиотеку `libaaudio.so`.
//!
//! Библиотека загружается через `dlopen` в рантайме (доступна в Android 8.0+ / API 26).
//! Захватываем моно PCM на запрошенной частоте (сейчас 16 кГц; при несовпадении
//! с нативной частотой устройства — ресемплируем) и пишем f32 в промежуточный файл.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use libc::{dlclose, dlopen, dlsym};

use super::stream::{resample_to, RawWriter};
use super::{AudioError, CaptureStream, DeviceCatalog, DeviceInfo, Result, StreamKind};

pub const AAUDIO_OK: i32 = 0;
pub const AAUDIO_DIRECTION_INPUT: i32 = 1;
pub const AAUDIO_FORMAT_PCM_I16: i32 = 1;
pub const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;

struct AAudioLib {
    handle: *mut c_void,
    create_builder: unsafe extern "C" fn(*mut *mut c_void) -> i32,
    builder_delete: unsafe extern "C" fn(*mut c_void) -> i32,
    builder_set_direction: unsafe extern "C" fn(*mut c_void, i32),
    builder_set_sample_rate: unsafe extern "C" fn(*mut c_void, i32),
    builder_set_channel_count: unsafe extern "C" fn(*mut c_void, i32),
    builder_set_format: unsafe extern "C" fn(*mut c_void, i32),
    builder_open_stream: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32,
    stream_request_start: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_request_stop: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_close: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_read: unsafe extern "C" fn(*mut c_void, *mut c_void, i32, i64) -> i32,
    stream_get_sample_rate: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_get_channel_count: unsafe extern "C" fn(*mut c_void) -> i32,
    stream_get_format: unsafe extern "C" fn(*mut c_void) -> i32,
    convert_result_to_text: unsafe extern "C" fn(i32) -> *const c_char,
}

unsafe impl Send for AAudioLib {}
unsafe impl Sync for AAudioLib {}

impl Drop for AAudioLib {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { dlclose(self.handle) };
        }
    }
}

impl AAudioLib {
    unsafe fn load() -> Option<Self> {
        let soname = CString::new("libaaudio.so").ok()?;
        let handle = dlopen(soname.as_ptr(), libc::RTLD_NOW);
        if handle.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:ident, $type:ty) => {{
                let s = CString::new(stringify!($name)).ok()?;
                let ptr = dlsym(handle, s.as_ptr());
                if ptr.is_null() {
                    dlclose(handle);
                    return None;
                }
                std::mem::transmute::<*mut c_void, $type>(ptr)
            }};
        }
        Some(Self {
            handle,
            create_builder: sym!(
                AAudio_createStreamBuilder,
                unsafe extern "C" fn(*mut *mut c_void) -> i32
            ),
            builder_delete: sym!(
                AAudioStreamBuilder_delete,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            builder_set_direction: sym!(
                AAudioStreamBuilder_setDirection,
                unsafe extern "C" fn(*mut c_void, i32)
            ),
            builder_set_sample_rate: sym!(
                AAudioStreamBuilder_setSampleRate,
                unsafe extern "C" fn(*mut c_void, i32)
            ),
            builder_set_channel_count: sym!(
                AAudioStreamBuilder_setChannelCount,
                unsafe extern "C" fn(*mut c_void, i32)
            ),
            builder_set_format: sym!(
                AAudioStreamBuilder_setFormat,
                unsafe extern "C" fn(*mut c_void, i32)
            ),
            builder_open_stream: sym!(
                AAudioStreamBuilder_openStream,
                unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32
            ),
            stream_request_start: sym!(
                AAudioStream_requestStart,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            stream_request_stop: sym!(
                AAudioStream_requestStop,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            stream_close: sym!(AAudioStream_close, unsafe extern "C" fn(*mut c_void) -> i32),
            stream_read: sym!(
                AAudioStream_read,
                unsafe extern "C" fn(*mut c_void, *mut c_void, i32, i64) -> i32
            ),
            stream_get_sample_rate: sym!(
                AAudioStream_getSampleRate,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            stream_get_channel_count: sym!(
                AAudioStream_getChannelCount,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            stream_get_format: sym!(
                AAudioStream_getFormat,
                unsafe extern "C" fn(*mut c_void) -> i32
            ),
            convert_result_to_text: sym!(
                AAudio_convertResultToText,
                unsafe extern "C" fn(i32) -> *const c_char
            ),
        })
    }

    fn error_str(&self, code: i32) -> String {
        unsafe {
            let ptr = (self.convert_result_to_text)(code);
            if ptr.is_null() {
                format!("код {code}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }
}

static LIB: OnceLock<Option<AAudioLib>> = OnceLock::new();

fn aaudio_lib() -> Result<&'static AAudioLib> {
    let opt = LIB.get_or_init(|| unsafe { AAudioLib::load() });
    opt.as_ref().ok_or_else(|| {
        AudioError::Other("библиотека libaaudio.so недоступна (требуется Android 8.0+)".to_string())
    })
}

pub fn devices() -> DeviceCatalog {
    DeviceCatalog {
        platform: "android".to_string(),
        inputs: vec![DeviceInfo {
            id: "default".to_string(),
            name: "Микрофон".to_string(),
            muted: false,
            system: false,
        }],
        system: Vec::new(),
        default_input: Some("default".to_string()),
        default_system: None,
    }
}

pub fn start_stream(
    kind: StreamKind,
    _device: Option<&str>,
    path: &Path,
    rate: u32,
) -> Result<CaptureStream> {
    if kind == StreamKind::System {
        return Err(AudioError::Other(
            "захват системного звука на Android не поддерживается: пишется только микрофон"
                .to_string(),
        ));
    }
    let lib = aaudio_lib()?;
    unsafe {
        let mut builder: *mut c_void = std::ptr::null_mut();
        let res = (lib.create_builder)(&mut builder);
        if res != AAUDIO_OK || builder.is_null() {
            return Err(AudioError::Record(format!(
                "не удалось создать builder AAudio: {}",
                lib.error_str(res)
            )));
        }
        (lib.builder_set_direction)(builder, AAUDIO_DIRECTION_INPUT);
        (lib.builder_set_sample_rate)(builder, rate as i32);
        (lib.builder_set_channel_count)(builder, 1);
        (lib.builder_set_format)(builder, AAUDIO_FORMAT_PCM_I16);

        let mut stream: *mut c_void = std::ptr::null_mut();
        let res = (lib.builder_open_stream)(builder, &mut stream);
        let _ = (lib.builder_delete)(builder);

        if res != AAUDIO_OK || stream.is_null() {
            let err = lib.error_str(res);
            let detail = if res == -905 || err.contains("NotAllowed") {
                "доступ к микрофону запрещён: включите разрешение в настройках Android"
            } else {
                &err
            };
            return Err(AudioError::Record(format!(
                "не удалось открыть поток микрофона AAudio ({detail})"
            )));
        }

        // Формат узнаём до старта: поток с экзотическим PCM (I24/I32) отвергаем сразу,
        // не запуская его и не рискуя писать в неподходящий буфер.
        let actual_rate = (lib.stream_get_sample_rate)(stream);
        let actual_channels = (lib.stream_get_channel_count)(stream);
        let actual_format = (lib.stream_get_format)(stream);
        if actual_format != AAUDIO_FORMAT_PCM_I16 && actual_format != AAUDIO_FORMAT_PCM_FLOAT {
            let _ = (lib.stream_close)(stream);
            return Err(AudioError::Record(format!(
                "неподдерживаемый формат PCM AAudio ({actual_format})"
            )));
        }

        let res = (lib.stream_request_start)(stream);
        if res != AAUDIO_OK {
            let err = lib.error_str(res);
            let _ = (lib.stream_close)(stream);
            let detail = if res == -905 || err.contains("NotAllowed") {
                "доступ к микрофону запрещён: включите разрешение в настройках Android"
            } else {
                &err
            };
            return Err(AudioError::Record(format!(
                "не удалось запустить запись AAudio ({detail})"
            )));
        }

        super::foreground_service(false, true);
        let result = spawn_stream(
            lib,
            stream,
            path.to_path_buf(),
            rate,
            actual_rate,
            actual_channels,
            actual_format,
        );
        if result.is_err() {
            super::foreground_service(false, false);
        }
        result
    }
}

fn spawn_stream(
    lib: &'static AAudioLib,
    stream: *mut c_void,
    path: std::path::PathBuf,
    rate: u32,
    actual_rate: i32,
    actual_channels: i32,
    actual_format: i32,
) -> Result<CaptureStream> {
    let stream_addr = stream as usize;
    CaptureStream::spawn(path, move |path, level, stop| {
        let stream = stream_addr as *mut c_void;
        let mut writer = match RawWriter::create(path) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("Не удалось создать файл записи: {e}");
                unsafe {
                    let _ = (lib.stream_request_stop)(stream);
                    let _ = (lib.stream_close)(stream);
                }
                super::foreground_service(false, false);
                return;
            }
        };

        let channels = (actual_channels.max(1)) as usize;
        let source_rate = (actual_rate.max(1)) as u32;
        let target_rate = rate;
        let frames_per_read = 1024usize;
        let timeout_ns = 100_000_000i64;

        let mut i16_buf = if actual_format == AAUDIO_FORMAT_PCM_I16 {
            vec![0i16; frames_per_read * channels]
        } else {
            Vec::new()
        };
        let mut f32_buf = if actual_format == AAUDIO_FORMAT_PCM_FLOAT {
            vec![0f32; frames_per_read * channels]
        } else {
            Vec::new()
        };

        while !stop.load(Ordering::Relaxed) {
            let read_frames = unsafe {
                if actual_format == AAUDIO_FORMAT_PCM_FLOAT {
                    (lib.stream_read)(
                        stream,
                        f32_buf.as_mut_ptr() as *mut c_void,
                        frames_per_read as i32,
                        timeout_ns,
                    )
                } else {
                    (lib.stream_read)(
                        stream,
                        i16_buf.as_mut_ptr() as *mut c_void,
                        frames_per_read as i32,
                        timeout_ns,
                    )
                }
            };

            if read_frames > 0 {
                let n = read_frames as usize;
                let mono_samples: Vec<f32> = if actual_format == AAUDIO_FORMAT_PCM_FLOAT {
                    if channels == 1 {
                        f32_buf[..n].to_vec()
                    } else {
                        f32_buf[..n * channels]
                            .chunks_exact(channels)
                            .map(|ch| ch[0])
                            .collect()
                    }
                } else {
                    if channels == 1 {
                        i16_buf[..n].iter().map(|&s| s as f32 / 32768.0).collect()
                    } else {
                        i16_buf[..n * channels]
                            .chunks_exact(channels)
                            .map(|ch| ch[0] as f32 / 32768.0)
                            .collect()
                    }
                };

                let final_samples = if source_rate != target_rate {
                    resample_to(&mono_samples, source_rate, target_rate)
                } else {
                    mono_samples
                };

                if !final_samples.is_empty() {
                    level.update(&final_samples);
                    if let Err(e) = writer.write_samples(&final_samples) {
                        eprintln!("Ошибка записи звука: {e}");
                        break;
                    }
                }
            } else if read_frames < 0 {
                eprintln!("AAudio read: {}", lib.error_str(read_frames));
                break;
            }
        }

        unsafe {
            let _ = (lib.stream_request_stop)(stream);
            let _ = (lib.stream_close)(stream);
        }
        let _ = writer.flush();
        super::foreground_service(false, false);
    })
}
