#![cfg(target_os = "linux")]

use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::time::{Duration, Instant};

use super::detect::{has_matching_render, merge_clients, ActiveAudioClient, DetectionCapability};
use super::pulse::{
    self, cstr_to_string, PaCVolume, PaChannelMap, PaSampleSpec, PaSourceInfo, PulseLib,
    PA_CONTEXT_FAILED, PA_CONTEXT_NOAUTOSPAWN, PA_CONTEXT_READY, PA_CONTEXT_TERMINATED,
};

#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(super) struct PaSourceOutputInfo {
    index: u32,
    name: *const c_char,
    owner_module: u32,
    client: u32,
    source: u32,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    buffer_usec: u64,
    source_usec: u64,
    resample_method: *const c_char,
    driver: *const c_char,
    proplist: *mut c_void,
    corked: c_int,
    volume: PaCVolume,
    mute: c_int,
    has_volume: c_int,
    volume_writable: c_int,
    format: *mut c_void,
}

#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(super) struct PaSinkInputInfo {
    index: u32,
    name: *const c_char,
    owner_module: u32,
    client: u32,
    sink: u32,
    sample_spec: PaSampleSpec,
    channel_map: PaChannelMap,
    volume: PaCVolume,
    buffer_usec: u64,
    sink_usec: u64,
    resample_method: *const c_char,
    driver: *const c_char,
    mute: c_int,
    proplist: *mut c_void,
    corked: c_int,
    has_volume: c_int,
    volume_writable: c_int,
    format: *mut c_void,
}

// Страховка ABI против тихого сдвига смещений proplist/corked при смене заголовков libpulse.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<PaSourceOutputInfo>() == 376);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<PaSinkInputInfo>() == 376);

pub(super) type PaSourceOutputInfoCb =
    unsafe extern "C" fn(*mut c_void, *const PaSourceOutputInfo, c_int, *mut c_void);
pub(super) type PaSinkInputInfoCb =
    unsafe extern "C" fn(*mut c_void, *const PaSinkInputInfo, c_int, *mut c_void);

struct RawCapture {
    source: u32,
    client: u32,
    pid: Option<u32>,
    process: Option<String>,
    app_name: Option<String>,
}

struct RawRender {
    client: u32,
    pid: Option<u32>,
    process: Option<String>,
}

struct DetectState {
    ctx: *mut c_void,
    mainloop: *mut c_void,
    requested: bool,
    monitor_sources: HashSet<u32>,
    captures: Vec<RawCapture>,
    renders: Vec<RawRender>,
    sources_done: bool,
    outputs_done: bool,
    inputs_done: bool,
}

fn all_done(state: &DetectState) -> bool {
    state.sources_done && state.outputs_done && state.inputs_done
}

fn quit_mainloop(state: &mut DetectState) {
    if let Ok(lib) = pulse::lib() {
        // SAFETY: mainloop жив до конца active_mic_clients; колбэки вызываются из того же потока.
        unsafe { (lib.mainloop_quit)(state.mainloop, 0) };
    }
}

unsafe fn proplist_get(lib: &PulseLib, proplist: *const c_void, key: &CStr) -> Option<String> {
    if proplist.is_null() {
        return None;
    }
    // SAFETY: proplist валиден на время колбэка PulseAudio; строка заимствуется без free.
    let ptr = unsafe { (lib.proplist_gets)(proplist, key.as_ptr()) };
    let s = cstr_to_string(ptr);
    (!s.is_empty()).then_some(s)
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
    // SAFETY: userdata — указатель на DetectState на стеке активного вызова.
    let state = unsafe { &mut *(userdata as *mut DetectState) };
    if eol != 0 {
        state.sources_done = true;
        if all_done(state) {
            quit_mainloop(state);
        }
        return;
    }
    if info.is_null() {
        return;
    }
    // SAFETY: ненулевой info гарантирован контрактом pa_source_info_cb_t.
    let info = unsafe { &*info };
    let is_monitor = (info.monitor_of_sink != u32::MAX && !info.monitor_of_sink_name.is_null())
        || cstr_to_string(info.name).ends_with(".monitor");
    if is_monitor {
        state.monitor_sources.insert(info.index);
    }
}

unsafe extern "C" fn on_source_output(
    _ctx: *mut c_void,
    info: *const PaSourceOutputInfo,
    eol: c_int,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: userdata — указатель на DetectState на стеке активного вызова.
    let state = unsafe { &mut *(userdata as *mut DetectState) };
    if eol != 0 {
        state.outputs_done = true;
        if all_done(state) {
            quit_mainloop(state);
        }
        return;
    }
    if info.is_null() {
        return;
    }
    // SAFETY: ненулевой info гарантирован контрактом pa_source_output_info_cb_t.
    let info = unsafe { &*info };
    if info.corked != 0 {
        return;
    }
    let Ok(lib) = pulse::lib() else { return };
    // SAFETY: proplist внутри info валиден на время колбэка.
    let (pid, process, app_name) = unsafe {
        let pid = proplist_get(lib, info.proplist, c"application.process.id")
            .and_then(|s| s.parse::<u32>().ok());
        let process = proplist_get(lib, info.proplist, c"application.process.binary");
        let app_name = proplist_get(lib, info.proplist, c"application.name");
        (pid, process, app_name)
    };
    state.captures.push(RawCapture {
        source: info.source,
        client: info.client,
        pid,
        process,
        app_name,
    });
}

unsafe extern "C" fn on_sink_input(
    _ctx: *mut c_void,
    info: *const PaSinkInputInfo,
    eol: c_int,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: userdata — указатель на DetectState на стеке активного вызова.
    let state = unsafe { &mut *(userdata as *mut DetectState) };
    if eol != 0 {
        state.inputs_done = true;
        if all_done(state) {
            quit_mainloop(state);
        }
        return;
    }
    if info.is_null() {
        return;
    }
    // SAFETY: ненулевой info гарантирован контрактом pa_sink_input_info_cb_t.
    let info = unsafe { &*info };
    if info.corked != 0 {
        return;
    }
    let Ok(lib) = pulse::lib() else { return };
    // SAFETY: proplist внутри info валиден на время колбэка.
    let (pid, process) = unsafe {
        let pid = proplist_get(lib, info.proplist, c"application.process.id")
            .and_then(|s| s.parse::<u32>().ok());
        let process = proplist_get(lib, info.proplist, c"application.process.binary");
        (pid, process)
    };
    state.renders.push(RawRender {
        client: info.client,
        pid,
        process,
    });
}

unsafe extern "C" fn on_state(_ctx: *mut c_void, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: userdata — указатель на DetectState на стеке активного вызова.
    let state = unsafe { &mut *(userdata as *mut DetectState) };
    let Ok(lib) = pulse::lib() else { return };
    // SAFETY: ctx валиден, пока mainloop активен.
    let st = unsafe { (lib.context_get_state)(state.ctx) };
    if st == PA_CONTEXT_READY && !state.requested {
        state.requested = true;
        // SAFETY: три параллельных запроса к готовому контексту; операции сразу unref.
        unsafe {
            let op = (lib.context_get_source_info_list)(state.ctx, Some(on_source), userdata);
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
            let op = (lib.context_get_source_output_info_list)(
                state.ctx,
                Some(on_source_output),
                userdata,
            );
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
            let op =
                (lib.context_get_sink_input_info_list)(state.ctx, Some(on_sink_input), userdata);
            if !op.is_null() {
                (lib.operation_unref)(op);
            }
        }
    } else if st == PA_CONTEXT_FAILED || st == PA_CONTEXT_TERMINATED {
        // SAFETY: mainloop валиден до выхода из active_mic_clients.
        unsafe { (lib.mainloop_quit)(state.mainloop, 1) };
    }
}

pub(super) fn detection_capability() -> DetectionCapability {
    if pulse::lib().is_ok() {
        DetectionCapability::Process
    } else {
        DetectionCapability::Unsupported
    }
}

pub(super) fn active_mic_clients() -> Vec<ActiveAudioClient> {
    let Ok(lib) = pulse::lib() else {
        return Vec::new();
    };
    let state = query_pulse(lib);
    let self_pid = std::process::id();

    let mut render_pids = HashSet::new();
    let mut render_clients = HashSet::new();
    let mut render_bins = HashSet::new();
    for r in &state.renders {
        if r.pid == Some(self_pid) {
            continue;
        }
        if let Some(p) = r.pid {
            render_pids.insert(p);
        }
        if r.client != u32::MAX {
            render_clients.insert(r.client);
        }
        if let Some(b) = r.process.as_deref() {
            render_bins.insert(b.to_ascii_lowercase());
        }
    }

    let clients = state
        .captures
        .into_iter()
        .filter(|c| !state.monitor_sources.contains(&c.source))
        .filter(|c| c.pid != Some(self_pid))
        .map(|c| {
            // Совпадение по client-индексу закрывает Flatpak-приложения с изолированным PID.
            let has_active_render = (c.client != u32::MAX && render_clients.contains(&c.client))
                || has_matching_render(c.pid, c.process.as_deref(), &render_pids, &render_bins);
            ActiveAudioClient {
                pid: c.pid,
                process: c.process,
                app_name: c.app_name,
                has_active_render,
            }
        })
        .collect();
    merge_clients(clients)
}

fn query_pulse(lib: &PulseLib) -> DetectState {
    let mut state = DetectState {
        ctx: std::ptr::null_mut(),
        mainloop: std::ptr::null_mut(),
        requested: false,
        monitor_sources: HashSet::new(),
        captures: Vec::new(),
        renders: Vec::new(),
        sources_done: false,
        outputs_done: false,
        inputs_done: false,
    };
    // SAFETY: однопоточный жизненный цикл mainloop и контекста в пределах одного вызова.
    unsafe {
        let mainloop = (lib.mainloop_new)();
        if mainloop.is_null() {
            return state;
        }
        let api = (lib.mainloop_get_api)(mainloop);
        let name = CString::new("podtekst-detect").expect("имя клиента без нуля");
        let ctx = (lib.context_new)(api, name.as_ptr());
        if ctx.is_null() {
            (lib.mainloop_free)(mainloop);
            return state;
        }
        state.ctx = ctx;
        state.mainloop = mainloop;
        (lib.context_set_state_callback)(
            ctx,
            Some(on_state),
            &mut state as *mut DetectState as *mut c_void,
        );
        let rc = (lib.context_connect)(
            ctx,
            std::ptr::null(),
            PA_CONTEXT_NOAUTOSPAWN,
            std::ptr::null(),
        );
        if rc >= 0 {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let mut quit = false;
                loop {
                    let iter = (lib.mainloop_iterate)(mainloop, 0);
                    if iter < 0 {
                        quit = true;
                        break;
                    }
                    if iter == 0 {
                        break;
                    }
                }
                if quit || all_done(&state) || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            (lib.context_disconnect)(ctx);
        }
        (lib.context_unref)(ctx);
        (lib.mainloop_free)(mainloop);
    }
    state
}
