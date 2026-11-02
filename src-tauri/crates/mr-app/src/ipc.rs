use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use crate::recording::{start_recording_with_notify, stop_recording_with_notify, RecordingState};

pub(crate) const IPC_PORT: u16 = 48291;

/// Ответы, которыми отвечает только наш инстанс. Порт 48291 может занять
/// значит нашего приложения на порту нет.
const IPC_REPLIES: &[&str] = &["STOPPED", "STARTED", "OK", "UNKNOWN"];

pub(crate) fn send_ipc(cmd: &str) -> Option<String> {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{IPC_PORT}")).ok()?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
    stream.write_all(format!("{cmd}\n").as_bytes()).ok()?;
    // Читаем до перевода строки: одиночный read() мог получить половину ответа.
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).ok()? == 1 {
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 256 {
            break;
        }
    }
    let reply = String::from_utf8_lossy(&buf).trim().to_string();
    // `ERROR: ...` — ответ нашего инстанса (например, микрофон занят):
    // CLI должен показать его, а не запускать второй экземпляр.
    (IPC_REPLIES.contains(&reply.as_str()) || reply.starts_with("ERROR:")).then_some(reply)
}

/// Одна строка протокола: успех — фиксированное слово, отказ — причина.
fn result_reply(res: &serde_json::Value, ok: &str) -> String {
    if res.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        return ok.to_string();
    }
    let err = res
        .get("error")
        .and_then(|x| x.as_str())
        .unwrap_or("не удалось");
    format!("ERROR: {}", err.replace(['\n', '\r'], " "))
}

/// Окно живёт в Tauri-слое, а IPC — в ядре: связываем их callback-ом.
pub(crate) type UiIpcCallback = Box<dyn Fn(&str) + Send + Sync>;
static UI_IPC_CALLBACK: std::sync::OnceLock<Mutex<Option<UiIpcCallback>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "tauri-ui")]
pub(crate) fn set_ui_ipc_callback(cb: UiIpcCallback) {
    let slot = UI_IPC_CALLBACK.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(cb);
    }
}

pub(crate) fn call_ui_ipc(cmd: &str) -> bool {
    if let Some(slot) = UI_IPC_CALLBACK.get() {
        if let Ok(guard) = slot.lock() {
            if let Some(cb) = guard.as_ref() {
                cb(cmd);
                return true;
            }
        }
    }
    false
}

pub(crate) fn handle_ipc_client(
    mut stream: TcpStream,
    state: Arc<Mutex<RecordingState>>,
    _port: u16,
) {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    while stream.read(&mut byte).is_ok_and(|n| n == 1) {
        if byte[0] == b'\n' || buf.len() > 256 {
            break;
        }
        buf.push(byte[0]);
    }
    let cmd = String::from_utf8_lossy(&buf).trim().to_uppercase();
    let resp: String = match cmd.as_str() {
        "TOGGLE" => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if st.is_recording {
                result_reply(&stop_recording_with_notify(&mut st), "STOPPED")
            } else {
                result_reply(&start_recording_with_notify(&mut st), "STARTED")
            }
        }
        "START" => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            result_reply(&start_recording_with_notify(&mut st), "STARTED")
        }
        "STOP" => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            result_reply(&stop_recording_with_notify(&mut st), "STOPPED")
        }
        "SHOW" | "HIDE" | "QUIT" => {
            if call_ui_ipc(&cmd) {
                "OK".to_string()
            } else {
                "ERROR: окно недоступно в headless-режиме".to_string()
            }
        }
        "PING" => "OK".to_string(),
        _ => "UNKNOWN".to_string(),
    };
    let _ = stream.write_all(format!("{resp}\n").as_bytes());
}
