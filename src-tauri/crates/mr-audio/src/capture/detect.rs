use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionCapability {
    Process,
    DeviceOnly,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CaptureState {
    #[default]
    None,
    Paused,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAudioClient {
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub app_name: Option<String>,
    pub capture: CaptureState,
    pub has_active_render: bool,
}

pub fn detection_capability() -> DetectionCapability {
    #[cfg(target_os = "windows")]
    {
        DetectionCapability::Process
    }
    #[cfg(target_os = "linux")]
    {
        super::pulse_detect::detection_capability()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::detection_capability()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        DetectionCapability::Unsupported
    }
}

pub fn active_audio_clients() -> Vec<ActiveAudioClient> {
    #[cfg(target_os = "windows")]
    {
        win::active_audio_clients()
    }
    #[cfg(target_os = "linux")]
    {
        super::pulse_detect::active_audio_clients()
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::active_audio_clients()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

#[cfg_attr(
    not(any(target_os = "windows", target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
pub(super) fn has_matching_render(
    pid: Option<u32>,
    process: Option<&str>,
    render_pids: &HashSet<u32>,
    render_names: &HashSet<String>,
) -> bool {
    pid.is_some_and(|p| render_pids.contains(&p))
        || process.is_some_and(|name| render_names.contains(&name.to_ascii_lowercase()))
}

#[cfg_attr(
    not(any(target_os = "windows", target_os = "linux", target_os = "macos")),
    allow(dead_code)
)]
pub(super) fn merge_clients(raw: Vec<ActiveAudioClient>) -> Vec<ActiveAudioClient> {
    let mut out: Vec<ActiveAudioClient> = Vec::with_capacity(raw.len());
    for item in raw {
        if let Some(pid) = item.pid {
            if let Some(existing) = out.iter_mut().find(|c| c.pid == Some(pid)) {
                existing.capture = existing.capture.max(item.capture);
                existing.has_active_render |= item.has_active_render;
                if existing.process.is_none() {
                    existing.process = item.process;
                }
                if existing.app_name.is_none() {
                    existing.app_name = item.app_name;
                }
                continue;
            }
        }
        out.push(item);
    }
    out
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn exe_basename(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path).trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(target_os = "windows")]
mod win {
    use std::collections::{HashMap, HashSet};

    use windows::core::{Interface, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, S_OK};
    use windows::Win32::Media::Audio::{
        eCapture, eRender, AudioSessionStateActive, AudioSessionStateInactive, EDataFlow,
        IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
        DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoUninitialize, CLSCTX_ALL};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::{
        exe_basename, has_matching_render, merge_clients, ActiveAudioClient, CaptureState,
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FlowKind {
        Capture,
        Render,
    }

    struct HandleGuard(HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            // SAFETY: дескриптор открыт OpenProcess и закрывается ровно один раз.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) fn active_audio_clients() -> Vec<ActiveAudioClient> {
        let com_owned = super::super::windows::com_init();
        // Отдельная область: COM-объекты обязаны освободиться до CoUninitialize.
        let clients = unsafe { collect_clients_com() };
        if com_owned {
            // SAFETY: парный вызов к S_OK/S_FALSE из com_init на этом же потоке.
            unsafe { CoUninitialize() };
        }
        clients
    }

    unsafe fn collect_clients_com() -> Vec<ActiveAudioClient> {
        let Ok(en) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return Vec::new();
        };
        let self_pid = std::process::id();
        let capture_sessions = active_sessions_for_flow(&en, eCapture, FlowKind::Capture, self_pid);
        let render_sessions = active_sessions_for_flow(&en, eRender, FlowKind::Render, self_pid);
        if capture_sessions.is_empty() && render_sessions.is_empty() {
            return Vec::new();
        }

        let mut exe_cache: HashMap<u32, Option<String>> = HashMap::new();
        let mut resolve = |pid: u32| -> Option<String> {
            exe_cache
                .entry(pid)
                .or_insert_with(|| process_exe_name(pid))
                .clone()
        };

        let mut render_pids = HashSet::with_capacity(render_sessions.len());
        let mut render_names = HashSet::with_capacity(render_sessions.len());
        let mut clients = Vec::with_capacity(capture_sessions.len() + render_sessions.len());
        for (pid, _) in render_sessions {
            render_pids.insert(pid);
            let process = resolve(pid);
            if let Some(name) = process.as_deref() {
                render_names.insert(name.to_ascii_lowercase());
            }
            clients.push(ActiveAudioClient {
                pid: Some(pid),
                process,
                app_name: None,
                capture: CaptureState::None,
                has_active_render: true,
            });
        }

        for (pid, capture) in capture_sessions {
            let process = resolve(pid);
            let has_active_render =
                has_matching_render(Some(pid), process.as_deref(), &render_pids, &render_names);
            clients.push(ActiveAudioClient {
                pid: Some(pid),
                process,
                app_name: None,
                capture,
                has_active_render,
            });
        }
        merge_clients(clients)
    }

    unsafe fn active_sessions_for_flow(
        en: &IMMDeviceEnumerator,
        flow: EDataFlow,
        kind: FlowKind,
        self_pid: u32,
    ) -> Vec<(u32, CaptureState)> {
        let mut sessions_out = Vec::new();
        let Ok(coll) = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
            return sessions_out;
        };
        let Ok(dev_count) = coll.GetCount() else {
            return sessions_out;
        };
        for dev_idx in 0..dev_count {
            let Ok(dev) = coll.Item(dev_idx) else {
                continue;
            };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = mgr.GetSessionEnumerator() else {
                continue;
            };
            let Ok(sess_count) = sessions.GetCount() else {
                continue;
            };
            for sess_idx in 0..sess_count {
                let Ok(ctrl) = sessions.GetSession(sess_idx) else {
                    continue;
                };
                let capture_state = match (kind, ctrl.GetState().ok()) {
                    (_, Some(s)) if s == AudioSessionStateActive => CaptureState::Active,
                    (FlowKind::Capture, Some(s)) if s == AudioSessionStateInactive => {
                        CaptureState::Paused
                    }
                    _ => continue,
                };
                let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if ctrl2.IsSystemSoundsSession() == S_OK {
                    continue;
                }
                let Ok(pid) = ctrl2.GetProcessId() else {
                    continue;
                };
                if pid == 0 || pid == self_pid {
                    continue;
                }
                sessions_out.push((pid, capture_state));
            }
        }
        sessions_out
    }

    fn process_exe_name(pid: u32) -> Option<String> {
        // PROCESS_QUERY_LIMITED_INFORMATION открывает UWP/Store и повышенные процессы без прав админа.
        // SAFETY: OpenProcess без наследования; дескриптор закрывается HandleGuard.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let _guard = HandleGuard(handle);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: буфер UTF-16 длиной len, QueryFullProcessImageNameW пишет не больше len.
        unsafe {
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        exe_basename(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_exe_basename_from_windows_and_unix_paths() {
        assert_eq!(
            exe_basename(r"C:\Program Files\Zoom\bin\Zoom.exe").as_deref(),
            Some("Zoom.exe")
        );
        assert_eq!(exe_basename("/usr/bin/chrome").as_deref(), Some("chrome"));
        assert_eq!(
            exe_basename("ms-teams.exe").as_deref(),
            Some("ms-teams.exe")
        );
        assert_eq!(exe_basename(""), None);
        assert_eq!(exe_basename(r"C:\trailing\"), None);
    }

    #[test]
    fn render_correlation_matches_pid_or_case_insensitive_process() {
        let render_pids = HashSet::from([100u32]);
        let render_names = HashSet::from(["firefox.exe".to_string()]);

        assert!(has_matching_render(
            Some(100),
            None,
            &render_pids,
            &render_names
        ));
        assert!(has_matching_render(
            Some(200),
            Some("Firefox.exe"),
            &render_pids,
            &render_names
        ));
        assert!(!has_matching_render(
            Some(200),
            Some("chrome.exe"),
            &render_pids,
            &render_names
        ));
        assert!(!has_matching_render(
            None,
            None,
            &render_pids,
            &render_names
        ));
    }

    #[test]
    fn merge_clients_deduplicates_pid_and_unions_state() {
        let merged = merge_clients(vec![
            ActiveAudioClient {
                pid: Some(42),
                process: None,
                app_name: None,
                capture: CaptureState::Paused,
                has_active_render: false,
            },
            ActiveAudioClient {
                pid: Some(42),
                process: Some("Zoom.exe".into()),
                app_name: Some("Zoom".into()),
                capture: CaptureState::Active,
                has_active_render: true,
            },
            ActiveAudioClient {
                pid: None,
                process: None,
                app_name: None,
                capture: CaptureState::Active,
                has_active_render: false,
            },
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].pid, Some(42));
        assert_eq!(merged[0].process.as_deref(), Some("Zoom.exe"));
        assert_eq!(merged[0].app_name.as_deref(), Some("Zoom"));
        assert_eq!(merged[0].capture, CaptureState::Active);
        assert!(merged[0].has_active_render);
        assert_eq!(merged[1].pid, None);
    }

    #[test]
    fn capability_and_poll_smoke_never_include_self() {
        let cap = detection_capability();
        #[cfg(target_os = "windows")]
        assert_eq!(cap, DetectionCapability::Process);
        #[cfg(not(target_os = "windows"))]
        let _ = cap;

        let clients = active_audio_clients();
        let self_pid = std::process::id();
        assert!(clients.iter().all(|c| c.pid != Some(self_pid)));
    }
}
