use std::io::Read;
use std::path::PathBuf;

use serde_json::json;

use super::{json_response, query_param};
use crate::config::save_dir;

/// Имя попадает в AppleScript/PowerShell: кавычки и подстановки ломают
/// команду, поэтому оставляем только безопасные символы.
fn sanitize_shell_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\'' | '"' | '`' | '$' | '\n' | '\r'))
        .collect()
}

pub(crate) fn pick_save_path(default_name: &str, filter_pattern: &str) -> Option<PathBuf> {
    let default_name = sanitize_shell_text(default_name);
    let initial_path = save_dir().join(&default_name);
    #[cfg(target_os = "macos")]
    let _ = (filter_pattern, &initial_path);

    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = std::process::Command::new("zenity")
            .arg("--file-selection")
            .arg("--save")
            .arg("--confirm-overwrite")
            .arg(format!("--filename={}", initial_path.display()))
            .arg(format!("--file-filter={filter_pattern}"))
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
            // Выход с ненулевым кодом у zenity — пользователь нажал "Отмена"
            return None;
        }

        if let Ok(output) = std::process::Command::new("kdialog")
            .arg("--getsavefilename")
            .arg(initial_path.display().to_string())
            .arg(filter_pattern)
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
            return None;
        }
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "set f to choose file name with prompt \"Сохранить файл как:\" default name \"{}\"\nPOSIX path of f",
            default_name
        );
        if let Ok(output) = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
            return None;
        }
    }

    #[cfg(target_os = "windows")]
    {
        let initial = sanitize_shell_text(&initial_path.to_string_lossy());
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.SaveFileDialog; $d.FileName = '{}'; $d.Filter = 'Files ({})|{}'; if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ $d.FileName }}",
            initial,
            filter_pattern,
            filter_pattern
        );
        if let Ok(output) = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&script)
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
            return None;
        }
    }

    None
}

pub(super) fn handle_save_dialog(mut request: tiny_http::Request, query: &str) {
    let name = query_param(query, "name").unwrap_or_else(|| "document.docx".to_string());

    let mut body = Vec::new();
    if request
        .as_reader()
        .take(super::MAX_JSON_BODY)
        .read_to_end(&mut body)
        .is_err()
        || body.is_empty()
    {
        json_response(request, json!({"ok": false, "error": "Пустые данные"}), 400);
        return;
    }

    let filter = if name.ends_with(".docx") {
        "*.docx"
    } else {
        "*.*"
    };

    let is_headless = std::env::var("DISPLAY").is_err()
        && std::env::var("WAYLAND_DISPLAY").is_err()
        && cfg!(target_os = "linux");

    if is_headless {
        json_response(request, json!({"ok": false, "no_dialog": true}), 200);
        return;
    }

    match pick_save_path(&name, filter) {
        Some(mut path) => {
            if name.ends_with(".docx") && path.extension().is_none_or(|e| e != "docx") {
                path.set_extension("docx");
            }
            if let Err(e) = std::fs::write(&path, &body) {
                json_response(
                    request,
                    json!({"ok": false, "error": format!("Ошибка записи файла: {e}")}),
                    500,
                );
            } else {
                json_response(
                    request,
                    json!({"ok": true, "path": path.to_string_lossy()}),
                    200,
                );
            }
        }
        None => {
            #[cfg(target_os = "linux")]
            let tool_exists = std::path::Path::new("/usr/bin/zenity").exists()
                || std::path::Path::new("/usr/bin/kdialog").exists();
            #[cfg(not(target_os = "linux"))]
            let tool_exists = true;

            if tool_exists {
                json_response(request, json!({"ok": false, "cancelled": true}), 200);
            } else {
                json_response(request, json!({"ok": false, "no_dialog": true}), 200);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_shell_text;

    #[test]
    fn shell_text_strips_quotes_and_controls() {
        assert_eq!(sanitize_shell_text("a'b\"c`d$e\nf\rg\u{7}"), "abcdefg");
        assert_eq!(sanitize_shell_text("Совещание 2.docx"), "Совещание 2.docx");
    }
}
