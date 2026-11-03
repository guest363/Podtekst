#[test]
fn info_plist_declares_microphone_and_screen_capture() {
    let plist = include_str!("../Info.plist");
    assert!(plist.contains("<key>NSMicrophoneUsageDescription</key>"));
    assert!(plist.contains("<key>NSScreenCaptureUsageDescription</key>"));
}

#[test]
fn tauri_config_targets_13_4_with_entitlements() {
    let conf = include_str!("../tauri.conf.json");
    assert!(conf.contains("\"minimumSystemVersion\": \"13.4\""));
    assert!(conf.contains("\"entitlements\": \"Entitlements.plist\""));
}

#[test]
fn entitlements_grant_audio_input() {
    let entitlements = include_str!("../Entitlements.plist");
    assert!(entitlements.contains("<key>com.apple.security.device.audio-input</key>"));
}
