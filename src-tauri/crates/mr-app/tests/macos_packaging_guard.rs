//! Разрешения macOS: проверяем значения, а не наличие подстрок — `<false/>` в
//! Entitlements и пустое описание доступа дают отказ TCC и реджект нотаризации.

/// Сравнение по значению, а не по подстроке: `<false/>` содержит тот же ключ.
fn plist_value<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("<key>{key}</key>");
    let rest = plist.split_once(&marker)?.1;
    let rest = rest.trim_start();
    let (open, close) = if rest.starts_with("<string>") {
        ("<string>", "</string>")
    } else if rest.starts_with("<true/>") {
        return Some("true");
    } else if rest.starts_with("<false/>") {
        return Some("false");
    } else {
        return None;
    };
    let value = rest.strip_prefix(open)?;
    let (value, _) = value.split_once(close)?;
    Some(value)
}

/// `<dict>`/`<array>` в plist обязаны быть закрыты — иначе файл не читается.
fn plist_balanced(plist: &str) -> bool {
    for tag in ["dict", "array"] {
        let open = plist.matches(&format!("<{tag}>")).count();
        let close = plist.matches(&format!("</{tag}>")).count();
        if open != close {
            return false;
        }
    }
    true
}

#[test]
fn info_plist_declares_microphone_and_screen_capture() {
    let plist = include_str!("../Info.plist");
    assert!(plist_balanced(plist), "Info.plist: незакрытые dict/array");
    for key in [
        "NSMicrophoneUsageDescription",
        "NSScreenCaptureUsageDescription",
    ] {
        let value = plist_value(plist, key).unwrap_or_default();
        assert!(
            value.trim().chars().count() > 10,
            "{key}: пустое описание доступа"
        );
    }
}

#[test]
fn tauri_config_targets_13_4_with_entitlements() {
    let conf: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json: JSON");
    let minimum = conf
        .pointer("/bundle/macOS/minimumSystemVersion")
        .and_then(serde_json::Value::as_str);
    assert_eq!(minimum, Some("13.4"), "minimumSystemVersion");
    let entitlements = conf
        .pointer("/bundle/macOS/entitlements")
        .and_then(serde_json::Value::as_str);
    assert_eq!(entitlements, Some("Entitlements.plist"), "entitlements");
}

#[test]
fn entitlements_grant_audio_input() {
    let entitlements = include_str!("../Entitlements.plist");
    assert!(
        plist_balanced(entitlements),
        "Entitlements.plist: битый plist"
    );
    assert_eq!(
        plist_value(entitlements, "com.apple.security.device.audio-input"),
        Some("true"),
        "без audio-input macOS не даст микрофон"
    );
}

/// `-link-arg-bin` в build.rs действует только на точное имя из `[[bin]]`.
#[test]
fn build_script_embeds_plist_into_declared_binary() {
    let manifest = include_str!("../Cargo.toml");
    let bin = manifest
        .lines()
        .skip_while(|line| !line.contains("[[bin]]"))
        .find_map(|line| {
            line.trim()
                .strip_prefix("name")
                .and_then(|rest| rest.split_once('='))
                .map(|(_, value)| value.trim().trim_matches('"').to_string())
        })
        .expect("в Cargo.toml нет [[bin]] name");
    let build = include_str!("../build.rs");
    assert!(
        build.contains("rustc-link-arg-bin={}"),
        "build.rs должен брать имя бина из Cargo.toml, а не хардкодить"
    );
    assert!(
        !build.contains(&format!("rustc-link-arg-bin={bin}=-Wl")),
        "имя бина захардкожено литералом"
    );
    let _ = bin;
}
