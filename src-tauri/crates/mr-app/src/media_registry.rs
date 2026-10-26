//! Реестр медиафайлов: какие создал сам рекордер и какие убраны из списка.
//!
//! Свои записи можно удалять вместе с расшифровкой. Всё, что попало в папку
//! записей извне (положили вручную, принесли из другого приложения), при
//! удалении из UI только скрывается: исходный файл на диске остаётся.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::{app_data_dir, config_lock};

#[derive(Default)]
pub(crate) struct Registry {
    owned: Vec<String>,
    imported: Vec<String>,
    hidden: Vec<String>,
}

fn registry_path() -> PathBuf {
    app_data_dir().join("media_registry.json")
}

/// Абсолютный путь без `..`; существующий файл — через canonicalize,
/// ещё не созданный — через канонический родительский каталог.
fn normalize(p: &Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    if let Ok(real) = abs.canonicalize() {
        return real.to_string_lossy().into_owned();
    }
    if let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) {
        if let Ok(dir) = parent.canonicalize() {
            return dir.join(name).to_string_lossy().into_owned();
        }
    }
    abs.to_string_lossy().into_owned()
}

fn load() -> Registry {
    let raw: Value = std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let list = |key: &str| -> Vec<String> {
        raw.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    Registry {
        owned: list("owned"),
        imported: list("imported"),
        hidden: list("hidden"),
    }
}

fn save(reg: &Registry) {
    let _ = std::fs::create_dir_all(app_data_dir());
    let path = registry_path();
    let tmp = path.with_extension("json.tmp");
    let body = json!({"owned": reg.owned, "imported": reg.imported, "hidden": reg.hidden});
    if let Ok(text) = serde_json::to_string_pretty(&body) {
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn update(f: impl FnOnce(&mut Registry)) {
    config_lock(|| {
        let mut reg = load();
        f(&mut reg);
        save(&reg);
    });
}

/// Имя, которое рекордер генерирует сам: `ЧЧ:ММ-ЧЧ:ММ__ДД-ММ-ГГГГ.wav`
/// (на Windows двоеточия заменены дефисами, при совпадении — суффикс ` (2)`).
fn legacy_recording_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !ext.eq_ignore_ascii_case("wav") {
        return false;
    }
    let stem = strip_copy_suffix(stem);
    // Старый формат рекордера: `[12:00-12:30__26-10-2026].wav`.
    let stem = stem
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(stem);
    let Some((clock, date)) = stem.split_once("__") else {
        return false;
    };
    let clock_ok = clock.len() == 11
        && clock.as_bytes().iter().enumerate().all(|(i, c)| {
            if [2, 5, 8].contains(&i) {
                *c == b':' || *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    // Unix: `12:00-12:30`, Windows: `12-00-12-30`.
    let clock_ok = clock_ok && clock.as_bytes()[2] == clock.as_bytes()[8];
    let date_ok = date.len() == 10
        && date.as_bytes().iter().enumerate().all(|(i, c)| {
            if [2, 5].contains(&i) {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    clock_ok && date_ok
}

fn strip_copy_suffix(s: &str) -> &str {
    if !s.ends_with(')') {
        return s;
    }
    if let Some(i) = s.rfind(" (") {
        if s[i + 2..s.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            return &s[..i];
        }
    }
    s
}

impl Registry {
    pub(crate) fn is_owned(&self, path: &Path) -> bool {
        let key = normalize(path);
        if self.imported.iter().any(|p| p == &key) {
            return false;
        }
        self.owned.iter().any(|p| p == &key) || legacy_recording_name(path)
    }

    pub(crate) fn is_hidden(&self, path: &Path) -> bool {
        let key = normalize(path);
        self.hidden.iter().any(|p| p == &key)
    }
}

pub(crate) fn load_registry() -> Registry {
    load()
}

/// Запомнить файл, созданный рекордером.
pub(crate) fn mark_owned(path: &Path) {
    let key = normalize(path);
    update(|reg| {
        if !reg.owned.iter().any(|p| p == &key) {
            reg.owned.push(key);
        }
    });
}

pub(crate) fn unmark_owned(path: &Path) {
    let key = normalize(path);
    update(|reg| reg.owned.retain(|p| p != &key));
}

/// Запомнить файл, скопированный извне: даже если имя похоже на запись
/// рекордера, удалять его нельзя — только убирать из списка.
pub(crate) fn mark_imported(path: &Path) {
    let key = normalize(path);
    update(|reg| {
        if !reg.imported.iter().any(|p| p == &key) {
            reg.imported.push(key);
        }
    });
}

/// Убрать внешний файл из списка, не трогая сам файл.
pub(crate) fn hide(path: &Path) {
    let key = normalize(path);
    update(|reg| {
        if !reg.hidden.iter().any(|p| p == &key) {
            reg.hidden.push(key);
        }
    });
}

/// Переименование: путь переезжает и в списке своих, и в списке скрытых.
pub(crate) fn path_renamed(old: &Path, new: &Path) {
    let from = normalize(old);
    let to = normalize(new);
    update(|reg| {
        for list in [&mut reg.owned, &mut reg.imported, &mut reg.hidden] {
            for p in list.iter_mut() {
                if p == &from {
                    *p = to.clone();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_own_recording_names_only() {
        assert!(legacy_recording_name(Path::new(
            "12:00-12:30__26-10-2026.wav"
        )));
        assert!(legacy_recording_name(Path::new(
            "12-00-12-30__26-10-2026 (2).wav"
        )));
        assert!(legacy_recording_name(Path::new(
            "[18:16-18:16__25-10-2026].wav"
        )));
        assert!(!legacy_recording_name(Path::new("важная встреча.wav")));
        assert!(!legacy_recording_name(Path::new(
            "12:00-12:30__26-10-2026.mp3"
        )));
    }

    #[test]
    fn imported_flag_wins_over_recording_name() {
        let mut reg = Registry::default();
        let p = Path::new("12:00-12:30__26-10-2026.wav");
        assert!(reg.is_owned(p));
        reg.imported.push(normalize(p));
        assert!(!reg.is_owned(p), "импортированный файл нельзя удалять");
    }

    #[test]
    fn strip_copy_suffix_only_for_numbered_copies() {
        assert_eq!(strip_copy_suffix("запись (2)"), "запись");
        assert_eq!(strip_copy_suffix("запись (важно)"), "запись (важно)");
        assert_eq!(strip_copy_suffix("запись"), "запись");
    }
}
