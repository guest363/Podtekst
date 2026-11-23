//! Кроссплатформенный автозапуск при входе в систему (без Tauri-зависимостей).
//!
//! * Linux: файл `~/.config/autostart/podtekst.desktop`
//! * macOS: `~/Library/LaunchAgents/io.github.guest363.podtekst.plist`
//! * Windows: ключ автозапуска документирован; запись в реестр выполняется
//!   установщиком (NSIS/WiX), здесь — проверка наличия через %APPDATA% ярлык.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum DesktopError {
    #[error("ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),
    #[error("не поддерживается на этой ОС без установщика: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, DesktopError>;

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn autostart_file_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library")
            .join("LaunchAgents")
            .join("io.github.guest363.podtekst.plist")
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
                .join("Podtekst.lnk");
        }
        home_dir().join("Podtekst-autostart.lnk")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".config"));
        base.join("autostart").join("podtekst.desktop")
    }
}

pub fn is_autostart_enabled() -> bool {
    autostart_file_path().exists()
}

static HOTKEY: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// Регистрация глобальной клавиши живёт в Tauri-слое; проверка настроек
/// показывает факт, а не константу «зарегистрирована».
#[cfg(feature = "tauri-ui")]
pub(crate) fn set_hotkey_registered(registered: bool) {
    *HOTKEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(registered);
}

pub(crate) fn hotkey_registered() -> Option<bool> {
    *HOTKEY.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn install_autostart(exec_path: &str) -> Result<()> {
    let target = autostart_file_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(target_os = "macos")]
    {
        // Путь может содержать & и < — экранируем для валидного XML.
        let esc = exec_path
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n<key>Label</key><string>io.github.guest363.podtekst</string>\n<key>ProgramArguments</key><array><string>{esc}</string><string>--tray</string></array>\n<key>RunAtLoad</key><true/>\n</dict>\n</plist>\n"
        );
        std::fs::write(&target, plist)?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        // Ярлык .lnk требует COM; установщик создаёт его сам.
        // Здесь фиксируем намерение текстовым маркером рядом с бинарником.
        let _ = exec_path;
        Err(DesktopError::Unsupported(
            "на Windows автозапуск создаёт установщик (NSIS/WiX) или вручную: shell:startup"
                .to_string(),
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Кавычки обязательны: путь с пробелами иначе разваливает Exec на аргументы.
        let desktop = format!(
            "[Desktop Entry]\nVersion=1.0\nType=Application\nName=Подтекст (Фоновый режим)\nExec=\"{exec_path}\" --tray\nIcon=podtekst\nTerminal=false\nCategories=AudioVideo;Audio;Recorder;\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&target, desktop)?;
        Ok(())
    }
}

pub fn uninstall_autostart() -> Result<bool> {
    let target = autostart_file_path();
    if !target.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&target)?;
    Ok(true)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn escape_applescript(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn notify_icon(title: &str, body: &str, icon: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(mut child) = std::process::Command::new("notify-send")
            .args(["-a", "Подтекст", "-i", icon, title, body])
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = icon;
        let esc_body = escape_applescript(body);
        let esc_title = escape_applescript(title);
        let script = format!("display notification \"{esc_body}\" with title \"{esc_title}\"");
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;

        let _ = icon;
        let xml = format!(
            "<toast><visual><binding template=\"ToastText02\"><text id=\"1\">{}</text><text id=\"2\">{}</text></binding></visual></toast>",
            escape_xml(title),
            escape_xml(body),
        );
        // Постоянный AUMID Windows PowerShell гарантирует показ уведомления без регистрации ярлыка.
        let ps = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
            [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null; \
            $doc = New-Object Windows.Data.Xml.Dom.XmlDocument; \
            $doc.LoadXml($env:PODTEKST_TOAST_XML); \
            $toast = [Windows.UI.Notifications.ToastNotification]::new($doc); \
            [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($toast)";
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                ps,
            ])
            .env("PODTEKST_TOAST_XML", xml)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (title, body, icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_autostart_roundtrip() -> Result<()> {
        assert!(!is_autostart_enabled());
        #[cfg(target_os = "windows")]
        {
            // .lnk на Windows создаёт установщик, вручную — не поддерживается
            assert!(install_autostart("/fake/podtekst.exe").is_err());
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            install_autostart("/fake/podtekst")?;
            assert!(is_autostart_enabled());
            let content = std::fs::read_to_string(autostart_file_path()).unwrap();
            assert!(content.contains("--tray"));
            assert!(uninstall_autostart()?);
            assert!(!is_autostart_enabled());
            Ok(())
        }
    }

    #[test]
    fn desktop_entry_roundtrip_in_temp_home() {
        // Изолируем HOME, чтобы не трогать реальный автозапуск пользователя
        let tmp = std::env::temp_dir().join(format!("mr-desktop-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev_home = std::env::var("HOME").ok();
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("HOME", &tmp);
        std::env::remove_var("XDG_CONFIG_HOME");
        let res = check_autostart_roundtrip();
        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }
        if let Some(x) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", x);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        res.unwrap();
    }

    #[test]
    fn notification_escapers_handle_special_chars() {
        assert_eq!(escape_applescript("a\"b\\c\nd"), "a\\\"b\\\\c d");
        assert_eq!(
            escape_xml("<Zoom> & \"'ok'\""),
            "&lt;Zoom&gt; &amp; &quot;&apos;ok&apos;&quot;"
        );
    }
}
