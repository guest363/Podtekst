//! Кроссплатформенный автозапуск при входе в систему (без Tauri-зависимостей).
//!
//! * Linux: файл `~/.config/autostart/meeting-recorder.desktop`
//! * macOS: `~/Library/LaunchAgents/com.meetingrecorder.app.plist`
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

/// Путь к записи автозапуска для текущей ОС.
pub fn autostart_file_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .join("Library")
            .join("LaunchAgents")
            .join("com.meetingrecorder.app.plist");
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
                .join("MeetingRecorder.lnk");
        }
        home_dir().join("MeetingRecorder-autostart.lnk")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".config"));
        base.join("autostart").join("meeting-recorder.desktop")
    }
}

/// Наличие записи автозапуска (используется тестами и внешними вызовами).
#[allow(dead_code)]
pub fn is_autostart_enabled() -> bool {
    autostart_file_path().exists()
}

/// Создаёт запись автозапуска (`exec_path` — путь к бинарнику).
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
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n<key>Label</key><string>com.meetingrecorder.app</string>\n<key>ProgramArguments</key><array><string>{esc}</string><string>--tray</string></array>\n<key>RunAtLoad</key><true/>\n</dict>\n</plist>\n"
        );
        std::fs::write(&target, plist)?;
        return Ok(());
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
            "[Desktop Entry]\nVersion=1.0\nType=Application\nName=Запись встреч (Фоновый режим)\nExec=\"{exec_path}\" --tray\nIcon=meeting-recorder\nTerminal=false\nCategories=AudioVideo;Audio;Recorder;\nStartupNotify=false\nX-GNOME-Autostart-enabled=true\n"
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

/// Системное всплывающее уведомление (Linux notify-send / macOS osascript).
#[allow(dead_code)]
pub fn notify(title: &str, body: &str) {
    notify_icon(title, body, "audio-input-microphone");
}

/// Системное уведомление с указанием иконки.
pub fn notify_icon(title: &str, body: &str, icon: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(mut child) = std::process::Command::new("notify-send")
            .args(["-a", "Запись встреч", "-i", icon, title, body])
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
        let script = format!("display notification \"{body}\" with title \"{title}\"");
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
            assert!(install_autostart("/fake/meeting-recorder.exe").is_err());
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            install_autostart("/fake/meeting-recorder")?;
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
}
