use std::path::Path;

/// Виртуальные драйверы и SCK: их имена технические и нужны для диагностики.
const KNOWN_DEVICES: [&str; 5] = [
    "blackhole",
    "soundflower",
    "loopback",
    "virtual cable",
    "screencapturekit",
];

pub(crate) fn redact_path(path: &Path) -> String {
    redact_text(&path.to_string_lossy())
}

pub(crate) fn redact_text(text: &str) -> String {
    redact_with(text, home_dir().as_deref())
}

pub(crate) fn mask_device_name(name: &str, index: usize) -> String {
    let lower = name.to_lowercase();
    if KNOWN_DEVICES.iter().any(|needle| lower.contains(needle)) {
        name.to_string()
    } else {
        format!("Устройство {}", index + 1)
    }
}

fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

fn redact_with(text: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if !home.is_empty() => text.replace(home, "~"),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_home_prefix_only() {
        let text = "/home/user/recordings/2026-11-03.wav";
        assert_eq!(
            redact_with(text, Some("/home/user")),
            "~/recordings/2026-11-03.wav"
        );
        assert_eq!(redact_with(text, None), text);
    }

    #[test]
    fn masks_personal_device_names() {
        assert_eq!(mask_device_name("AirPods Pro Test", 0), "Устройство 1");
        assert_eq!(mask_device_name("BlackHole 2ch", 1), "BlackHole 2ch");
        assert_eq!(
            mask_device_name("Системный звук (ScreenCaptureKit)", 0),
            "Системный звук (ScreenCaptureKit)"
        );
    }

    #[test]
    fn redacts_path_with_known_home() {
        let path = Path::new("/tmp/example/recording.wav");
        assert!(redact_path(path).ends_with("recording.wav"));
    }
}
