//! Минимальный FFI к PulseAudio (`libpulse.so.0`), загружаемый в рантайме.
//!
//! Бинарник собирается без системных dev-зависимостей: библиотека
//! подгружается через `dlopen` при первом обращении к звуку. Такой путь
//! работает и с обычным PulseAudio, и с его совместимым сервером внутри
//! PipeWire, и в Flatpak-окружении, где нет ни `pactl`, ни `pw-record`.
//!
//! Используется только то, что нужно для записи: простой API `pa_simple`
//! (чтение сэмплов) и асинхронный API `pa_context` (список устройств).

#![cfg(target_os = "linux")]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

use libc::{dlclose, dlopen, dlsym};

/// `PA_SAMPLE_S16LE` — формат сэмпла, который запрашиваем у сервера (enum = c_int).
pub const PA_SAMPLE_S16LE: c_int = 3;
/// `PA_STREAM_RECORD` — направление потока (запись). 0 = NODIRECTION, 1 = PLAYBACK, 2 = RECORD.
pub const PA_STREAM_RECORD: c_int = 2;
/// `PA_CONTEXT_READY` — контекст подключён к серверу.
pub const PA_CONTEXT_READY: c_int = 4;
/// `PA_CONTEXT_FAILED` — подключение не удалось.
pub const PA_CONTEXT_FAILED: c_int = 5;
/// `PA_CONTEXT_TERMINATED` — сервер завершил соединение.
pub const PA_CONTEXT_TERMINATED: c_int = 6;
/// `PA_CONTEXT_NOAUTOSPAWN` — не поднимать демон PulseAudio автоматически.
pub const PA_CONTEXT_NOAUTOSPAWN: c_int = 0x0001;

const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

/// `pa_sample_spec`: формат — enum (4 байта), затем частота и каналы.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PaSampleSpec {
    pub format: c_int,
    pub rate: u32,
    pub channels: u8,
}

/// `pa_channel_map`: число каналов + позиции (enum-массив из 32 элементов).
#[repr(C)]
#[derive(Clone, Copy)]
struct PaChannelMap {
    channels: u8,
    map: [c_int; 32],
}

/// `pa_cvolume`: каналы (uint8) + значения громкости (uint32 × 32).
#[repr(C)]
#[derive(Clone, Copy)]
struct PaCVolume {
    channels: u8,
    values: [u32; 32],
}

/// `pa_source_info` — раскладка 1:1 с `pulse/introspect.h` (лишние поля — указатели/скаляры).
#[repr(C)]
#[derive(Clone, Copy)]
struct PaSourceInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    owner_module: u32,
    volume: PaCVolume,
    mute: c_int,
    monitor_of_sink: u32,
    monitor_of_sink_name: *const c_char,
    latency: u64,
    driver: *const c_char,
    flags: c_int,
    proplist: *mut c_void,
    configured_latency: u64,
    base_volume: u32,
    state: c_int,
    n_volume_steps: u32,
    card: u32,
    n_ports: u32,
    ports: *mut c_void,
    active_port: *mut c_void,
    n_formats: u32,
    formats: *mut c_void,
}

/// `pa_sink_info` — те же начальные поля, что у источника (сверено с заголовками).
#[repr(C)]
#[derive(Clone, Copy)]
struct PaSinkInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    owner_module: u32,
    volume: PaCVolume,
    mute: c_int,
    monitor_source: u32,
    monitor_source_name: *const c_char,
    latency: u64,
    driver: *const c_char,
    flags: c_int,
    proplist: *mut c_void,
    configured_latency: u64,
    base_volume: u32,
    state: c_int,
    n_volume_steps: u32,
    card: u32,
    n_ports: u32,
    ports: *mut c_void,
    active_port: *mut c_void,
    n_formats: u32,
    formats: *mut c_void,
}

// Контроль соответствия ABI: размеры сверены с системными заголовками pulse 17
// (компиляция C-пробы sizeof/offsetof); при смене ABI pulse эти ассерты
// остановят сборку вместо молчалирого segfault.
const _: () = assert!(std::mem::size_of::<PaSourceInfo>() == 416);
const _: () = assert!(std::mem::size_of::<PaSinkInfo>() == 416);
const _: () = assert!(std::mem::size_of::<PaChannelMap>() == 132);
const _: () = assert!(std::mem::size_of::<PaCVolume>() == 132);
const _: () = assert!(std::mem::size_of::<PaSampleSpec>() == 12);
const _: () = assert!(std::mem::size_of::<PaBufferAttr>() == 20);

/// `pa_buffer_attr`: параметры буферизации (раскладка 1:1 с pulse/def.h).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PaBufferAttr {
    pub maxlength: u32,
    pub tlength: u32,
    pub prebuf: u32,
    pub minreq: u32,
    pub fragsize: u32,
}

/// `pa_server_info` — имена устройств по умолчанию (раскладка 1:1 с introspect.h).
#[repr(C)]
struct PaServerInfo {
    user_name: *const c_char,
    host_name: *const c_char,
    server_version: *const c_char,
    server_name: *const c_char,
    sample_spec: PaSampleSpec,
    default_sink_name: *const c_char,
    default_source_name: *const c_char,
    cookie: u32,
    channel_map: PaChannelMap,
}
const _: () = assert!(std::mem::size_of::<PaServerInfo>() == 200);

type PaSimpleNew = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const PaSampleSpec,
    *const PaChannelMap,
    *const c_void,
    *mut c_int,
) -> *mut c_void;
// pa_simple_read возвращает int (0 = успех, < 0 = ошибка) и заполняет весь буфер.
type PaSimpleRead = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type PaSimpleFree = unsafe extern "C" fn(*mut c_void);
type PaStrerror = unsafe extern "C" fn(c_int) -> *const c_char;

type PaMainloopNew = unsafe extern "C" fn() -> *mut c_void;
type PaMainloopGetApi = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type PaMainloopIterate = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type PaMainloopQuit = unsafe extern "C" fn(*mut c_void, c_int);
type PaMainloopFree = unsafe extern "C" fn(*mut c_void);

type PaContextNew = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
type PaContextConnect =
    unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *const c_void) -> c_int;
type PaContextSetStateCallback =
    unsafe extern "C" fn(*mut c_void, Option<PaContextNotifyCb>, *mut c_void);
type PaContextGetState = unsafe extern "C" fn(*mut c_void) -> c_int;
type PaContextDisconnect = unsafe extern "C" fn(*mut c_void);
type PaContextUnref = unsafe extern "C" fn(*mut c_void);
type PaOperationUnref = unsafe extern "C" fn(*mut c_void);
type PaContextGetSourceInfoList =
    unsafe extern "C" fn(*mut c_void, Option<PaSourceInfoCb>, *mut c_void) -> *mut c_void;
type PaContextGetSinkInfoList =
    unsafe extern "C" fn(*mut c_void, Option<PaSinkInfoCb>, *mut c_void) -> *mut c_void;
type PaContextGetServerInfo =
    unsafe extern "C" fn(*mut c_void, Option<PaServerInfoCb>, *mut c_void) -> *mut c_void;

type PaContextNotifyCb = unsafe extern "C" fn(*mut c_void, *mut c_void);
type PaSourceInfoCb = unsafe extern "C" fn(*mut c_void, *const PaSourceInfo, c_int, *mut c_void);
type PaSinkInfoCb = unsafe extern "C" fn(*mut c_void, *const PaSinkInfo, c_int, *mut c_void);
type PaServerInfoCb = unsafe extern "C" fn(*mut c_void, *const PaServerInfo, *mut c_void);

/// Загруженная библиотека PulseAudio: основной API из `libpulse.so.0`,
/// простой потоковый API `pa_simple_*` — из `libpulse-simple.so.0`.
pub struct PulseLib {
    handle: *mut c_void,
    simple_handle: *mut c_void,
    simple_new: PaSimpleNew,
    simple_read: PaSimpleRead,
    simple_free: PaSimpleFree,
    strerror: PaStrerror,
    mainloop_new: PaMainloopNew,
    mainloop_get_api: PaMainloopGetApi,
    mainloop_iterate: PaMainloopIterate,
    mainloop_quit: PaMainloopQuit,
    mainloop_free: PaMainloopFree,
    context_new: PaContextNew,
    context_connect: PaContextConnect,
    context_set_state_callback: PaContextSetStateCallback,
    context_get_state: PaContextGetState,
    context_disconnect: PaContextDisconnect,
    context_unref: PaContextUnref,
    operation_unref: PaOperationUnref,
    context_get_source_info_list: PaContextGetSourceInfoList,
    context_get_sink_info_list: PaContextGetSinkInfoList,
    context_get_server_info: PaContextGetServerInfo,
}

unsafe impl Send for PulseLib {}
unsafe impl Sync for PulseLib {}

/// Ошибка загрузки библиотеки или вызова PulseAudio.
#[derive(Debug, Clone)]
pub struct PulseError(pub String);

impl std::fmt::Display for PulseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PulseError {}

// Типы полей заданы в структуре PulseLib; clippy не видит их через macro_rules!.
#[allow(clippy::missing_transmute_annotations)]
macro_rules! sym {
    ($lib:expr, $name:literal) => {{
        let cname = CString::new($name).expect("имя символа без нуля");
        let ptr = unsafe { dlsym($lib, cname.as_ptr()) };
        if ptr.is_null() {
            return Err(PulseError(format!("в libpulse нет символа {}", $name)));
        }
        unsafe { std::mem::transmute(ptr) }
    }};
}

/// Открыть библиотеку по списку имён; `None`, если ни одно не нашлось.
fn try_dlopen(names: &[&str]) -> Option<*mut c_void> {
    for name in names {
        let cname = CString::new(*name).expect("имя библиотеки без нуля");
        let handle = unsafe { dlopen(cname.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if !handle.is_null() {
            return Some(handle);
        }
    }
    None
}

impl PulseLib {
    /// Загрузить `libpulse.so.0` + `libpulse-simple.so.0` (и запасные имена).
    #[allow(clippy::missing_transmute_annotations)] // тип задан полем структуры
    fn load() -> Result<Self, PulseError> {
        let handle = try_dlopen(&["libpulse.so.0", "libpulse.so"])
            .ok_or_else(|| PulseError("не удалось загрузить libpulse.so.0".into()))?;
        // pa_simple_* живут в отдельной библиотеке-обёртке; без неё захват невозможен.
        let (simple_handle, simple_new, simple_read, simple_free): (
            *mut c_void,
            PaSimpleNew,
            PaSimpleRead,
            PaSimpleFree,
        ) = match try_dlopen(&["libpulse-simple.so.0", "libpulse-simple.so"]) {
            Some(sh) => (
                sh,
                sym!(sh, "pa_simple_new"),
                sym!(sh, "pa_simple_read"),
                sym!(sh, "pa_simple_free"),
            ),
            None => {
                return Err(PulseError(
                    "не удалось загрузить libpulse-simple.so.0".into(),
                ))
            }
        };
        Ok(Self {
            handle,
            simple_handle,
            simple_new,
            simple_read,
            simple_free,
            strerror: sym!(handle, "pa_strerror"),
            mainloop_new: sym!(handle, "pa_mainloop_new"),
            mainloop_get_api: sym!(handle, "pa_mainloop_get_api"),
            mainloop_iterate: sym!(handle, "pa_mainloop_iterate"),
            mainloop_quit: sym!(handle, "pa_mainloop_quit"),
            mainloop_free: sym!(handle, "pa_mainloop_free"),
            context_new: sym!(handle, "pa_context_new"),
            context_connect: sym!(handle, "pa_context_connect"),
            context_set_state_callback: sym!(handle, "pa_context_set_state_callback"),
            context_get_state: sym!(handle, "pa_context_get_state"),
            context_disconnect: sym!(handle, "pa_context_disconnect"),
            context_unref: sym!(handle, "pa_context_unref"),
            operation_unref: sym!(handle, "pa_operation_unref"),
            context_get_source_info_list: sym!(handle, "pa_context_get_source_info_list"),
            context_get_sink_info_list: sym!(handle, "pa_context_get_sink_info_list"),
            context_get_server_info: sym!(handle, "pa_context_get_server_info"),
        })
    }
}

impl Drop for PulseLib {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.simple_handle);
            dlclose(self.handle);
        }
    }
}

/// Загруженная библиотека (один раз на процесс).
pub fn lib() -> Result<&'static PulseLib, PulseError> {
    static LIB: OnceLock<Result<PulseLib, PulseError>> = OnceLock::new();
    LIB.get_or_init(PulseLib::load)
        .as_ref()
        .map_err(|e| e.clone())
}

/// Описание источника из PulseAudio.
#[derive(Debug, Clone, Default)]
pub struct SourceEntry {
    pub name: String,
    pub description: String,
    pub muted: bool,
    pub monitor: bool,
}

/// Описание выхода из PulseAudio.
#[derive(Debug, Clone, Default)]
pub struct SinkEntry {
    pub name: String,
    pub description: String,
    pub muted: bool,
}

/// Результат опроса сервера: источники, выходы и значения по умолчанию.
#[derive(Debug, Clone, Default)]
pub struct PulseDevices {
    pub sources: Vec<SourceEntry>,
    pub sinks: Vec<SinkEntry>,
    pub default_source: Option<String>,
    pub default_sink: Option<String>,
}

/// Счётчик источников/выходов, собираемый в колбэках обхода.
#[derive(Default)]
struct CollectState {
    sources: Vec<SourceEntry>,
    sinks: Vec<SinkEntry>,
    /// Флаг «список источников получен целиком».
    sources_done: bool,
    /// Флаг «список выходов получен целиком».
    sinks_done: bool,
}

/// Состояние обхода сервера PulseAudio.
struct ContextState {
    ctx: *mut c_void,
    mainloop: *mut c_void,
    requested: bool,
    collect: CollectState,
    /// Имена устройств по умолчанию из `pa_server_info`.
    default_source: Option<String>,
    default_sink: Option<String>,
    /// Флаг «server_info получен».
    server_done: bool,
}

/// Все ли ответы сервера собраны (можно останавливать mainloop).
fn all_done(state: &ContextState) -> bool {
    state.collect.sources_done && state.collect.sinks_done && state.server_done
}

unsafe extern "C" fn on_source(
    _ctx: *mut c_void,
    info: *const PaSourceInfo,
    eol: c_int,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &mut *(userdata as *mut ContextState) };
    if eol != 0 {
        state.collect.sources_done = true;
        if all_done(state) {
            quit_mainloop(state);
        }
        return;
    }
    if info.is_null() {
        return;
    }
    let info = unsafe { &*info };
    let name = cstr_to_string(info.name);
    let description = cstr_to_string(info.description);
    // В PulseAudio PA_INVALID_INDEX == u32::MAX (-1). Если это не монитор, monitor_of_sink равен PA_INVALID_INDEX.
    let is_monitor_sink = info.monitor_of_sink != u32::MAX && !info.monitor_of_sink_name.is_null();
    let monitor = is_monitor_sink || name.ends_with(".monitor");
    state.collect.sources.push(SourceEntry {
        name,
        description,
        muted: info.mute != 0,
        monitor,
    });
}

unsafe extern "C" fn on_sink(
    _ctx: *mut c_void,
    info: *const PaSinkInfo,
    eol: c_int,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &mut *(userdata as *mut ContextState) };
    if eol != 0 {
        state.collect.sinks_done = true;
        if all_done(state) {
            quit_mainloop(state);
        }
        return;
    }
    if info.is_null() {
        return;
    }
    let info = unsafe { &*info };
    state.collect.sinks.push(SinkEntry {
        name: cstr_to_string(info.name),
        description: cstr_to_string(info.description),
        muted: info.mute != 0,
    });
}

/// Ответ `pa_context_get_server_info`: имена устройств по умолчанию.
unsafe extern "C" fn on_server_info(
    _ctx: *mut c_void,
    info: *const PaServerInfo,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &mut *(userdata as *mut ContextState) };
    if !info.is_null() {
        let info = unsafe { &*info };
        let sink = cstr_to_string(info.default_sink_name);
        let source = cstr_to_string(info.default_source_name);
        state.default_sink = (!sink.is_empty()).then_some(sink);
        state.default_source = (!source.is_empty()).then_some(source);
    }
    state.server_done = true;
    if all_done(state) {
        quit_mainloop(state);
    }
}

/// Остановить обход сервера.
fn quit_mainloop(state: &mut ContextState) {
    if let Ok(lib) = lib() {
        unsafe { (lib.mainloop_quit)(state.mainloop, 0) };
    }
}

unsafe extern "C" fn on_state(_ctx: *mut c_void, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &mut *(userdata as *mut ContextState) };
    let Ok(lib) = lib() else { return };
    let st = unsafe { (lib.context_get_state)(state.ctx) };
    if st == PA_CONTEXT_READY && !state.requested {
        state.requested = true;
        unsafe {
            let op = (lib.context_get_source_info_list)(state.ctx, Some(on_source), userdata);
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
            let op = (lib.context_get_sink_info_list)(state.ctx, Some(on_sink), userdata);
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
            let op = (lib.context_get_server_info)(state.ctx, Some(on_server_info), userdata);
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
        }
    } else if st == PA_CONTEXT_FAILED || st == PA_CONTEXT_TERMINATED {
        unsafe { (lib.mainloop_quit)(state.mainloop, 1) };
    }
}

/// Строка C → Rust (NULL безопасен).
fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string()
}

/// Поток записи PulseAudio (`pa_simple`).
///
/// Указатель непрозрачен; `pa_simple` не потокобезопасен, поэтому владение
/// переносится в поток захвата один раз (`Send`), без `Copy`/`Sync`.
pub struct StreamHandle(pub *mut c_void);

unsafe impl Send for StreamHandle {}

impl PulseLib {
    /// Прочитать список устройств у сервера PulseAudio.
    pub fn devices(&self) -> Result<PulseDevices, PulseError> {
        unsafe {
            let mainloop = (self.mainloop_new)();
            if mainloop.is_null() {
                return Err(PulseError("не удалось создать mainloop PulseAudio".into()));
            }
            let api = (self.mainloop_get_api)(mainloop);
            let name = CString::new("podtekst").expect("имя клиента без нуля");
            let ctx = (self.context_new)(api, name.as_ptr());
            if ctx.is_null() {
                (self.mainloop_free)(mainloop);
                return Err(PulseError("не удалось создать контекст PulseAudio".into()));
            }
            let mut state = ContextState {
                ctx,
                mainloop,
                requested: false,
                collect: CollectState::default(),
                default_source: None,
                default_sink: None,
                server_done: false,
            };
            (self.context_set_state_callback)(
                ctx,
                Some(on_state),
                &mut state as *mut ContextState as *mut c_void,
            );
            let rc = (self.context_connect)(
                ctx,
                std::ptr::null(),
                PA_CONTEXT_NOAUTOSPAWN,
                std::ptr::null(),
            );
            if rc < 0 {
                (self.context_unref)(ctx);
                (self.mainloop_free)(mainloop);
                return Err(PulseError(format!(
                    "не удалось подключиться к PulseAudio ({rc})"
                )));
            }
            // Обход с предельным временем: mainloop в неблокирующем режиме,
            // иначе iterate(block=1) может повиснуть на замолчавшем сервере.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
            loop {
                let iter = (self.mainloop_iterate)(mainloop, 0);
                if iter < 0 {
                    break; // pa_mainloop_quit или ошибка цикла
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            (self.context_disconnect)(ctx);
            (self.context_unref)(ctx);
            (self.mainloop_free)(mainloop);
            Ok(PulseDevices {
                sources: state.collect.sources,
                sinks: state.collect.sinks,
                default_source: state.default_source,
                default_sink: state.default_sink,
            })
        }
    }

    /// Открыть поток записи с устройства (моно, s16le, указанная частота).
    ///
    /// `device` = `None` — устройство по умолчанию; строка — имя источника
    /// (для системного звука это монитор выхода `<sink>.monitor`).
    pub fn open_capture(
        &self,
        device: Option<&str>,
        stream_name: &str,
        rate: u32,
    ) -> Result<StreamHandle, PulseError> {
        let spec = PaSampleSpec {
            format: PA_SAMPLE_S16LE,
            rate,
            channels: 1,
        };
        let app = CString::new("podtekst").expect("имя приложения без нуля");
        let dev = device.map(|d| CString::new(d).unwrap_or_default());
        let stream = CString::new(stream_name).expect("имя потока без нуля");
        let mut err: c_int = 0;
        let attr = PaBufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: 1024,
        };
        let handle = unsafe {
            (self.simple_new)(
                std::ptr::null(),
                app.as_ptr(),
                PA_STREAM_RECORD,
                dev.as_ref().map(|d| d.as_ptr()).unwrap_or(std::ptr::null()),
                stream.as_ptr(),
                &spec,
                std::ptr::null(),
                &attr as *const _ as *const c_void,
                &mut err,
            )
        };
        if handle.is_null() {
            let msg = unsafe { cstr_to_string((self.strerror)(err)) };
            return Err(PulseError(format!(
                "PulseAudio не открыл поток «{stream_name}»{}: {msg}",
                device.map(|d| format!(" ({d})")).unwrap_or_default()
            )));
        }
        Ok(StreamHandle(handle))
    }

    /// Прочитать сырые байты из потока (blocking).
    pub fn read(&self, handle: &StreamHandle, buf: &mut [u8]) -> Result<usize, PulseError> {
        let mut err: c_int = 0;
        let n = unsafe {
            (self.simple_read)(
                handle.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut err,
            )
        };
        if n < 0 {
            let msg = unsafe { cstr_to_string((self.strerror)(err)) };
            return Err(PulseError(format!(
                "чтение потока PulseAudio не удалось: {msg}"
            )));
        }
        // Успех = весь запрошенный буфер заполнен (blocking-чтение pa_simple).
        Ok(buf.len())
    }

    /// Закрыть поток.
    pub fn close(&self, handle: StreamHandle) {
        if !handle.0.is_null() {
            unsafe { (self.simple_free)(handle.0) };
        }
    }
}
