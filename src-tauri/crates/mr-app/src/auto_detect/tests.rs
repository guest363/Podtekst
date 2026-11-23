use std::time::{Duration, Instant};

use mr_audio::capture::{ActiveAudioClient, CaptureState, DetectionCapability};

use super::detector::{Detector, DetectorAction, TickContext};
use crate::config::AutoDetectMode;

fn client_state(pid: u32, process: &str, capture: CaptureState, render: bool) -> ActiveAudioClient {
    ActiveAudioClient {
        pid: Some(pid),
        process: Some(process.to_string()),
        app_name: None,
        capture,
        has_active_render: render,
    }
}

fn client(pid: u32, process: &str, render: bool) -> ActiveAudioClient {
    client_state(pid, process, CaptureState::Active, render)
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
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(2),
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
            t0 + Duration::from_secs(16),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            rec_ctx,
        )
        .is_empty());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(17),
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
    for step in [0, 2, 4] {
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
    assert!(d
        .tick(
            t0 + Duration::from_secs(6),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &call,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(8),
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

    for step in [0, 2] {
        let _ = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &teams,
            idle_ctx(),
        );
    }
    d.on_manual_stop();
    for step in [4, 6, 8, 10] {
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
    for step in [12, 22, 24] {
        let active = if step < 24 { &[][..] } else { &teams[..] };
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
            t0 + Duration::from_secs(26),
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
    for step in [0, 2] {
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
            t0 + Duration::from_secs(4),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &tg,
            idle_ctx(),
        )
        .is_empty());

    let mut d2 = Detector::new();
    for step in [0, 2] {
        let _ = d2.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &tg,
            idle_ctx(),
        );
    }
    d2.on_auto_start_failed();
    for step in [4, 6, 8] {
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
        capture: CaptureState::Active,
        has_active_render: false,
    }];
    let mut acts = Vec::new();
    for step in [0, 2] {
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
        t0 + Duration::from_secs(4),
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
    for step in [0, 2] {
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
    for step in [0, 2] {
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
    for step in [0, 2] {
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
    for step in [0, 2] {
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
            t0 + Duration::from_secs(4),
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
    for step in [0, 2] {
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
            t0 + Duration::from_secs(4),
            AutoDetectMode::Notify,
            DetectionCapability::Process,
            &zoom,
            manual_rec,
        )
        .is_empty());

    d.on_manual_stop();
    for step in [6, 8, 10, 12] {
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

    for step in [14, 24, 26] {
        let active = if step < 26 { &[][..] } else { &zoom[..] };
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
            t0 + Duration::from_secs(28),
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

#[test]
fn conferencing_starts_and_stays_alive_without_microphone() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom_listen = [client_state(42, "zoom", CaptureState::None, true)];

    assert!(d
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom_listen,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(2),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom_listen,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "zoom" }]
    );

    let rec_ctx = TickContext {
        is_recording: true,
        is_auto_recording: true,
        ..idle_ctx()
    };
    for step in [10, 25, 40] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &zoom_listen,
                rec_ctx,
            )
            .is_empty());
    }
    assert_eq!(
        d.tick(
            t0 + Duration::from_secs(55),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            rec_ctx,
        ),
        vec![DetectorAction::StopAutoRecording { slug: "zoom" }]
    );
}

#[test]
fn browser_and_messenger_handle_muted_calls_and_short_speech_without_youtube_false_positives() {
    let t0 = Instant::now();
    let rec_ctx = TickContext {
        is_recording: true,
        is_auto_recording: true,
        ..idle_ctx()
    };

    // Просмотр видео без захвата микрофона в браузере и пустой sink_input Electron-клиента
    // (например, Пачки в простое) не запускают запись, а звонок в Telegram без микрофона запускается.
    let mut d_media = Detector::new();
    let youtube = [client_state(10, "chrome", CaptureState::None, true)];
    let idle_pachca = [client_state(12, "Pachca", CaptureState::None, true)];
    for step in [0, 2, 4] {
        assert!(d_media
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &youtube,
                idle_ctx(),
            )
            .is_empty());
        assert!(d_media
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &idle_pachca,
                idle_ctx(),
            )
            .is_empty());
    }

    let mut d_tg = Detector::new();
    let tg_muted_call = [client_state(
        11,
        "telegram-desktop",
        CaptureState::None,
        true,
    )];
    assert!(d_tg
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &tg_muted_call,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d_tg.tick(
            t0 + Duration::from_secs(2),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &tg_muted_call,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "telegram" }]
    );

    // Браузер с потоком захвата на паузе (corked) + звуком созвона стартует без речи.
    let mut d_muted = Detector::new();
    let muted_meet = [client_state(10, "chrome", CaptureState::Paused, true)];
    assert!(d_muted
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &muted_meet,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d_muted.tick(
            t0 + Duration::from_secs(2),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &muted_meet,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "chrome" }]
    );

    // Короткая реплика 4 с в середине созвона запускает запись и далее удерживается по выводу звука.
    let mut d_short = Detector::new();
    let split_chrome = [
        client_state(10, "chrome", CaptureState::None, true),
        client_state(12, "chrome", CaptureState::Active, true),
    ];
    assert!(d_short
        .tick(
            t0,
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &split_chrome,
            idle_ctx(),
        )
        .is_empty());
    assert_eq!(
        d_short.tick(
            t0 + Duration::from_secs(2),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &split_chrome,
            idle_ctx(),
        ),
        vec![DetectorAction::StartAutoRecording { slug: "chrome" }]
    );
    for step in [4, 20, 40] {
        assert!(d_short
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Auto,
                DetectionCapability::Process,
                &youtube,
                rec_ctx,
            )
            .is_empty());
    }
    assert_eq!(
        d_short.tick(
            t0 + Duration::from_secs(55),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &[],
            rec_ctx,
        ),
        vec![DetectorAction::StopAutoRecording { slug: "chrome" }]
    );
}

#[test]
fn switching_auto_to_notify_disarms_auto_stop_without_prompting() {
    let mut d = Detector::new();
    let t0 = Instant::now();
    let zoom = [client(42, "Zoom.exe", false)];

    for step in [0, 2] {
        let _ = d.tick(
            t0 + Duration::from_secs(step),
            AutoDetectMode::Auto,
            DetectionCapability::Process,
            &zoom,
            idle_ctx(),
        );
    }

    // После смены режима на notify и сброса auto-флага запись считается ручной и не останавливается.
    let disarmed_rec = TickContext {
        is_recording: true,
        is_auto_recording: false,
        ..idle_ctx()
    };
    for step in [4, 20, 40] {
        assert!(d
            .tick(
                t0 + Duration::from_secs(step),
                AutoDetectMode::Notify,
                DetectionCapability::Process,
                &[],
                disarmed_rec,
            )
            .is_empty());
    }
    assert!(d.prompt().is_none());
}
