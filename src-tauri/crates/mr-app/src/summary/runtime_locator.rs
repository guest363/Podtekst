use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    build: String,
    commit: String,
    #[serde(default)]
    required_flags: Vec<String>,
}

fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("../../summary-runtime-manifest.json"))
            .expect("summary-runtime-manifest.json вшит и валиден")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub dir: PathBuf,
    pub exe: PathBuf,
    pub version: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    NotFound,
    Probe(String),
    VersionMismatch { version: String, commit: String },
    MissingFlags(Vec<String>),
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

#[cfg(test)]
fn target_key() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "macos-arm64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        "unsupported"
    }
}

pub fn triple() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        "unsupported"
    }
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in ["PODTEKST_SUMMARY_RUNTIME", "MR_SUMMARY_RUNTIME"] {
        if let Ok(value) = std::env::var(key) {
            if value.trim().is_empty() {
                continue;
            }
            let path = PathBuf::from(value);
            if path.is_file() {
                if let Some(parent) = path.parent() {
                    out.push(parent.to_path_buf());
                }
            } else {
                out.push(path);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            out.push(parent.join("runtime").join(triple()));
            out.push(parent.join("runtime"));
            // macOS .app: бинарник в Contents/MacOS, ресурсы — Contents/Resources.
            out.push(parent.join("../Resources/runtime").join(triple()));
        }
    }
    for key in ["PODTEKST_RESOURCE_DIR", "TAURI_RESOURCE_DIR"] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() {
                let base = PathBuf::from(dir);
                out.push(base.join("runtime").join(triple()));
                out.push(base.join("runtime"));
            }
        }
    }
    // Dev-запуск из репозитория: src-tauri/binaries/runtime без бандла.
    let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../src-tauri/binaries/runtime")
        .join(triple());
    out.push(dev.clone());
    out.push(dev.join(".."));
    out
}

pub fn locate() -> Result<&'static RuntimeInfo, &'static RuntimeError> {
    static CACHE: OnceLock<Result<RuntimeInfo, RuntimeError>> = OnceLock::new();
    CACHE.get_or_init(discover).as_ref()
}

fn discover() -> Result<RuntimeInfo, RuntimeError> {
    let manifest = manifest();
    for dir in candidate_dirs() {
        let exe = dir.join(exe_name());
        if !exe.is_file() {
            continue;
        }
        let probe = probe(&exe, &dir)?;
        if probe.build != manifest.build || probe.commit != manifest.commit {
            return Err(RuntimeError::VersionMismatch {
                version: probe.version,
                commit: probe.commit,
            });
        }
        if !probe.help_ok {
            return Err(RuntimeError::MissingFlags(manifest.required_flags.clone()));
        }
        return Ok(RuntimeInfo {
            dir,
            exe,
            version: manifest.version.clone(),
            commit: manifest.commit.clone(),
        });
    }
    Err(RuntimeError::NotFound)
}

struct Probe {
    version: String,
    build: String,
    commit: String,
    help_ok: bool,
}

fn probe_with_env(exe: &Path, dir: &Path) -> Command {
    let mut cmd = Command::new(exe);
    let lib_key = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let prev = std::env::var(lib_key).unwrap_or_default();
    let joined = if prev.is_empty() {
        dir.to_string_lossy().to_string()
    } else {
        format!("{}:{prev}", dir.to_string_lossy())
    };
    cmd.env(lib_key, joined);
    cmd
}

fn run_output(cmd: &mut Command) -> String {
    let Ok(output) = cmd.output() else {
        return String::new();
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn probe(exe: &Path, dir: &Path) -> Result<Probe, RuntimeError> {
    let version_text = run_output(probe_with_env(exe, dir).arg("--version"));
    if version_text.trim().is_empty() {
        return Err(RuntimeError::Probe("--version не ответил".to_string()));
    }
    let (_, build) = version_text
        .split_once("build ")
        .ok_or_else(|| RuntimeError::Probe("нет номера сборки".to_string()))?;
    let build: String = build.chars().take_while(|c| c.is_ascii_digit()).collect();
    let commit = version_text
        .split("commit ")
        .nth(1)
        .map(|tail| {
            tail.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .unwrap_or_default();
    let version = version_text
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let help_text = run_output(probe_with_env(exe, dir).arg("--help"));
    let missing: Vec<String> = manifest()
        .required_flags
        .iter()
        .filter(|flag| !help_text.contains(flag.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(RuntimeError::MissingFlags(missing));
    }
    Ok(Probe {
        version,
        build,
        commit,
        help_ok: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::config::RUNTIME_COMMIT;

    #[test]
    fn manifest_pin_matches_config() {
        let m = manifest();
        assert_eq!(m.build, "12182");
        assert_eq!(m.commit, RUNTIME_COMMIT);
        assert_eq!(m.version, "b12182");
    }

    #[test]
    fn target_key_is_known_on_supported_hosts() {
        #[cfg(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64")
        ))]
        assert!(matches!(
            target_key(),
            "linux-x86_64" | "macos-arm64" | "windows-x86_64"
        ));
        assert!(!exe_name().is_empty());
        assert!(!triple().is_empty());
    }

    #[test]
    fn missing_runtime_is_reported_not_panicking() {
        // Env пустой, dev-путь, скорее всего, отсутствует; главное — без паники.
        if std::env::var("PODTEKST_SUMMARY_RUNTIME").is_err()
            && std::env::var("MR_SUMMARY_RUNTIME").is_err()
        {
            let _ = locate();
        }
    }
}
