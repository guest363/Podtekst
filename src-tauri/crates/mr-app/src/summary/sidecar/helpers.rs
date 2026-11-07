use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use sha2::{Digest, Sha256};

use crate::summary::backend::BackendError;
use crate::summary::config::MODEL_ID;

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 256 бит из двух UUIDv4 (по 122 бита OS RNG) через SHA-256: токен не
/// попадает в argv, только в файл 0600 (M18).
pub(super) fn new_token() -> String {
    let seed = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    sha256_hex(seed.as_bytes())
}

pub(super) fn token_hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

pub(super) fn api_key_path() -> PathBuf {
    crate::summary::process::summary_dir().join("api-key.txt")
}

pub(super) fn write_token(path: &Path, token: &str) -> Result<(), BackendError> {
    crate::fsutil::write_atomic_private(path, format!("{token}\n").as_bytes())
        .map_err(|e| BackendError::Load(format!("token file: {e}")))
}

pub(super) fn free_port() -> Result<u16, BackendError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| BackendError::Load(format!("port: {e}")))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| BackendError::Load(format!("port: {e}")))
}

/// Пины §6: без GPU/mlock/-march; `--parallel 1` обязателен против дефолтных
/// четырёх слотов, раздувающих KV и RSS.
pub(super) fn build_args(
    model_path: &Path,
    port: u16,
    token_path: &Path,
    threads: u32,
    n_ctx: u32,
) -> Vec<String> {
    vec![
        "--model".to_string(),
        model_path.to_string_lossy().to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--parallel".to_string(),
        "1".to_string(),
        "--ctx-size".to_string(),
        n_ctx.to_string(),
        "-b".to_string(),
        "512".to_string(),
        "-ub".to_string(),
        "128".to_string(),
        "--threads".to_string(),
        threads.to_string(),
        "--temp".to_string(),
        "0".to_string(),
        "--top-k".to_string(),
        "1".to_string(),
        "--top-p".to_string(),
        "0.9".to_string(),
        "--repeat-penalty".to_string(),
        "1.1".to_string(),
        "--seed".to_string(),
        "42".to_string(),
        "--jinja".to_string(),
        "--no-webui".to_string(),
        "--reasoning".to_string(),
        "off".to_string(),
        "--api-key-file".to_string(),
        token_path.to_string_lossy().to_string(),
        "--log-disable".to_string(),
        "--alias".to_string(),
        MODEL_ID.to_string(),
    ]
}

pub(super) fn prepend_lib_path(cmd: &mut Command, dir: &Path) {
    let key = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let prev = std::env::var(key).unwrap_or_default();
    let joined = if prev.is_empty() {
        dir.to_string_lossy().to_string()
    } else {
        format!("{}:{prev}", dir.to_string_lossy())
    };
    cmd.env(key, joined);
}

#[cfg(windows)]
pub(super) fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(super) fn hide_window(_cmd: &mut Command) {}

pub(super) fn spawn_stderr(mut child: Child) -> (Child, std::thread::JoinHandle<(u64, String)>) {
    // stderr не логируется и не пишется на диск (H15): поток дочитывает его,
    // считает длину и SHA-256, а текст выбрасывает.
    let mut stderr = child.stderr.take();
    let handle = std::thread::spawn(move || {
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;
        if let Some(pipe) = stderr.as_mut() {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        bytes += n as u64;
                        hasher.update(&buf[..n]);
                    }
                }
            }
        }
        let digest = hasher.finalize();
        (bytes, sha256_hex(digest.as_slice()))
    });
    (child, handle)
}

pub(super) fn map_ureq(error: ureq::Error) -> BackendError {
    match error {
        ureq::Error::Status(code, _) => match code {
            401 | 403 => BackendError::Http("unauthorized".to_string()),
            503 => BackendError::Load(format!("status {code}")),
            500 => BackendError::Http("status 500".to_string()),
            _ => BackendError::Http(format!("status {code}")),
        },
        ureq::Error::Transport(_) => BackendError::Crashed,
    }
}

/// Убирает thinking и служебные токены chat-template (§6): JSON/GBNF не
/// используется, схема проверяется постфактум.
pub(super) fn sanitize_output(text: &str) -> String {
    let text = text
        .rsplit("</think>")
        .next()
        .unwrap_or(text)
        .replace("<|im_start|>", "")
        .replace("<|im_end|>", "")
        .replace(" thinking", "")
        .replace("</think>", "");
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_contain_pinned_flags() {
        let args = build_args(
            Path::new("/models/q.gguf"),
            8123,
            Path::new("/data/key"),
            6,
            8192,
        );
        for flag in [
            "--jinja",
            "--no-webui",
            "--reasoning",
            "off",
            "--api-key-file",
            "--log-disable",
            "--parallel",
            "1",
            "--ctx-size",
            "8192",
            "-b",
            "512",
            "-ub",
            "128",
            "--threads",
            "6",
        ] {
            assert!(args.iter().any(|a| a == flag), "нет {flag}");
        }
        assert!(!args.iter().any(|a| a == "--mlock"));
        let joined = args.join(" ");
        assert!(!joined.contains("Bearer"), "токен не должен быть в argv");
        assert!(!joined.contains("CANARY"));
    }

    #[test]
    fn token_is_256_bit_and_never_repeated() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_eq!(token_hash(&a).len(), 64);
        assert_ne!(token_hash(&a), token_hash(&b));
    }

    #[test]
    fn sanitize_strips_thinking_and_service_tokens() {
        let text = "preamble  thinkinginternal</think>## Кратко\n<|im_end|>ok";
        let clean = sanitize_output(text);
        assert!(!clean.contains("thinking"));
        assert!(!clean.contains("<|im_end|>"));
        assert!(clean.starts_with("## Кратко"));
    }
}
