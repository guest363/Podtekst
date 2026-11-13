use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Deserialize;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

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

pub fn target_key() -> &'static str {
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
            // Linux bundle: Tauri кладёт ресурсы в usr/lib/<productName>
            // (AppImage: внутри примонтированного AppDir, deb/rpm: /usr/lib).
            #[cfg(target_os = "linux")]
            {
                out.push(parent.join("../lib/Podtekst/runtime").join(triple()));
                out.push(parent.join("../lib/podtekst/runtime").join(triple()));
                out.push(PathBuf::from("/usr/lib/Podtekst/runtime").join(triple()));
                out.push(PathBuf::from("/usr/lib/podtekst/runtime").join(triple()));
            }
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

pub fn locate() -> Result<&'static RuntimeInfo, RuntimeError> {
    static CACHE: OnceLock<RuntimeInfo> = OnceLock::new();
    if let Some(info) = CACHE.get() {
        return Ok(info);
    }
    let info = discover()?;
    Ok(CACHE.get_or_init(|| info))
}

fn discover() -> Result<RuntimeInfo, RuntimeError> {
    discover_from(candidate_dirs())
}

pub fn validate_dir(dir: &Path) -> Result<RuntimeInfo, RuntimeError> {
    let manifest = manifest();
    let exe = dir.join(exe_name());
    if !exe.is_file() {
        return Err(RuntimeError::NotFound);
    }
    let probe = probe(&exe, dir)?;
    if probe.build != manifest.build || probe.commit != manifest.commit {
        return Err(RuntimeError::VersionMismatch {
            version: probe.version,
            commit: probe.commit,
        });
    }
    if !probe.help_ok {
        return Err(RuntimeError::MissingFlags(manifest.required_flags.clone()));
    }
    Ok(RuntimeInfo {
        dir: dir.to_path_buf(),
        exe,
        version: manifest.version.clone(),
        commit: manifest.commit.clone(),
    })
}

/// `llama-server --list-devices`: единственный дешёвый способ отличить
/// рабочую GPU-сборку от установленной, но без драйвера.
pub fn list_devices(exe: &Path, dir: &Path) -> Result<String, RuntimeError> {
    list_devices_with_backend(exe, dir, None)
}

/// То же, но с подключённым внешним backend-плагином (`GGML_BACKEND_PATH`).
/// Плагин живёт в app data и не требует второй копии llama-server.
pub fn list_devices_with_backend(
    exe: &Path,
    dir: &Path,
    backend: Option<&Path>,
) -> Result<String, RuntimeError> {
    let mut cmd = probe_with_env(exe, dir);
    if let Some(backend) = backend {
        cmd.env("GGML_BACKEND_PATH", backend);
        if let Some(parent) = backend.parent() {
            let key = if cfg!(target_os = "macos") {
                "DYLD_LIBRARY_PATH"
            } else {
                "LD_LIBRARY_PATH"
            };
            let prev = std::env::var(key).unwrap_or_default();
            let joined = if prev.is_empty() {
                parent.to_string_lossy().to_string()
            } else {
                format!("{}:{prev}", parent.to_string_lossy())
            };
            cmd.env(key, joined);
        }
    }
    if backend.is_some() {
        // Тип Vulkan-устройства (`uma: 0|1`) есть только в debug-логе.
        cmd.arg("-v");
    }
    // Порядок важен: параметры разбираются слева направо, а `--list-devices`
    // завершает процесс до разбора следующих флагов.
    run_output(cmd.arg("--list-devices"), PROBE_TIMEOUT)
}

/// Один битый кандидат не должен скрывать рабочий runtime за ним:
/// идём по всем каталогам и возвращаем последнюю содержательную ошибку.
fn discover_from(dirs: Vec<PathBuf>) -> Result<RuntimeInfo, RuntimeError> {
    let mut last = RuntimeError::NotFound;
    for dir in dirs {
        if !dir.join(exe_name()).is_file() {
            continue;
        }
        match validate_dir(&dir) {
            Ok(info) => return Ok(info),
            Err(error) => last = error,
        }
    }
    Err(last)
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

fn run_output(cmd: &mut Command, timeout: Duration) -> Result<String, RuntimeError> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RuntimeError::Probe(e.to_string()))?;
    // Дренаж обоих pipe обязан идти одновременно с ожиданием: `--help`
    // llama-server больше 64 КиБ, и заполненный pipe иначе блокирует child
    // до timeout.
    let stdout = child
        .stdout
        .take()
        .map(|pipe| std::thread::spawn(move || read_all(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| std::thread::spawn(move || read_all(pipe)));
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = join_reader(stdout, Duration::from_millis(100));
                    let _ = join_reader(stderr, Duration::from_millis(100));
                    return Err(RuntimeError::Probe("probe timeout".to_string()));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_reader(stdout, Duration::from_millis(100));
                let _ = join_reader(stderr, Duration::from_millis(100));
                return Err(RuntimeError::Probe(e.to_string()));
            }
        }
    }
    let mut text = String::new();
    if let Some(buf) = join_reader(stdout, Duration::from_secs(1)) {
        text.push_str(&String::from_utf8_lossy(&buf));
    }
    if let Some(buf) = join_reader(stderr, Duration::from_secs(1)) {
        text.push_str(&String::from_utf8_lossy(&buf));
    }
    Ok(text)
}

fn read_all(mut pipe: impl Read) -> Vec<u8> {
    const CAP: usize = 4 * 1024 * 1024;
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // Drain всегда, храним только первые 4 МиБ: runaway probe
                // не должен съесть память, но и не должен заблокироваться.
                if out.len() < CAP {
                    let take = (CAP - out.len()).min(n);
                    out.extend_from_slice(&chunk[..take]);
                }
            }
        }
    }
    out
}

/// Join с ограничением: убитый probe может оставить grandchild, держащий
/// pipe, и бесконечный join снова сделал бы timeout неограниченным.
fn join_reader(
    handle: Option<std::thread::JoinHandle<Vec<u8>>>,
    wait: Duration,
) -> Option<Vec<u8>> {
    let handle = handle?;
    let deadline = Instant::now() + wait;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.join().ok()
}

fn probe(exe: &Path, dir: &Path) -> Result<Probe, RuntimeError> {
    let version_text = run_output(probe_with_env(exe, dir).arg("--version"), PROBE_TIMEOUT)?;
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
    let help_text = run_output(probe_with_env(exe, dir).arg("--help"), PROBE_TIMEOUT)?;
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mr-runtime-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn stub(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(exe_name());
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn matching_stub(dir: &Path) {
        let flags = manifest().required_flags.join(" ");
        stub(
            dir,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo 'version: b12182'\n  echo 'built with build 12182 commit {RUNTIME_COMMIT}'\nelse\n  echo '{flags}'\nfi\n"
            ),
        );
    }

    #[cfg(unix)]
    fn retry_text_busy<T>(
        mut call: impl FnMut() -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        // Параллельный тест может держать fd на свежем скрипте в момент
        // fork/exec другого child: ETXTBSY исчезает после его exec.
        for _ in 0..20 {
            match call() {
                Err(RuntimeError::Probe(ref error)) if error.contains("Text file busy") => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => return result,
            }
        }
        call()
    }

    #[cfg(unix)]
    #[test]
    fn bad_candidate_does_not_hide_valid_runtime() {
        let base = temp_dir("candidates");
        let bad = base.join("bad");
        stub(
            &bad,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'version: b1'; echo 'build 1 commit deadbeef'; else echo '--jinja'; fi\n",
        );
        let good = base.join("good");
        matching_stub(&good);
        let info = retry_text_busy(|| discover_from(vec![bad.clone(), good.clone()]))
            .expect("второй runtime не найден");
        assert_eq!(info.dir, good);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn hung_probe_is_bounded() {
        let base = temp_dir("hung");
        let exe = stub(&base.join("hung"), "#!/bin/sh\nsleep 60\n");
        let started = Instant::now();
        let error = retry_text_busy(|| {
            run_output(
                Command::new(&exe).arg("--version"),
                Duration::from_millis(150),
            )
        })
        .unwrap_err();
        assert!(matches!(error, RuntimeError::Probe(_)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "probe не прервался"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_bundle_resources_are_candidates() {
        let expected = format!("lib/Podtekst/runtime/{}", triple());
        assert!(candidate_dirs().iter().any(|dir| dir.ends_with(&expected)));
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
    #[cfg(unix)]
    #[test]
    fn large_help_output_is_drained_without_deadlock() {
        let base = temp_dir("large-help");
        let dir = base.join("large");
        let flags = manifest().required_flags.join(" ");
        stub(
            &dir,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo 'version: b12182'\n  echo 'built with build 12182 commit {RUNTIME_COMMIT}'\nelse\n  head -c 131072 /dev/zero | tr '\\0' 'x'\n  echo ''\n  echo '{flags}'\nfi\n"
            ),
        );
        let info = retry_text_busy(|| discover_from(vec![dir.clone()]))
            .expect("большой --help заблокировал probe");
        assert_eq!(info.dir, dir);
        let _ = std::fs::remove_dir_all(&base);
    }
}
