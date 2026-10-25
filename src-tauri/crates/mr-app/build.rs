use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// pyke's macOS ONNX Runtime is a CoreML build; its Objective-C `@available`
/// checks reference `___isPlatformVersionAtLeast` from clang's compiler-rt.
/// rustc links with `-nodefaultlibs`, so add that archive on Apple targets.
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

fn main() {
    link_clang_rt_on_apple();
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=TARGET_CC");

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
