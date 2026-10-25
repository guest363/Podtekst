//! Подключаемый справочник участников из Markdown-базы знаний
//! (Obsidian/репозиторий: таблицы `| Роль | Сотрудник |`, заголовки `### Имя (Роль)`, списки).
//!
//! Пути сканирования настраиваются в `config.json → knowledge_dirs`.
//! По умолчанию (пустые настройки) ничего не сканирует — приватность по умолчанию.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeopleEntry {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub is_external: bool,
}

/// Общие слова, не являющиеся именами (без привязки к конкретной компании).
const COMMON_NON_NAME_WORDS: &[&str] = &[
    "Для",
    "Сущность",
    "Распоряжение",
    "Подтверждение",
    "Штраф",
    "Роли",
    "Локальная",
    "Проект",
    "Раздел",
    "База",
    "Справка",
    "Отчет",
    "Отчёт",
    "Задача",
    "План",
    "Инструкция",
    "Пример",
    "Версия",
    "Статус",
    "Ошибка",
    "Сервер",
    "Канал",
    "Тест",
    "Стенд",
    "Среда",
    "Клиент",
    "Договор",
    "Правило",
    "Регламент",
];

fn is_cyrillic_upper(c: char) -> bool {
    ('А'..='Я').contains(&c) || c == 'Ё' || c.is_ascii_uppercase()
}
fn is_cyrillic_lower(c: char) -> bool {
    ('а'..='я').contains(&c) || c == 'ё' || c.is_ascii_lowercase()
}
fn is_name_word(w: &str) -> bool {
    // Двойные имена через дефис: «Анна-Мария» — каждое слово с заглавной.
    w.split('-').all(|part| {
        let mut ch = part.chars();
        match ch.next() {
            Some(first) if is_cyrillic_upper(first) => ch.all(is_cyrillic_lower),
            _ => false,
        }
    }) && w.chars().count() >= 2
}

fn is_name(value: &str) -> bool {
    let v = value.trim();
    if v.len() < 3 || v.len() > 70 {
        return false;
    }
    if !v
        .chars()
        .all(|c| c.is_alphabetic() || c == '-' || c == ' ' || c == 'Ё' || c == 'ё')
    {
        return false;
    }
    let words: Vec<&str> = v.split_whitespace().collect();
    if !(2..=3).contains(&words.len()) {
        return false;
    }
    if words.iter().any(|w| COMMON_NON_NAME_WORDS.contains(w)) {
        return false;
    }
    if words
        .iter()
        .any(|w| w.len() > 1 && w.chars().all(|c| c.is_uppercase()))
    {
        return false;
    }
    words.iter().all(|w| is_name_word(w))
}

fn split_row(line: &str) -> Vec<String> {
    let s = line.trim();
    if !s.starts_with('|') {
        return vec![];
    }
    s.trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_string())
        .collect()
}

fn clean_cell(cell: &str) -> String {
    let mut v = cell.trim().to_string();
    while v.starts_with('`') && v.ends_with('`') && v.len() >= 2 {
        v = v[1..v.len() - 1].trim().to_string();
    }
    if v.starts_with("**") && v.ends_with("**") && v.len() >= 4 {
        v = v[2..v.len() - 2].to_string();
    }
    // [текст](ссылка) → текст
    if v.starts_with('[') {
        if let Some(end) = v.find("](") {
            v = v[1..end].to_string();
        }
    }
    v.trim().to_string()
}

fn parse_file(path: &Path, domain: &str) -> Vec<PeopleEntry> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let mut out = vec![];
    for raw_line in text.lines() {
        let line = raw_line.trim();
        // Заголовки ### Имя (Роль) и списки - Имя (Роль)
        let head = line
            .strip_prefix("#### ")
            .or_else(|| line.strip_prefix("### "))
            .or_else(|| line.strip_prefix("## "))
            .or_else(|| line.strip_prefix("- "))
            .or_else(|| line.strip_prefix("* "));
        if let Some(h) = head {
            let (name_part, role_part) =
                match h.find('(').and_then(|i| h.rfind(')').map(|j| (i, j))) {
                    Some((i, j)) if j > i => (h[..i].trim(), h[i + 1..j].trim()),
                    _ => (h.trim(), ""),
                };
            if is_name(name_part) {
                out.push(PeopleEntry {
                    name: name_part.to_string(),
                    role: if role_part.is_empty() {
                        "Участник команды".to_string()
                    } else {
                        role_part.to_string()
                    },
                    domain: domain.to_string(),
                    source: path.to_string_lossy().to_string(),
                    is_external: false,
                });
                continue;
            }
        }
        // Markdown-таблицы
        let cells = split_row(line);
        if cells.len() < 2 {
            continue;
        }
        let cleaned: Vec<String> = cells.iter().map(|c| clean_cell(c)).collect();
        if cleaned
            .iter()
            .filter(|c| !c.is_empty())
            .all(|c| c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue;
        }
        let role_hint = cleaned[0].chars().take(80).collect::<String>();
        for cell in cleaned.iter().skip(1) {
            if is_name(cell) {
                out.push(PeopleEntry {
                    name: cell.clone(),
                    role: role_hint.clone(),
                    domain: domain.to_string(),
                    source: path.to_string_lossy().to_string(),
                    is_external: false,
                });
            }
        }
    }
    out
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".next"
            | ".git"
            | ".cache"
            | "dist"
            | "build"
            | ".venv"
            | "__pycache__"
            | "target"
    )
}

fn collect_md_files(root: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            if !p
                .file_name()
                .map(|n| is_skipped_dir(&n.to_string_lossy()))
                .unwrap_or(false)
            {
                collect_md_files(&p, out);
            }
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            out.push(p);
        }
    }
}

/// Сканирует директории базы знаний (макс. 400 файлов). Возвращает записи (ФИО, роль).
pub fn scan_knowledge_dirs(roots: &[PathBuf]) -> Vec<PeopleEntry> {
    let mut files = vec![];
    for root in roots {
        if root.is_dir() {
            collect_md_files(root, &mut files);
        }
    }
    files.truncate(400);
    files.sort();
    let mut out = vec![];
    for f in files {
        let root = roots
            .iter()
            .find(|r| f.starts_with(r))
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        let domain = f
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "knowledge".to_string());
        let _ = root;
        out.extend(parse_file(&f, &domain));
    }
    // Дедуп по имени (более длинная роль побеждает)
    let mut by_name: HashMap<String, PeopleEntry> = HashMap::new();
    for p in out {
        by_name
            .entry(p.name.clone())
            .and_modify(|e| {
                if p.role.len() > e.role.len() {
                    e.role = p.role.clone();
                }
            })
            .or_insert(p);
    }
    let mut res: Vec<PeopleEntry> = by_name.into_values().collect();
    res.sort_by(|a, b| {
        let ka = a.name.split_whitespace().last().unwrap_or("");
        let kb = b.name.split_whitespace().last().unwrap_or("");
        ka.cmp(kb)
    });
    res
}

/// Читает `knowledge_dirs` из `<app_data>/config.json`.
pub fn configured_roots(app_data: &Path) -> Vec<PathBuf> {
    let cfg_path = app_data.join("config.json");
    let text = match std::fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut roots = vec![];
    let mut seen = HashSet::new();
    if let Some(arr) = v.get("knowledge_dirs").and_then(|x| x.as_array()) {
        for item in arr.iter().filter_map(|x| x.as_str()) {
            let p = if let Some(rest) = item.strip_prefix("~/") {
                match std::env::var("HOME") {
                    Ok(h) => PathBuf::from(h).join(rest),
                    Err(_) => continue,
                }
            } else {
                PathBuf::from(item)
            };
            if p.is_dir() && seen.insert(p.clone()) {
                roots.push(p);
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_knowledge() -> (PathBuf, tempfile_guard::Guard) {
        let dir = std::env::temp_dir().join(format!(
            "mr-dir-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("team")).unwrap();
        let guard = tempfile_guard::Guard(dir.clone());
        (dir, guard)
    }

    mod tempfile_guard {
        use std::path::PathBuf;
        pub struct Guard(pub PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn parses_tables_headers_and_lists() {
        let (kb, _g) = tmp_knowledge();
        std::fs::write(
            kb.join("team").join("README.md"),
            "| Роль | Сотрудник |\n|---|---|\n| Backend | Иван Петров |\n\n### Мария Сидорова (QA)\n\n- Петр Иванов (DevOps)\n",
        )
        .unwrap();
        let people = scan_knowledge_dirs(&[kb.join("team")]);
        let names: Vec<_> = people.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Иван Петров"), "{names:?}");
        assert!(names.contains(&"Мария Сидорова"), "{names:?}");
        assert!(names.contains(&"Петр Иванов"), "{names:?}");
        let ivan = people.iter().find(|p| p.name == "Иван Петров").unwrap();
        assert!(ivan.role.contains("Backend"));
    }

    #[test]
    fn rejects_non_names() {
        assert!(!is_name("База Знаний"));
        assert!(!is_name("Иван"));
        assert!(!is_name("A"));
        assert!(is_name("Анна-Мария Соколова"));
    }
}
