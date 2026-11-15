use std::io::Write as _;

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

fn package(version: &str, tools: &[&str]) -> pins::Package {
    pins::Package {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: version.to_string(),
        source: "test".to_string(),
        provides: tools.iter().map(|name| name.to_string()).collect(),
        format: "zip".to_string(),
        url: "https://example.invalid/a.zip".to_string(),
        sha256: "0".repeat(64),
    }
}

fn write_zip(path: &Path, name: &str, body: &[u8]) {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    writer
        .start_file(name, SimpleFileOptions::default().unix_permissions(0o755))
        .unwrap();
    writer.write_all(body).unwrap();
    writer.finish().unwrap();
}

/// Настоящий ELF с нужной строкой версии: проверка пина запускает файл.
#[cfg(unix)]
fn fake_tool(dir: &Path, version: &str) -> PathBuf {
    let source = dir.join("fake.c");
    let body = format!(
        "#include <stdio.h>\nint main(void){{puts(\"ffmpeg version {version} Copyright (c) test\");return 0;}}\n"
    );
    std::fs::write(&source, body).expect("исходник фикстуры");
    let binary = dir.join(bin_name("ffmpeg"));
    let status = std::process::Command::new("cc")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .status()
        .expect("cc нужен для фикстуры установки ffmpeg");
    assert!(status.success(), "cc не собрал фикстуру");
    binary
}

#[test]
fn can_update_follows_probes_and_pair() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-can-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let packages = vec![package("9.0.2", &["ffmpeg", "ffprobe"])];
    let arch = std::env::consts::ARCH;
    let current = probe(Some("9.0.2"), "app", Some(arch));
    let other = probe(Some("7.1.1"), "app", Some(arch));
    let foreign = if arch == "aarch64" {
        "x86_64"
    } else {
        "aarch64"
    };
    let system = probe(Some("7.1.1"), "path", Some(arch));

    assert!(!needs_update(
        &dir,
        Some(&packages),
        Some(&system),
        Some(&system)
    ));
    assert!(!needs_update(&dir, Some(&packages), None, None));
    std::fs::write(dir.join(bin_name("ffmpeg")), b"broken").unwrap();
    assert!(
        needs_update(&dir, Some(&packages), None, None),
        "битый управляемый бинарник — повод переустановить"
    );
    assert!(needs_update(
        &dir,
        Some(&packages),
        Some(&other),
        Some(&other)
    ));
    assert!(
        needs_update(
            &dir,
            Some(&packages),
            Some(&probe(Some("9.0.2"), "app", Some(foreign))),
            Some(&current)
        ),
        "проб чужой архитектуры — повод переустановить"
    );
    assert!(
        needs_update(&dir, Some(&packages), Some(&current), Some(&other)),
        "пара ffmpeg/ffprobe расходится"
    );
    assert!(!needs_update(
        &dir,
        Some(&packages),
        Some(&current),
        Some(&current)
    ));
    assert!(!needs_update(&dir, None, Some(&current), Some(&current)));
    let _ = std::fs::remove_dir_all(&dir);
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
fn package_current_requires_working_probe() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-cur-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pinned = package("9.0.2", &["ffmpeg"]);
    assert!(!package_current(&pinned, &dir), "файла нет");
    std::fs::write(dir.join(bin_name("ffmpeg")), b"broken").unwrap();
    assert!(!package_current(&pinned, &dir), "битый бинарник");
    #[cfg(unix)]
    {
        let fake = fake_tool(&dir, "9.0.2");
        assert_eq!(fake, dir.join(bin_name("ffmpeg")));
        assert!(package_current(&pinned, &dir));
        assert!(!package_current(&package("9.0.3", &["ffmpeg"]), &dir));
        assert!(!package_current(
            &package("9.0.2", &["ffmpeg", "ffprobe"]),
            &dir
        ));
    }
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
    let mut package = package("9.0.2", &["ffmpeg"]);
    package.sha256 = "0".repeat(64);
    let error = install_archive(&package, &archive, &bin, &work).expect_err("sha256 не совпадает");
    assert!(error.contains("контрольная сумма"), "{error}");
    assert_eq!(std::fs::read(bin.join(bin_name("ffmpeg"))).unwrap(), b"old");

    package.sha256 = mr_models::sha256_of_file(&archive).unwrap();
    let error = install_archive(&package, &archive, &bin, &work).expect_err("не бинарник");
    assert!(
        error.contains("архитектура") || error.contains("не запускается"),
        "{error}"
    );
    assert_eq!(std::fs::read(bin.join(bin_name("ffmpeg"))).unwrap(), b"old");
    assert!(!bin.join(format!("{}.new", bin_name("ffmpeg"))).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn install_archive_publishes_pinned_version_and_replaces_broken() {
    let dir = std::env::temp_dir().join(format!("mr-ffmpeg-ok-{}", uuid::Uuid::new_v4()));
    let bin = dir.join("bin");
    let work = dir.join("work");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    // Без C-компилятора тест обязан падать: иначе публикация бинарника
    // (самая рискованная операция модуля) остаётся непроверенной.
    let fake = fake_tool(&dir, "9.0.2");
    let archive = dir.join("a.zip");
    write_zip(&archive, "ffmpeg", &std::fs::read(&fake).unwrap());
    let mut package = package("9.0.2", &["ffmpeg"]);
    package.sha256 = mr_models::sha256_of_file(&archive).unwrap();

    install_archive(&package, &archive, &bin, &work).expect("установка");
    assert!(bin.join(bin_name("ffmpeg")).is_file());
    assert!(!bin.join(format!("{}.new", bin_name("ffmpeg"))).exists());

    let mut newer = package.clone();
    newer.version = "9.0.3".to_string();
    let error = install_archive(&newer, &archive, &bin, &work).expect_err("пин не совпал");
    assert!(error.contains("апстрим обновился"), "{error}");
    assert!(
        bin.join(bin_name("ffmpeg")).is_file(),
        "рабочий бинарник не тронут"
    );

    std::fs::write(bin.join(bin_name("ffmpeg")), b"broken").unwrap();
    assert!(!package_current(&package, &bin));
    install_archive(&package, &archive, &bin, &work).expect("переустановка");
    assert_ne!(
        std::fs::read(bin.join(bin_name("ffmpeg"))).unwrap(),
        b"broken"
    );
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
        "installed",
        "can_update",
        "pair_ok",
    ] {
        assert!(status.get(key).is_some(), "нет поля {key}");
    }
}
