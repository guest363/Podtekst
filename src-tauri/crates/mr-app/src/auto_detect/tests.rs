use std::time::{Duration, Instant};

use mr_audio::capture::{ActiveAudioClient, DetectionCapability};

use super::detector::{Detector, DetectorAction, TickContext};
use crate::config::AutoDetectMode;

fn client(pid: u32, process: &str, render: bool) -> ActiveAudioClient {
    ActiveAudioClient {
        pid: Some(pid),
        process: Some(process.to_string()),
        app_name: None,
        has_active_render: render,
    }
}

fn idle_ctx() -> TickContext {
    TickContext {
        is_recording: false,
        is_auto_recording: false,
        lifecycle_in_flight: false,
        self_pid: 9999,
    }
}

#[test]
fn native_app_debounces_starts_and_stops_after_cooldown() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom = [client(42, "Zoom.exe", false)];

    assert!(d
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        )
        .is_empty());
    assert!(d
        .tick(
            t0 + Duration::from_secs(2),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(4),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "zoom" }]
    );

    let rec_ctx = TickContext {
        is_recording: true,
        is_auto_recording: true,
        ..idle_ctx()
    };
    assert!(d
        .tick(
            t0 + Duration::from_secs(18),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            rec_ctx,
        )
        .is_empty());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(19),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            rec_ctx,
        ),
        vec![DetectorAction::StopAutoRecording { slug: "zoom" }]
    );
}

#[test]
fn browser_requires_active_render() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let mic_only = [client(10, "chrome.exe", false)];
    for step in [0, 2, 4, 6] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &mic_only,
                idle_ctx(),
            )
            .is_empty());
    }
    let call = [client(10, "chrome.exe", true)];
    for step in [8, 10] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &call,
                idle_ctx(),
            )
            .is_empty());
    }
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(12),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &call,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "chrome" }]
    );
}

#[test]
fn manual_recording_is_never_auto_stopped_and_manual_stop_suppresses() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let teams = [client(7, "ms-teams.exe", true)];
    let manual = TickContext {
        is_recording: true,
        is_auto_recording: false,
        ..idle_ctx()
    };
    assert!(d
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            manual,
        )
        .is_empty());

    for step in [0, 2, 4] {
        let _ = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &teams,
            idle_ctx(),
        );
    }
    d.on_manual_stop();
    for step in [6, 8, 10, 12] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &teams,
                idle_ctx(),
            )
            .is_empty());
    }
    for step in [14, 24, 26, 28] {
        let active = if step < 26 { &[][..] } else { &teams[..] };
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                active,
                idle_ctx(),
            )
            .is_empty());
    }
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(30),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &teams,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "teams" }]
    );
}

#[test]
fn dismiss_and_start_failure_suppress_until_mic_released() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let tg = [client(5, "Telegram.exe", false)];

    let mut last = Vec::new();
    for step in [0, 2, 4] {
        last = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &tg,
            idle_ctx(),
        );
    }
    assert_eq!(
        last,
        vec![DetectorAction::NotifyCall {
            slug: "telegram",
            label: "Telegram",
        }]
    );
    assert!(d.prompt().is_some());
    d.on_dismiss();
    assert!(d.prompt().is_none());
    assert!(d
        .tick(
            t0 + Duration::from_secs(6),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &tg,
            idle_ctx(),
        )
        .is_empty());

    let mut d2 = Detector::new();
    for step in [0, 2, 4] {
        let _ = d2.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &tg,
            idle_ctx(),
        );
    }
    d2.on_auto_start_failed();
    for step in [6, 8, 10] {
        assert!(d2
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &tg,
                idle_ctx(),
            )
            .is_empty());
    }
}

#[test]
fn device_only_forces_notify_self_excluded_and_off_resets() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let anon = [ActiveAudioClient {
        pid: None,
        process: None,
        app_name: None,
        has_active_render: false,
    }];
    let mut acts = Vec::new();
    for step in [0, 2, 4] {
        acts = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::DeviceOnly,
            &anon,
            idle_ctx(),
        );
    }
    assert_eq!(
        acts,
        vec![DetectorAction::NotifyCall {
            slug: "call",
            label: "Звонок",
        }]
    );
    assert!(d.prompt().is_some());
    d.tick(
        t0 + Duration::from_secs(6),
        AutoDetectMode::Off,
        DetectionCapability::DeviceOnly,
        &anon,
        idle_ctx(),
    );
    assert!(d.prompt().is_none());

    let self_client = [
        client(9999, "Zoom.exe", true),
        client(12, "podtekst.exe", true),
    ];
    let mut d2 = Detector::new();
    for step in [0, 2, 4] {
        assert!(d2
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &self_client,
                idle_ctx(),
            )
            .is_empty());
    }
}

#[test]
fn lifecycle_in_flight_skips_tick_without_advancing() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom = [client(42, "Zoom.exe", false)];
    let busy = TickContext {
        lifecycle_in_flight: true,
        ..idle_ctx()
    };
    for step in [0, 2, 4, 6] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &zoom,
                busy,
            )
            .is_empty());
    }
}

#[test]
fn macos_helper_roles_and_avconferenced_match() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let chrome_helper = [client(11, "com.google.Chrome.helper.audio", true)];
    let mut acts = Vec::new();
    for step in [0, 2, 4] {
        acts = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &chrome_helper,
            idle_ctx(),
        );
    }
    assert_eq!(
        acts,
        vec![DetectorAction::StartAutoRecording { slug: "chrome" }]
    );

    let mut d2 = Detector::new();
    let discord_helper = [client(12, "com.hnc.Discord.helper.renderer", false)];
    for step in [0, 2, 4] {
        acts = d2.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &discord_helper,
            idle_ctx(),
        );
    }
    assert_eq!(
        acts,
        vec![DetectorAction::NotifyCall {
            slug: "discord",
            label: "Discord",
        }]
    );

    let mut d3 = Detector::new();
    let avconf = [client(13, "com.apple.avconferenced", false)];
    for step in [0, 2, 4] {
        acts = d3.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &avconf,
            idle_ctx(),
        );
    }
    assert_eq!(
        acts,
        vec![DetectorAction::NotifyCall {
            slug: "facetime",
            label: "FaceTime",
        }]
    );
}

#[test]
fn prompted_transitions_to_auto_on_mode_switch() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom = [client(42, "Zoom.exe", false)];
    for step in [0, 2, 4] {
        let _ = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        );
    }
    assert!(d.prompt().is_some());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(6),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "zoom" }]
    );
}

#[test]
fn manual_start_from_banner_then_manual_stop_suppresses_same_call() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom = [client(42, "Zoom.exe", false)];
    for step in [0, 2, 4] {
        let _ = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        );
    }
    assert!(d.prompt().is_some());

    d.on_manual_start();
    assert!(d.prompt().is_none());
    let manual_rec = TickContext {
        is_recording: true,
        is_auto_recording: false,
        ..idle_ctx()
    };
    assert!(d
        .tick(
            t0 + Duration::from_secs(6),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &zoom,
            manual_rec,
        )
        .is_empty());

    d.on_manual_stop();
    for step in [8, 10, 12, 14] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Notify,
                DetectionCapability::Process,
                &zoom,
                idle_ctx(),
            )
            .is_empty());
        assert!(d.prompt().is_none());
    }

    for step in [16, 26, 28, 30] {
        let active = if step < 28 { &[][..] } else { &zoom[..] };
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Notify,
                DetectionCapability::Process,
                active,
                idle_ctx(),
            )
            .is_empty());
    }
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(32),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        ),
        vec![DetectorAction::NotifyCall {
            slug: "zoom",
            label: "Zoom",
        }]
    );
}
