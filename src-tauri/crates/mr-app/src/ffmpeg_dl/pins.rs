//! Пины компонентов, вшитые в сборку: клиент не проверяет версии в сети,
//! а ставит ровно тот архив, который проверил CI.

use std::sync::OnceLock;

use serde::Deserialize;

const PINS_JSON: &str = include_str!("../../components-pins.json");

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Package {
    pub os: String,
    pub arch: String,
    pub version: String,
    pub source: String,
    pub provides: Vec<String>,
    pub format: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Deserialize)]
struct PinFile {
    #[allow(dead_code)]
    schema: u32,
    packages: Vec<Package>,
}

fn parsed() -> &'static Result<Vec<Package>, String> {
    static PINS: OnceLock<Result<Vec<Package>, String>> = OnceLock::new();
    PINS.get_or_init(|| {
        let file = serde_json::from_str::<PinFile>(PINS_JSON)
            .map_err(|e| format!("components-pins.json не разбирается: {e}"))?;
        validate(&file.packages)?;
        Ok(file.packages)
    })
}

pub(super) fn all() -> &'static [Package] {
    parsed().as_deref().unwrap_or(&[])
}

/// Пины вшиты в сборку: битый файл не должен выглядеть как «нет пина для платформы».
fn validate(packages: &[Package]) -> Result<(), String> {
    const OSES: [&str; 3] = ["linux", "macos", "windows"];
    const ARCHES: [&str; 2] = ["x86_64", "aarch64"];
    for package in packages {
        if !OSES.contains(&package.os.as_str()) || !ARCHES.contains(&package.arch.as_str()) {
            return Err(format!(
                "неизвестная платформа {}/{}",
                package.os, package.arch
            ));
        }
        if !package.url.starts_with("https://") {
            return Err(format!("{}: url не https", package.source));
        }
        if package.sha256.len() != 64 || !package.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{}: sha256 не 64 hex", package.source));
        }
        if package.provides.is_empty() {
            return Err(format!("{}: пустой список файлов", package.source));
        }
    }
    Ok(())
}

pub(super) fn current() -> Result<Vec<Package>, String> {
    for_target(std::env::consts::OS, std::env::consts::ARCH)
}

pub(super) fn for_target(os: &str, arch: &str) -> Result<Vec<Package>, String> {
    if (os, arch) == ("macos", "x86_64") {
        return Err(
            "Сборка для Mac с процессором Intel не поддерживается: ffmpeg устанавливается только на Apple Silicon."
                .to_string(),
        );
    }
    if let Err(error) = parsed() {
        return Err(format!(
            "Вшитый список пинов повреждён ({error}): переустановите приложение."
        ));
    }
    let packages: Vec<Package> = all()
        .iter()
        .filter(|p| p.os == os && p.arch == arch)
        .cloned()
        .collect();
    if packages.is_empty() {
        return Err(format!(
            "Автоскачивание ffmpeg не настроено для {os}/{arch}: добавьте ffmpeg в PATH."
        ));
    }
    Ok(packages)
}

/// Все пакеты одной платформы держат одну версию: UI показывает её как ожидаемую.
pub(super) fn expected_version(packages: &[Package]) -> Option<String> {
    let first = packages.first()?.version.clone();
    packages.iter().all(|p| p.version == first).then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_pins_parse() {
        let packages = all();
        assert_eq!(packages.len(), 5, "в пинах должно быть 5 пакетов");
        assert!(packages
            .iter()
            .all(|p| p.url.starts_with("https://") && !p.provides.is_empty()));
    }

    #[test]
    fn every_supported_target_has_a_pin() {
        for (os, arch) in [
            ("windows", "x86_64"),
            ("macos", "aarch64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let packages = for_target(os, arch).expect("пин платформы");
            assert!(
                packages
                    .iter()
                    .any(|p| p.provides.iter().any(|n| n == "ffmpeg")),
                "{os}/{arch}: нет ffmpeg"
            );
            assert!(
                packages
                    .iter()
                    .any(|p| p.provides.iter().any(|n| n == "ffprobe")),
                "{os}/{arch}: нет ffprobe"
            );
            assert!(
                expected_version(&packages).is_some(),
                "{os}/{arch}: у пакетов разные версии"
            );
        }
    }

    #[test]
    fn expected_versions_follow_platform() {
        assert_eq!(
            expected_version(&for_target("linux", "aarch64").unwrap()).as_deref(),
            Some("7.0.2")
        );
        assert_eq!(
            expected_version(&for_target("macos", "aarch64").unwrap()).as_deref(),
            Some("9.0.2")
        );
        assert_eq!(
            expected_version(&for_target("windows", "x86_64").unwrap()).as_deref(),
            Some("9.0.2")
        );
    }

    #[test]
    fn pinned_versions_are_bare_tokens() {
        for package in all() {
            assert_eq!(
                mr_audio::tools::version_token(&package.version).as_deref(),
                Some(package.version.as_str()),
                "version пина {} должен быть без префикса и суффикса",
                package.version
            );
        }
    }

    #[test]
    fn intel_mac_is_rejected() {
        let error = for_target("macos", "x86_64").expect_err("Intel Mac не поддерживается");
        assert!(error.contains("Intel"), "{error}");
    }

    #[test]
    fn unknown_target_is_rejected() {
        assert!(for_target("freebsd", "x86_64").is_err());
    }

    #[test]
    fn bundled_pins_pass_validation() {
        parsed().as_ref().expect("пины должны проходить проверку");
        let mut broken = vec![Package {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            version: "1".to_string(),
            source: "test".to_string(),
            provides: vec!["ffmpeg".to_string()],
            format: "zip".to_string(),
            url: "http://example.invalid/a.zip".to_string(),
            sha256: "0".repeat(64),
        }];
        assert!(validate(&broken).is_err(), "http-пин должен отклоняться");
        broken[0].url = "https://example.invalid/a.zip".to_string();
        broken[0].sha256 = "zz".to_string();
        assert!(
            validate(&broken).is_err(),
            "некороткий sha должен отклоняться"
        );
        broken[0].sha256 = "0".repeat(64);
        broken[0].os = "freebsd".to_string();
        assert!(
            validate(&broken).is_err(),
            "неизвестная ось должна отклоняться"
        );
    }
}
