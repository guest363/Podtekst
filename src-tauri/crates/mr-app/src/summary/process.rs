use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::app_data_dir;

/// PID-файл sidecar: без него SIGKILL родителя оставил бы сироту с моделью
/// в 5.4 ГиБ до перезагрузки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    /// Платформенное время старта: clock ticks (Linux), timeval (macOS),
    /// FILETIME (Windows). Сравнивается точно, иначе чужой PID не трогаем.
    pub start_time: String,
    pub port: u16,
    pub model_sha256: String,
    pub token_hash: String,
}

pub fn summary_dir() -> PathBuf {
    app_data_dir().join("summary")
}

fn pid_file() -> PathBuf {
    summary_dir().join("sidecar.pid.json")
}

pub fn write_record(record: &PidRecord) -> std::io::Result<()> {
    let path = pid_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    crate::fsutil::write_atomic_private(&path, &text)
}

pub fn remove_record() {
    let _ = std::fs::remove_file(pid_file());
}

pub fn read_record() -> Option<PidRecord> {
    let text = std::fs::read_to_string(pid_file()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Часть `/proc/<pid>/stat` после `comm`: поле 22 (starttime) лежит по
/// индексу 19, потому что поля 1–2 уже разобраны.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn parse_linux_starttime(stat: &str) -> Option<u64> {
    let close = stat.rfind(')')?;
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    rest.nth(19)?.parse().ok()
}

pub fn start_time(pid: u32) -> Option<String> {
    start_time_impl(pid)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn start_time_impl(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_linux_starttime(&stat).map(|v| v.to_string())
}

#[cfg(target_os = "macos")]
fn start_time_impl(pid: u32) -> Option<String> {
    // libc на Apple не экспортирует kinfo_proc; proc_pidinfo (macOS-only)
    // даёт pbi_start_tvsec/usec без парсинга sysctl.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo заполняет структуру известного размера.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if written <= 0 {
        return None;
    }
    Some(format!(
        "{}.{:06}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(windows)]
fn start_time_impl(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: handle закрывается, все структуры инициализированы.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation = std::mem::zeroed();
        let mut exit = std::mem::zeroed();
        let mut kernel = std::mem::zeroed();
        let mut user = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        Some(ticks.to_string())
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn start_time_impl(_pid: u32) -> Option<String> {
    None
}

#[cfg(target_os = "ios")]
fn start_time_impl(_pid: u32) -> Option<String> {
    None
}

fn process_name(pid: u32) -> Option<String> {
    process_name_impl(pid)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn process_name_impl(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn process_name_impl(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: буфер и его длина валидны.
    let n = unsafe {
        libc::proc_name(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).to_string())
}

#[cfg(windows)]
fn process_name_impl(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: буфер и длина валидны, handle закрывается.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn process_name_impl(_pid: u32) -> Option<String> {
    None
}

/// Убиваем только свой sidecar: имя `llama-server` и точное время старта.
pub fn should_reap(pid: u32, recorded_start: &str) -> bool {
    let name = process_name(pid).map(|n| n.to_ascii_lowercase());
    let name_ok = name
        .as_deref()
        .is_some_and(|n| n == "llama-server" || n == "llama-server.exe");
    if !name_ok {
        return false;
    }
    start_time(pid).is_some_and(|now| now == recorded_start)
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    // SAFETY: отправляем SIGKILL конкретному PID; проверка имени уже прошла.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    // SAFETY: handle закрывается.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn kill_pid(_pid: u32) {}

/// Старт приложения: подчистить сироту прошлого запуска. Если время старта
/// не читается или не совпало — файл удаляем, процесс не трогаем.
pub fn cleanup_orphans() {
    let Some(record) = read_record() else {
        return;
    };
    if should_reap(record.pid, &record.start_time) {
        kill_pid(record.pid);
    }
    remove_record();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn stat_starttime_survives_spaces_and_parens_in_comm() {
        // Поле 22 — 999; после `comm` идут поля 3.., поэтому индекс 19.
        let mut tokens: Vec<String> = (3..=22)
            .map(|i| {
                if i == 22 {
                    "999".to_string()
                } else {
                    format!("f{i}")
                }
            })
            .collect();
        tokens.extend((23..=30).map(|i| format!("f{i}")));
        let stat = format!("42 (pod tekst (x)) {}", tokens.join(" "));
        assert_eq!(parse_linux_starttime(&stat), Some(999));
        assert_eq!(parse_linux_starttime("broken"), None);
    }

    #[test]
    fn current_process_has_start_time() {
        #[cfg(target_os = "linux")]
        {
            let pid = std::process::id();
            let first = start_time(pid).expect("starttime текущего процесса");
            let second = start_time(pid).expect("starttime текущего процесса");
            assert_eq!(first, second);
        }
    }

    #[test]
    fn foreign_or_forged_process_is_not_reaped() {
        assert!(!should_reap(std::process::id(), "not-the-real-start"));
        assert!(!should_reap(999_999_999, "1"));
    }
}
