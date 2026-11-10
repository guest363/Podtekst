use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ограничение macOS: сборка ONNX Runtime от pyke с CoreML обращается к
/// `___isPlatformVersionAtLeast` из clang compiler-rt. Так как rustc линкует с
/// `-nodefaultlibs`, на Apple-таргетах добавляем этот архив вручную.
fn link_clang_rt_on_apple() {
    if env::var("CARGO_CFG_TARGET_VENDOR").as_deref() != Ok("apple") {
        return;
    }
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let platform = match os.as_str() {
        "macos" => "osx",
        "ios" => {
            if env::var("CARGO_CFG_TARGET_ABI").as_deref() == Ok("sim") {
                "iossim"
            } else {
                "ios"
            }
        }
        _ => return,
    };
    let Some(dir) = resource_dir() else {
        println!("cargo:warning=clang resource dir not found; ONNX Runtime may fail to link");
        return;
    };
    let archive = dir.join(format!("libclang_rt.{platform}.a"));
    if !archive.is_file() {
        println!(
            "cargo:warning={} not found; ONNX Runtime may fail to link",
            archive.display()
        );
        return;
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static:-bundle=clang_rt.{platform}");
}

fn resource_dir() -> Option<PathBuf> {
    let target = env::var("TARGET").ok()?;
    let mut commands = vec![];
    for key in [
        format!("CC_{target}"),
        format!("CC_{}", target.replace('-', "_")),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ] {
        if let Ok(cc) = env::var(&key) {
            if !cc.is_empty() {
                commands.push(vec![cc]);
            }
        }
    }
    commands.push(vec!["cc".to_string()]);
    commands.push(vec!["xcrun".to_string(), "clang".to_string()]);
    commands.into_iter().find_map(|argv| {
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .arg("--print-resource-dir")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = Path::new(String::from_utf8(out.stdout).ok()?.trim()).join("lib/darwin");
        path.is_dir().then_some(path)
    })
}

/// Swift-мост ScreenCaptureKit лежит в /usr/lib/swift, а не в dyld-кэше:
/// без rpath приложение и тесты падают с «Library not loaded».
fn link_swift_runtime() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}

/// Command-line запуск (`run.command`) идёт мимо .app-бандла, а TCC требует
/// NSMicrophoneUsageDescription до первого обращения к микрофону. Вшиваем
/// Info.plist в секцию __TEXT бинарника — macOS читает её как у бандла.
fn embed_macos_info_plist() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let plist = Path::new(env!("CARGO_MANIFEST_DIR")).join("Info.plist");
    if !plist.is_file() {
        println!(
            "cargo:warning={} not found; microphone permission may be denied",
            plist.display()
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg-bin=podtekst=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}

fn git_output(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    link_clang_rt_on_apple();
    link_swift_runtime();
    embed_macos_info_plist();
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=TARGET_CC");

    // manifest.json dev-запуска хранит точку воспроизведения; git может
    // отсутствовать (сборка из архива), тогда честно пишем unknown.
    let commit = git_output(&["rev-parse", "--short", "HEAD"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = git_output(&["status", "--porcelain"])
        .map(|value| (!value.is_empty()).to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=PODTEKST_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PODTEKST_GIT_DIRTY={dirty}");

    // Tauri-метаданные (иконки, config) генерируются только в CI-сборке
    // с фичей `tauri-ui`, где доступен крейт tauri-build и системный WebView.
    // tauri.conf.json лежит рядом с пакетом (crates/mr-app/) — канон Tauri.
    #[cfg(feature = "tauri-ui")]
    {
        tauri_build::build()
    }
    #[cfg(not(feature = "tauri-ui"))]
    {
        println!("cargo:rerun-if-changed=../../ui/web/static/index.html");
    }
}
