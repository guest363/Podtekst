use std::path::Path;

use serde::Serialize;

use crate::install::ensure_recovered;
use crate::manifest::{ModelEntry, ModelsManifest};
use crate::models_dir;
use crate::state::{load_state_in, InstalledInfo, ModelsState};

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub version: String,
    pub installed_version: Option<String>,
    pub installed_order: Option<u32>,
    pub is_installed: bool,
    pub has_update: bool,
    pub needs_reinstall: bool,
    pub total_size_bytes: u64,
    pub on_disk_bytes: u64,
    pub files_total: usize,
    pub files_present: usize,
    pub license: String,
    pub description: String,
    pub embedding_space: Option<String>,
    pub ram_mb: u64,
    pub required_free_mb: u64,
    pub context_tokens: u32,
    pub min_system_ram_mb: u64,
    pub license_url: String,
    pub source_url: String,
}

pub fn get_models_status(app_data: &Path, manifest: &ModelsManifest) -> Vec<ModelStatus> {
    let root = models_dir(app_data);
    ensure_recovered(&root);
    let state = load_state_in(&root);
    manifest
        .models
        .iter()
        .map(|m| status_for(&root, &state, m))
        .collect()
}

fn status_for(root: &Path, state: &ModelsState, m: &ModelEntry) -> ModelStatus {
    let total: u64 = m.files.iter().map(|f| f.size_bytes).sum();
    let dir = root.join(&m.id);
    let mut present = 0;
    let mut on_disk = 0u64;
    let mut size_ok = true;
    for f in &m.files {
        let Ok(md) = dir.join(&f.filename).metadata() else {
            continue;
        };
        if !md.is_file() {
            continue;
        }
        present += 1;
        on_disk += md.len();
        // Наличие файла ещё не целостность: обрезанную модель видно по размеру
        // без хеширования всего файла на каждый запрос статуса.
        if f.size_bytes > 0 && md.len() != f.size_bytes {
            size_ok = false;
        }
    }
    let record = state.installed.get(&m.id);
    let installed = size_ok && present == m.files.len() && !m.files.is_empty();
    // Записи нет — версию назвать нельзя, пока файлы не переустановлены
    // с проверкой SHA-256.
    let needs_reinstall = installed && record.is_none_or(|i| record_conflicts(m, i));
    let installed_order = record.and_then(|i| i.order);
    let has_update = installed
        && !needs_reinstall
        && match installed_order {
            Some(order) => order < m.order,
            None => record.is_some_and(|i| i.version != m.version),
        };
    ModelStatus {
        id: m.id.clone(),
        kind: m.kind.clone(),
        title: m.title.clone(),
        version: m.version.clone(),
        installed_version: record.map(|i| i.version.clone()),
        installed_order,
        is_installed: installed,
        has_update,
        needs_reinstall,
        total_size_bytes: total,
        on_disk_bytes: on_disk,
        files_total: m.files.len(),
        files_present: present,
        license: m.license.clone(),
        description: m.description.clone(),
        embedding_space: m.embedding_space.clone(),
        ram_mb: m.ram_mb,
        required_free_mb: m.required_free_mb,
        context_tokens: m.context_tokens,
        min_system_ram_mb: m.min_system_ram_mb,
        license_url: m.license_url.clone(),
        source_url: m.source_url.clone(),
    }
}

/// Запись противоречит манифесту. Записи старого формата sha не хранят:
/// сверять нечего, а размеры файлов уже сошлись — принудительная перекачка
/// нескольких гигабайт после обновления не нужна.
fn record_conflicts(entry: &ModelEntry, info: &InstalledInfo) -> bool {
    if info.files.is_empty() {
        return false;
    }
    info.files.len() != entry.files.len()
        || info.files.iter().any(|i| {
            !entry.files.iter().any(|f| {
                f.filename == i.filename
                    && i.size_bytes == f.size_bytes
                    && i.sha256.eq_ignore_ascii_case(&f.sha256)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ModelFile, Upstream};
    use crate::state::{save_state_in, InstalledFile};
    use crate::testdir;

    fn entry(order: u32) -> ModelEntry {
        ModelEntry {
            id: "m".to_string(),
            title: "t".to_string(),
            version: format!("v{order}"),
            order,
            upstream: Upstream {
                kind: "hf".to_string(),
                repo: "r/x".to_string(),
                revision: "abc".to_string(),
                version: order.to_string(),
            },
            files: vec![ModelFile {
                filename: "w.onnx".to_string(),
                url_hf: "https://huggingface.co/r/x/resolve/abc/w.onnx".to_string(),
                mirrors: vec![],
                size_bytes: 4,
                sha256: "a".repeat(64),
                optional: false,
            }],
            embedding_space: None,
            embedding_dim: None,
            license: String::new(),
            description: String::new(),
            kind: "asr".to_string(),
            tier: String::new(),
            ram_mb: 0,
            required_free_mb: 0,
            context_tokens: 0,
            min_system_ram_mb: 0,
            license_url: String::new(),
            source_url: String::new(),
        }
    }

    fn manifest(order: u32) -> ModelsManifest {
        ModelsManifest {
            manifest_version: 2,
            updated_at: String::new(),
            models: vec![entry(order)],
        }
    }

    fn record(order: Option<u32>, version: &str, match_files: bool) -> InstalledInfo {
        InstalledInfo {
            version: version.to_string(),
            order,
            files: if match_files {
                vec![InstalledFile {
                    filename: "w.onnx".to_string(),
                    sha256: "a".repeat(64),
                    size_bytes: 4,
                }]
            } else {
                vec![]
            },
            source_revision: Some("abc".to_string()),
            embedding_space: None,
            installed_at: String::new(),
        }
    }

    fn with_files(app_data: &Path) {
        let dir = models_dir(app_data).join("m");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("w.onnx"), b"abcd").unwrap();
    }

    fn status(
        app_data: &Path,
        manifest: &ModelsManifest,
        info: Option<InstalledInfo>,
    ) -> ModelStatus {
        let mut state = ModelsState::default();
        if let Some(info) = info {
            state.installed.insert("m".to_string(), info);
        }
        save_state_in(&models_dir(app_data), &state).unwrap();
        get_models_status(app_data, manifest).remove(0)
    }

    #[test]
    fn update_is_directed_by_order() {
        let newer = testdir::new("up-newer");
        with_files(&newer);
        let s = status(&newer, &manifest(2), Some(record(Some(1), "v1", true)));
        assert!(s.has_update, "order 1 < 2 — есть обновление");
        assert!(!s.needs_reinstall);
        assert_eq!(s.installed_order, Some(1));

        let same = testdir::new("up-same");
        with_files(&same);
        let s = status(&same, &manifest(2), Some(record(Some(2), "v2", true)));
        assert!(!s.has_update);

        let older = testdir::new("up-older");
        with_files(&older);
        let s = status(&older, &manifest(2), Some(record(Some(3), "v3", true)));
        assert!(!s.has_update, "откат версии не считается обновлением");
    }

    #[test]
    fn lost_state_asks_for_reinstall_without_update() {
        let tmp = testdir::new("state-lost");
        with_files(&tmp);
        let s = status(&tmp, &manifest(2), None);
        assert!(s.is_installed, "файлы на месте");
        assert!(s.installed_version.is_none());
        assert!(s.installed_order.is_none());
        assert!(s.needs_reinstall, "нечем подтвердить версию");
        assert!(!s.has_update, "без записи обновление не заявляем");
    }

    #[test]
    fn record_with_other_sha_requires_reinstall() {
        let tmp = testdir::new("reinstall");
        with_files(&tmp);
        let mut info = record(Some(1), "v1", true);
        info.files[0].sha256 = "b".repeat(64);
        let s = status(&tmp, &manifest(2), Some(info));
        assert!(s.needs_reinstall, "записанный sha не совпал с манифестом");
        assert!(
            !s.has_update,
            "пока файлы не переустановлены, обновление не заявляем"
        );
    }

    #[test]
    fn legacy_record_without_files_is_trusted() {
        let tmp = testdir::new("legacy-record");
        with_files(&tmp);
        let s = status(&tmp, &manifest(1), Some(record(None, "v1", false)));
        assert!(
            !s.needs_reinstall,
            "сверять нечего — файлы совпали по размеру"
        );
        assert!(!s.has_update);
    }

    #[test]
    fn legacy_record_without_order_falls_back_to_version() {
        let legacy = testdir::new("legacy-up");
        with_files(&legacy);
        let s = status(
            &legacy,
            &manifest(2),
            Some(InstalledInfo {
                version: "v1".to_string(),
                ..record(None, "v1", true)
            }),
        );
        assert!(s.has_update, "версия отличается — обновление");
        assert_eq!(s.installed_order, None);
    }

    #[test]
    fn status_requires_exact_file_size() {
        let tmp = testdir::new("size");
        let dir = models_dir(&tmp).join("m");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("w.onnx"), b"abc").unwrap();
        assert!(!get_models_status(&tmp, &manifest(1))[0].is_installed);
        std::fs::write(dir.join("w.onnx"), b"abcd").unwrap();
        assert!(get_models_status(&tmp, &manifest(1))[0].is_installed);
    }

    #[test]
    fn status_json_exposes_reinstall_flag_and_order() {
        let tmp = testdir::new("status-json");
        with_files(&tmp);
        let value = serde_json::to_value(status(&tmp, &manifest(1), None)).unwrap();
        assert_eq!(value["needs_reinstall"], true);
        assert!(
            value.as_object().unwrap().contains_key("installed_order"),
            "поле отдаётся даже при потере state.json"
        );
    }

    #[test]
    fn bundled_statuses_cover_every_model() {
        let tmp = testdir::new("bundled");
        let statuses = get_models_status(&tmp, crate::bundled());
        assert_eq!(statuses.len(), crate::bundled().models.len());
        assert!(statuses.iter().all(|s| !s.id.is_empty()));
        assert!(statuses.iter().all(|s| !s.is_installed && !s.has_update));
    }
}
