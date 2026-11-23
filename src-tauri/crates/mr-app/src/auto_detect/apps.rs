use mr_audio::capture::{ActiveAudioClient, DetectionCapability};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatchedApp {
    pub(super) slug: &'static str,
    pub(super) label: &'static str,
}

pub(super) const GENERIC_CALL: MatchedApp = MatchedApp {
    slug: "call",
    label: "Звонок",
};

pub(super) fn matched_clients(
    clients: &[ActiveAudioClient],
    capability: DetectionCapability,
    self_pid: u32,
    is_recording: bool,
) -> Vec<MatchedApp> {
    let mut out = Vec::new();
    for client in clients {
        if client.pid == Some(self_pid) {
            continue;
        }
        if capability == DetectionCapability::DeviceOnly
            && client.pid.is_none()
            && client.process.is_none()
            && client.app_name.is_none()
        {
            if !is_recording {
                out.push(GENERIC_CALL);
            }
            continue;
        }
        if let Some(app) = classify_client(client) {
            if !out
                .iter()
                .any(|existing: &MatchedApp| existing.slug == app.slug)
            {
                out.push(app);
            }
        }
    }
    out
}

fn normalize_token(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    let trimmed = trimmed.strip_suffix(".exe").unwrap_or(&trimmed);
    let trimmed = trimmed.strip_suffix(".helper").unwrap_or(trimmed);
    trimmed.to_string()
}

fn is_self_name(token: &str) -> bool {
    matches!(token, "podtekst" | "meeting-recorder" | "meeting_recorder")
}

fn match_native(token: &str) -> Option<MatchedApp> {
    let (slug, label) = match token {
        "zoom" | "zoom.us" | "cpthost" | "us.zoom.xos" => ("zoom", "Zoom"),
        "teams"
        | "ms-teams"
        | "msteams"
        | "microsoft teams"
        | "com.microsoft.teams"
        | "com.microsoft.teams2" => ("teams", "Microsoft Teams"),
        "discord" | "discordcanary" | "discordptb" | "com.hnc.discord" => ("discord", "Discord"),
        "telegram"
        | "telegram desktop"
        | "telegram-desktop"
        | "ayugram"
        | "kotatogram"
        | "ru.keepcoder.telegram"
        | "org.telegram.desktop" => ("telegram", "Telegram"),
        "slack" | "com.tinyspeck.slackmacgap" => ("slack", "Slack"),
        "skype" | "skypeforlinux" | "com.skype.skype" => ("skype", "Skype"),
        "webex"
        | "ciscocollabhost"
        | "cisco-webex"
        | "com.cisco.webexmeetingsapp"
        | "com.webex.meetingmanager" => ("webex", "Webex"),
        "facetime" | "avconferenced" | "com.apple.facetime" => ("facetime", "FaceTime"),
        "jitsi" | "jitsi-meet" | "org.jitsi.jitsi-meet" => ("jitsi", "Jitsi"),
        "element" | "element-desktop" | "im.riot.app" => ("element", "Element"),
        "viber" | "com.viber.osx" => ("viber", "Viber"),
        "whatsapp" | "net.whatsapp.whatsapp" => ("whatsapp", "WhatsApp"),
        "telemost" | "yandex.telemost" | "yandex_telemost" | "yandextelemost"
        | "ru.yandex.telemost" => ("telemost", "Яндекс Телемост"),
        "vkcalls" | "vk calls" | "vk-calls" | "com.vk.calls" => ("vkcalls", "VK Звонки"),
        "salutejazz" | "sberjazz" | "ru.sberdevices.jazz" => ("salutejazz", "SaluteJazz"),
        "ktalk" | "kontur.talk" | "kontur-talk" | "kontur talk" | "ru.kontur.talk" => {
            ("ktalk", "Контур Толк")
        }
        "trueconf" | "com.trueconf.client" => ("trueconf", "TrueConf"),
        _ => return None,
    };
    Some(MatchedApp { slug, label })
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
    Some(MatchedApp { slug, label })
}

fn classify_client(client: &ActiveAudioClient) -> Option<MatchedApp> {
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
            return Some(app);
        }
    }
    if client.has_active_render {
        for token in &tokens {
            if let Some(app) = match_browser(token) {
                return Some(app);
            }
        }
    }
    None
}
