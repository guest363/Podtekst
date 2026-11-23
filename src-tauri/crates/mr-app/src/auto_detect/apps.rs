use mr_audio::capture::{ActiveAudioClient, CaptureState, DetectionCapability};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AppKind {
    Voip,
    Messenger,
    Browser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchedApp {
    pub(super) slug: &'static str,
    pub(super) label: &'static str,
    pub(super) kind: AppKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchedClient {
    pub(super) app: MatchedApp,
    pub(super) can_start: bool,
    pub(super) keep_alive: bool,
}

pub(super) const GENERIC_CALL: MatchedApp = MatchedApp {
    slug: "call",
    label: "Звонок",
    kind: AppKind::Voip,
};

pub(super) fn matched_clients(
    clients: &[ActiveAudioClient],
    capability: DetectionCapability,
    self_pid: u32,
    is_recording: bool,
) -> Vec<MatchedClient> {
    let mut out: Vec<MatchedClient> = Vec::new();
    for client in clients {
        if client.pid == Some(self_pid) {
            continue;
        }
        if capability == DetectionCapability::DeviceOnly
            && client.pid.is_none()
            && client.process.is_none()
            && client.app_name.is_none()
        {
            if client.capture == CaptureState::Active && !is_recording {
                push_or_merge(
                    &mut out,
                    MatchedClient {
                        app: GENERIC_CALL,
                        can_start: true,
                        keep_alive: true,
                    },
                );
            }
            continue;
        }
        if let Some(matched) = classify_client(client) {
            push_or_merge(&mut out, matched);
        }
    }
    out
}

fn push_or_merge(out: &mut Vec<MatchedClient>, item: MatchedClient) {
    if let Some(existing) = out.iter_mut().find(|e| e.app.slug == item.app.slug) {
        existing.can_start |= item.can_start;
        existing.keep_alive |= item.keep_alive;
    } else {
        out.push(item);
    }
}

fn normalize_token(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    let trimmed = trimmed.strip_suffix(".exe").unwrap_or(&trimmed);
    let trimmed = trimmed.split(".helper").next().unwrap_or(trimmed);
    trimmed.to_string()
}

fn is_self_name(token: &str) -> bool {
    matches!(token, "podtekst" | "meeting-recorder" | "meeting_recorder")
}

fn match_native(token: &str) -> Option<MatchedApp> {
    use AppKind::{Messenger, Voip};
    let (slug, label, kind) = match token {
        "zoom" | "zoom.us" | "cpthost" | "us.zoom.xos" => ("zoom", "Zoom", Voip),
        "teams"
        | "ms-teams"
        | "msteams"
        | "microsoft teams"
        | "com.microsoft.teams"
        | "com.microsoft.teams2" => ("teams", "Microsoft Teams", Messenger),
        "discord" | "discordcanary" | "discordptb" | "com.hnc.discord" => {
            ("discord", "Discord", Messenger)
        }
        "telegram"
        | "telegram desktop"
        | "telegram-desktop"
        | "ayugram"
        | "kotatogram"
        | "ru.keepcoder.telegram"
        | "org.telegram.desktop" => ("telegram", "Telegram", Voip),
        "slack" | "com.tinyspeck.slackmacgap" => ("slack", "Slack", Messenger),
        "skype" | "skypeforlinux" | "com.skype.skype" => ("skype", "Skype", Messenger),
        "webex"
        | "ciscocollabhost"
        | "cisco-webex"
        | "com.cisco.webexmeetingsapp"
        | "com.webex.meetingmanager" => ("webex", "Webex", Messenger),
        "facetime" | "avconferenced" | "com.apple.facetime" | "com.apple.avconferenced" => {
            ("facetime", "FaceTime", Voip)
        }
        "jitsi" | "jitsi-meet" | "org.jitsi.jitsi-meet" => ("jitsi", "Jitsi", Messenger),
        "element" | "element-desktop" | "im.riot.app" => ("element", "Element", Messenger),
        "viber" | "com.viber.osx" => ("viber", "Viber", Messenger),
        "whatsapp" | "net.whatsapp.whatsapp" => ("whatsapp", "WhatsApp", Messenger),
        "telemost" | "yandex.telemost" | "yandex_telemost" | "yandextelemost"
        | "ru.yandex.telemost" => ("telemost", "Яндекс Телемост", Messenger),
        "vkcalls" | "vk calls" | "vk-calls" | "com.vk.calls" => ("vkcalls", "VK Звонки", Messenger),
        "salutejazz" | "sberjazz" | "ru.sberdevices.jazz" => {
            ("salutejazz", "SaluteJazz", Messenger)
        }
        "ktalk" | "kontur.talk" | "kontur-talk" | "kontur talk" | "ru.kontur.talk" => {
            ("ktalk", "Контур Толк", Messenger)
        }
        "trueconf" | "com.trueconf.client" => ("trueconf", "TrueConf", Voip),
        "pachca" => ("pachca", "Пачка", Messenger),
        _ => return None,
    };
    Some(MatchedApp { slug, label, kind })
}

fn match_browser(token: &str) -> Option<MatchedApp> {
    let (slug, label) = match token {
        "chrome"
        | "google-chrome"
        | "google chrome"
        | "google chrome helper"
        | "chromium"
        | "chromium-browser"
        | "com.google.chrome"
        | "org.chromium.chromium" => ("chrome", "Chrome"),
        "msedge" | "microsoft-edge" | "microsoft edge" | "com.microsoft.edgemac" => {
            ("edge", "Edge")
        }
        "firefox" | "firefox-esr" | "firefox-bin" | "org.mozilla.firefox" => ("firefox", "Firefox"),
        "brave" | "brave-browser" | "com.brave.browser" => ("brave", "Brave"),
        "opera" | "com.operasoftware.opera" => ("opera", "Opera"),
        "vivaldi" | "vivaldi-bin" | "com.vivaldi.vivaldi" => ("vivaldi", "Vivaldi"),
        "yandex"
        | "yandex-browser"
        | "yandex_browser"
        | "browser"
        | "ru.yandex.desktop.yandex-browser" => ("yandex_browser", "Яндекс Браузер"),
        "safari" | "com.apple.safari" | "com.apple.webkit.gpu" => ("safari", "Safari"),
        "arc" | "company.thebrowser.browser" => ("arc", "Arc"),
        "zen" | "zen-bin" | "app.zen-browser.zen" => ("zen", "Zen"),
        _ => return None,
    };
    Some(MatchedApp {
        slug,
        label,
        kind: AppKind::Browser,
    })
}

fn signal_for(app: MatchedApp, client: &ActiveAudioClient) -> Option<MatchedClient> {
    let (can_start, keep_alive) = match app.kind {
        AppKind::Voip => {
            let active = client.capture == CaptureState::Active || client.has_active_render;
            (active, active)
        }
        // Electron/WebView-клиенты держат пустой sink_input в простое (например, для уведомлений),
        // поэтому без открытого потока захвата нельзя ни запускать, ни удерживать запись.
        AppKind::Messenger => {
            let can_start = client.capture == CaptureState::Active
                || (client.capture == CaptureState::Paused && client.has_active_render);
            (can_start, client.capture != CaptureState::None)
        }
        // Для браузера нужен хотя бы открытый поток захвата (активный или на паузе) с воспроизведением,
        // иначе нельзя отличить звонок от обычного видео или музыки.
        AppKind::Browser => {
            let can_start = client.capture != CaptureState::None && client.has_active_render;
            // shortcut: удержание браузера по активному выводу звука до паузы, пока нет проверки заголовка вкладки.
            let keep_alive = can_start || client.has_active_render;
            (can_start, keep_alive)
        }
    };
    keep_alive.then_some(MatchedClient {
        app,
        can_start,
        keep_alive,
    })
}

fn classify_client(client: &ActiveAudioClient) -> Option<MatchedClient> {
    let tokens: Vec<String> = [client.process.as_deref(), client.app_name.as_deref()]
        .into_iter()
        .flatten()
        .map(normalize_token)
        .filter(|s| !s.is_empty())
        .collect();
    if tokens.iter().any(|t| is_self_name(t)) {
        return None;
    }
    for token in &tokens {
        if let Some(app) = match_native(token) {
            return signal_for(app, client);
        }
    }
    for token in &tokens {
        if let Some(app) = match_browser(token) {
            return signal_for(app, client);
        }
    }
    None
}
