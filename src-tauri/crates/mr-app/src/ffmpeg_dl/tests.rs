use super::*;

fn probe(version: Option<&str>, source: &'static str, arch: Option<&'static str>) -> ToolProbe {
    ToolProbe {
        version: version.map(str::to_string),
        raw: version.unwrap_or("").to_string(),
        source,
        arch,
        path: PathBuf::from("ffmpeg"),
    }
}

fn state(version: &str, arch: &str) -> Installed {
    Installed {
        version: version.to_string(),
        arch: arch.to_string(),
        ..Installed::default()
    }
}

fn find_system_tool(names: &[&str]) -> Option<Vec<u8>> {
    let paths = std::env::var_os("PATH")?;
    for name in names {
        for dir in std::env::split_paths(&paths) {
            let path = dir.join(name);
            let Ok(body) = std::fs::read(&path) else {
                continue;
            };
            let runs = std::process::Command::new(&path)
                .arg("-version")
                .stdin(std::process::Stdio::null())
                .output()
                .map(|out| out.status.success() && !out.stdout.is_empty())
                .unwrap_or(false);
            if runs {
                return Some(body);
            }
        }
    }
    None
}

#[test]
fn can_update_follows_state_and_source() {
    let arch = std::env::consts::ARCH;
    let current = probe(Some("9.0.2"), "app", Some(arch));
    let other = probe(Some("7.1.1"), "app", Some(arch));
    let system = probe(Some("7.1.1"), "path", Some(arch));

    assert!(!needs_update(
        None,
        Some(&system),
        Some(&system),
        Some("9.0.2")
    ));
    assert!(!needs_update(
        Some(&state("9.0.2", arch)),
        Some(&current),
        Some(&current),
        Some("9.0.2")
    ));
    assert!(needs_update(
        None,
        Some(&other),
        Some(&other),
        Some("9.0.2")
    ));
    assert!(needs_update(
        Some(&state("7.1.1", arch)),
        Some(&current),
        Some(&current),
        Some("9.0.2")
    ));
    assert!(needs_update(
        Some(&state("9.0.2", "aarch64")),
        Some(&current),
        Some(&current),
        Some("9.0.2")
    ));
    assert!(needs_update(
        Some(&state("9.0.2", arch)),
        Some(&current),
        None,
        Some("9.0.2")
    ));
    assert!(!needs_update(None, None, None, Some("9.0.2")));
    assert!(!needs_update(None, Some(&current), Some(&current), None));
}

#[test]
fn pair_ok_compares_major_minor() {
    let a = probe(Some("9.0.2"), "app", Some("aarch64"));
    let b = probe(Some("9.0.4"), "app", Some("aarch64"));
    let old = probe(Some("8.0.2"), "app", Some("aarch64"));
    let nightly = probe(None, "app", Some("aarch64"));
    assert!(pair_ok(Some(&a), Some(&b)));
    assert!(!pair_ok(Some(&a), Some(&old)));
    assert!(!pair_ok(Some(&a), None));
    assert!(!pair_ok(Some(&nightly), Some(&a)));
    assert!(!pair_ok(None, Some(&a)));
}

#[test]
fn package_current_requires_state_and_files() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-cur-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let package = pins::Package {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: "9.0.2".to_string(),
        source: "test".to_string(),
        provides: vec!["ffmpeg".to_string(), "ffprobe".to_string()],
        format: "zip".to_string(),
        url: "https://example.invalid/a.zip".to_string(),
        sha256: None,
    };
    let current = state("9.0.2", std::env::consts::ARCH);
    assert!(!package_current(&package, &dir, Some(&current)));
    std::fs::write(dir.join(bin_name("ffmpeg")), b"x").unwrap();
    std::fs::write(dir.join(bin_name("ffprobe")), b"x").unwrap();
    assert!(package_current(&package, &dir, Some(&current)));
    assert!(!package_current(&package, &dir, None));
    assert!(!package_current(
        &package,
        &dir,
        Some(&state("7.1.1", std::env::consts::ARCH))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_checks_keep_the_working_binary() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-keep-{}", uuid::Uuid::new_v4()));
    let bin = dir.join("bin");
    let work = dir.join("work");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(bin.join(bin_name("ffmpeg")), b"old").unwrap();
    let archive = dir.join("a.zip");
    {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        writer
            .start_file("ffmpeg", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not a binary").unwrap();
        writer.finish().unwrap();
    }
    let mut package = pins::Package {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: "9.0.2".to_string(),
        source: "test".to_string(),
        provides: vec!["ffmpeg".to_string()],
        format: "zip".to_string(),
        url: "https://example.invalid/a.zip".to_string(),
        sha256: Some("0".repeat(64)),
    };
    let error = install_archive(&package, &archive, &bin, &work).expect_err("sha256 не совпадает");
    assert!(error.contains("контрольная сумма"), "{error}");
    assert_eq!(std::fs::read(bin.join(bin_name("ffmpeg"))).unwrap(), b"old");

    package.sha256 = Some(mr_models::sha256_of_file(&archive).unwrap());
    let error = install_archive(&package, &archive, &bin, &work).expect_err("не бинарник");
    assert!(
        error.contains("архитектура") || error.contains("не запускается"),
        "{error}"
    );
    assert_eq!(std::fs::read(bin.join(bin_name("ffmpeg"))).unwrap(), b"old");
    assert!(!bin.join(format!("{}.new", bin_name("ffmpeg"))).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "linux")]
#[test]
fn install_archive_publishes_running_binary() {
    // Нужен ELF, который печатает версию в stdout по `-version`.
    let Some(body) = find_system_tool(&["make", "unzip", "sqlite3"]) else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-ok-{}", uuid::Uuid::new_v4()));
    let bin = dir.join("bin");
    let work = dir.join("work");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let archive = dir.join("a.zip");
    {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        writer
            .start_file(
                "ffmpeg",
                SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
        writer.write_all(&body).unwrap();
        writer.finish().unwrap();
    }
    let package = pins::Package {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: "9.0.2".to_string(),
        source: "test".to_string(),
        provides: vec!["ffmpeg".to_string()],
        format: "zip".to_string(),
        url: "https://example.invalid/a.zip".to_string(),
        sha256: Some(mr_models::sha256_of_file(&archive).unwrap()),
    };
    let hashes = install_archive(&package, &archive, &bin, &work).expect("установка");
    assert!(bin.join(bin_name("ffmpeg")).is_file());
    assert_eq!(hashes.len(), 1);
    assert!(!bin.join(format!("{}.new", bin_name("ffmpeg"))).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_has_contract_keys() {
    let status = status_json();
    for key in [
        "available",
        "downloading",
        "pct",
        "error",
        "manual_url",
        "version",
        "version_raw",
        "ffprobe_version",
        "source",
        "arch",
        "expected_version",
        "can_update",
        "pair_ok",
    ] {
        assert!(status.get(key).is_some(), "нет поля {key}");
    }
}

#[test]
fn state_roundtrip_is_atomic() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-state-{}", uuid::Uuid::new_v4()));
    let state = Installed {
        version: "9.0.2".to_string(),
        arch: "aarch64".to_string(),
        source: "martin-riedl".to_string(),
        installed_at: "2025-01-01T00:00:00Z".to_string(),
        files: BTreeMap::from([("ffmpeg".to_string(), "a".repeat(64))]),
    };
    write_state(&dir, &state).expect("запись state.json");
    let read = read_state(&dir).expect("чтение state.json");
    assert_eq!(read.version, state.version);
    assert_eq!(read.files, state.files);
    assert!(!dir
        .join(format!("state.json.tmp.{}", std::process::id()))
        .exists());
    let _ = std::fs::remove_dir_all(&dir);
}
