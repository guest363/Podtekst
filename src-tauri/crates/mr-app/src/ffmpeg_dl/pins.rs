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
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct PinFile {
    #[allow(dead_code)]
    schema: u32,
    packages: Vec<Package>,
}

pub(super) fn all() -> &'static [Package] {
    static PINS: OnceLock<Vec<Package>> = OnceLock::new();
    PINS.get_or_init(|| {
        serde_json::from_str::<PinFile>(PINS_JSON)
            .map(|file| file.packages)
            .unwrap_or_default()
    })
}

pub(super) fn current() -> Result<Vec<Package>, String> {
    for_target(std::env::consts::OS, std::env::consts::ARCH)
}

pub(super) fn for_target(os: &str, arch: &str) -> Result<Vec<Package>, String> {
    if (os, arch) == ("macos", "x86_64") {
        return Err(
            "Сборка для Intel Mac не поддерживается: ffmpeg ставится только на Apple Silicon."
                .to_string(),
        );
    }
    let packages: Vec<Package> = all()
        .iter()
        .filter(|p| p.os == os && p.arch == arch)
        .cloned()
        .collect();
    if packages.is_empty() {
        return Err(format!(
            "Автоскачивание ffmpeg не настроено для {os}/{arch}: поставьте ffmpeg в PATH."
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
    fn intel_mac_is_rejected() {
        let error = for_target("macos", "x86_64").expect_err("Intel Mac не поддерживается");
        assert!(error.contains("Intel"), "{error}");
    }

    #[test]
    fn unknown_target_is_rejected() {
        assert!(for_target("freebsd", "x86_64").is_err());
    }
}
