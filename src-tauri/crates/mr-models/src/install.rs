use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::download::{download_with_resume, file_matches, tmp_dir, DownloadLock, ProgressEvent};
use crate::manifest::{ModelEntry, ModelFile, ModelsManifest};
use crate::state::{update_state, InstalledFile, InstalledInfo};
use crate::{is_placeholder_sha, models_dir, sha256_of_file, ModelsError, Result};

const STAGING_SUFFIX: &str = ".new";
const OLD_MARK: &str = ".old-";
/// Докачка, брошенная больше месяца назад, вряд ли продолжится: Range с чужого
/// смещения бесполезен, а место файл занимает.
const PART_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Запас сверх двух полных наборов: манифест, логи и WAV-заголовки записи.
const SPACE_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

/// Одновременно в процессе идёт максимум одна установка: иначе два потока
/// качают набор в один staging и публикуют его по очереди.
static INSTALL_SLOT: Mutex<()> = Mutex::new(());

/// Межпроцессный замок установки на файле: ОС снимает его сама при падении,
/// поэтому второй экземпляр приложения не должен «восстанавливать» чужой staging.
pub(crate) struct InstallLock {
    _file: fs::File,
}

fn lock_install(root: &Path) -> Option<InstallLock> {
    fs::create_dir_all(root).ok()?;
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(root.join(".install.lock"))
        .ok()?;
    match file.try_lock() {
        Ok(()) => Some(InstallLock { _file: file }),
        Err(_) => None,
    }
}

fn begin_install(root: &Path) -> Result<(std::sync::MutexGuard<'static, ()>, InstallLock)> {
    let slot = match INSTALL_SLOT.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return Err(ModelsError::Other("установка модели уже идёт".to_string()));
        }
    };
    let lock = lock_install(root).ok_or_else(|| {
        ModelsError::Other("установка модели уже идёт в другом экземпляре приложения".to_string())
    })?;
    Ok((slot, lock))
}

/// Скачивать набор нужно с запасом на второй экземпляр: старый каталог живёт
/// до успешной публикации нового.
pub fn required_space(total_bytes: u64) -> u64 {
    total_bytes
        .saturating_mul(2)
        .saturating_add(SPACE_RESERVE_BYTES)
}

pub fn check_space(root: &Path, total_bytes: u64) -> Result<()> {
    let need = required_space(total_bytes);
    let free = fs2::available_space(root).unwrap_or(u64::MAX);
    if free < need {
        return Err(ModelsError::NoSpace {
            need_bytes: need,
            free_bytes: free,
        });
    }
    Ok(())
}

/// Убрать следы прерванной установки: недокачанный staging (`<id>.new`) и
/// вернуть на место каталог, унесённый в `<id>.old-<pid>`. Недокачанные
/// `.part` в `<id>/.tmp` переживают перезапуск: докачка продолжается по Range.
///
/// Вызывается один раз при первом обращении к статусу (UI спрашивает его сразу
/// после старта) и перед каждой установкой: посреди работающей установки
/// восстановление уничтожило бы staging, поэтому его пропускают.
pub fn recover_interrupted(root: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    let mut olds: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if name.ends_with(STAGING_SUFFIX) {
            let _ = fs::remove_dir_all(&path);
        } else if name.contains(OLD_MARK) {
            let at = fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            olds.push((at, path));
        } else {
            remove_stale_parts(&path);
        }
    }
    // Самый свежий каталог восстанавливаем первым: он и есть последняя версия.
    olds.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (_, old) in olds {
        let Some(base) = old
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .and_then(|name| name.split_once(OLD_MARK).map(|(base, _)| base.to_string()))
        else {
            continue;
        };
        let target = root.join(base);
        if target.exists() {
            let _ = fs::remove_dir_all(&old);
        } else {
            let _ = fs::rename(&old, &target);
        }
    }
    Ok(())
}

fn remove_stale_parts(model_dir: &Path) {
    let Ok(entries) = fs::read_dir(tmp_dir(model_dir)) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().ends_with(".part") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|at| now.duration_since(at).is_ok_and(|age| age > PART_MAX_AGE))
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub(crate) fn ensure_recovered(root: &Path) {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.get().is_some() {
        return;
    }
    // Чужой процесс ставит модель: восстановление уничтожило бы его staging.
    // Флаг «выполнено» ставим только после реальной работы.
    let Some(_lock) = lock_install(root) else {
        return;
    };
    if INSTALL_SLOT.try_lock().is_err() {
        return;
    }
    let _ = recover_interrupted(root);
    let _ = DONE.set(());
}

pub fn download_or_update_model(
    app_data: &Path,
    manifest: &ModelsManifest,
    model_id: &str,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<()> {
    let spec = manifest
        .models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| ModelsError::NotFound(model_id.to_string()))?;
    // Путь строим из id манифеста: даже при коллизии входа инвариант не зависит от порядка.
    let root = models_dir(app_data);
    let _install = begin_install(&root)?;
    recover_interrupted(&root)?;
    let total: u64 = spec.files.iter().map(|f| f.size_bytes).sum();
    check_space(&root, total)?;
    let staging = root.join(format!("{}{STAGING_SUFFIX}", spec.id));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    // `.tmp` живёт в рабочем каталоге модели, а не в staging: перезапуск
    // удаляет staging, но обязан сохранить `.part` для докачки.
    let tmp = tmp_dir(&root.join(&spec.id));
    fs::create_dir_all(&tmp)?;
    for file in &spec.files {
        fetch_file(&tmp, &staging, file, on_progress)?;
    }
    publish(&root, &staging, &spec.id)?;
    let info = installed_info(spec);
    update_state(&root, |state| {
        state.installed.insert(spec.id.clone(), info);
    })
}

fn installed_info(spec: &ModelEntry) -> InstalledInfo {
    InstalledInfo {
        version: spec.version.clone(),
        order: Some(spec.order),
        files: spec
            .files
            .iter()
            .map(|f| InstalledFile {
                filename: f.filename.clone(),
                sha256: f.sha256.to_ascii_lowercase(),
                size_bytes: f.size_bytes,
            })
            .collect(),
        source_revision: Some(spec.upstream.revision.clone()).filter(|r| !r.is_empty()),
        embedding_space: spec.embedding_space.clone(),
        installed_at: now_secs(),
    }
}

fn now_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

fn fetch_file(
    tmp: &Path,
    staging: &Path,
    file: &ModelFile,
    on_progress: &dyn Fn(ProgressEvent),
) -> Result<()> {
    let target = staging.join(&file.filename);
    if target.is_file() && file_matches(&target, file.size_bytes, &file.sha256) {
        return Ok(());
    }
    let part = tmp.join(format!("{}.part", file.filename));
    let _lock = DownloadLock::acquire(tmp, &file.filename)?;
    if part.is_file() && file_matches(&part, file.size_bytes, &file.sha256) {
        let _ = fs::remove_file(&target);
        fs::rename(&part, &target)?;
        return Ok(());
    }
    let mut last: Option<ModelsError> = None;
    for url in std::iter::once(&file.url_hf)
        .chain(file.mirrors.iter())
        .filter(|u| !u.is_empty())
    {
        match download_with_resume(url, &part, file.size_bytes, &|| false, on_progress) {
            Ok(()) => match verify_file(&part, file) {
                Ok(()) => {
                    // Windows не переименовывает поверх существующего файла.
                    let _ = fs::remove_file(&target);
                    fs::rename(&part, &target)?;
                    return Ok(());
                }
                Err(error) => {
                    let _ = fs::remove_file(&part);
                    last = Some(error);
                }
            },
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| ModelsError::Other(format!("не удалось скачать {}", file.filename))))
}

fn verify_file(path: &Path, file: &ModelFile) -> Result<()> {
    let len = fs::metadata(path)?.len();
    if file.size_bytes > 0 && len != file.size_bytes {
        return Err(ModelsError::Net(format!(
            "{}: {} из {} байт",
            file.filename, len, file.size_bytes
        )));
    }
    if is_placeholder_sha(&file.sha256) {
        return Ok(());
    }
    let actual = sha256_of_file(path)?;
    if !actual.eq_ignore_ascii_case(&file.sha256) {
        return Err(ModelsError::ShaMismatch {
            file: file.filename.clone(),
            expected: file.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

/// Публикация набора: рабочий каталог уносится в `<id>.old-<pid>`, staging
/// встаёт на его место. Пока новая версия не встала, старая остаётся целой.
fn publish(root: &Path, staging: &Path, id: &str) -> Result<()> {
    let target = root.join(id);
    let old = root.join(format!("{id}{OLD_MARK}{}", std::process::id()));
    if target.exists() {
        fs::rename(&target, &old)?;
    }
    match fs::rename(staging, &target) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&old);
            Ok(())
        }
        Err(error) => {
            if old.exists() {
                let _ = fs::rename(&old, &target);
            }
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir;

    fn model_file(name: &str) -> ModelFile {
        ModelFile {
            filename: name.to_string(),
            url_hf: "https://example.invalid/x".to_string(),
            mirrors: vec![],
            size_bytes: 0,
            sha256: "0".repeat(64),
            optional: false,
        }
    }

    #[test]
    fn installed_info_keeps_order_files_and_revision() {
        let spec = ModelEntry {
            id: "m".to_string(),
            title: "t".to_string(),
            version: "2".to_string(),
            order: 2,
            upstream: crate::manifest::Upstream {
                kind: "hf".to_string(),
                repo: "r/x".to_string(),
                revision: "abc".to_string(),
                version: "2".to_string(),
            },
            files: vec![model_file("a.onnx")],
            embedding_space: None,
            embedding_dim: None,
            license: String::new(),
            description: String::new(),
            title_en: String::new(),
            kind: "asr".to_string(),
            tier: String::new(),
            ram_mb: 0,
            required_free_mb: 0,
            context_tokens: 0,
            min_system_ram_mb: 0,
            license_url: String::new(),
            source_url: String::new(),
        };
        let info = installed_info(&spec);
        assert_eq!(info.order, Some(2));
        assert_eq!(info.source_revision.as_deref(), Some("abc"));
        assert_eq!(info.files.len(), 1);
        assert_eq!(info.files[0].filename, "a.onnx");
    }

    #[test]
    fn space_reserves_two_copies_and_headroom() {
        assert_eq!(required_space(1000), 1000 * 2 + SPACE_RESERVE_BYTES);
        assert_eq!(required_space(u64::MAX), u64::MAX, "без переполнения");
    }

    #[test]
    fn staging_is_removed_and_part_survives_crash_on_second_file() {
        let tmp = testdir::new("recover-new");
        let root = models_dir(&tmp);
        let model = root.join("m");
        fs::create_dir_all(model.join(".tmp")).unwrap();
        fs::write(model.join("a.onnx"), b"old").unwrap();
        // Крах посреди скачивания: staging с первым файлом и `.part` второго.
        let staging = root.join("m.new");
        fs::create_dir_all(staging.join(".tmp")).unwrap();
        fs::write(staging.join("a.onnx"), b"new").unwrap();
        fs::write(model.join(".tmp/b.onnx.part"), b"part").unwrap();
        recover_interrupted(&root).unwrap();
        assert!(!staging.exists(), "недокачанный staging удалён");
        assert_eq!(fs::read(model.join("a.onnx")).unwrap(), b"old".to_vec());
        assert!(
            model.join(".tmp/b.onnx.part").is_file(),
            "недокачанный .part переживает перезапуск"
        );
    }

    #[test]
    fn stale_parts_are_removed_and_fresh_kept() {
        let tmp = testdir::new("recover-parts");
        let root = models_dir(&tmp);
        let parts = root.join("m/.tmp");
        fs::create_dir_all(&parts).unwrap();
        let stale = parts.join("old.onnx.part");
        let fresh = parts.join("new.onnx.part");
        fs::write(&stale, b"old").unwrap();
        fs::write(&fresh, b"new").unwrap();
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - (PART_MAX_AGE + Duration::from_secs(1)))
            .unwrap();
        recover_interrupted(&root).unwrap();
        assert!(!stale.exists(), "протухший .part удалён");
        assert!(fresh.is_file(), "свежий .part сохранён для докачки");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn restart_resumes_part_by_range() {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};

        let tmp = testdir::new("resume");
        let root = models_dir(&tmp);
        let model = root.join("m");
        let staging = root.join("m.new");
        fs::create_dir_all(model.join(".tmp")).unwrap();
        let body = b"0123456789abcdef".to_vec();
        let prefix = 6;
        fs::write(model.join(".tmp/a.onnx.part"), &body[..prefix]).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/a.onnx", listener.local_addr().unwrap());
        let expected = body.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().ok()?;
            let mut head = [0u8; 4096];
            let n = stream.read(&mut head).ok()?;
            let request = String::from_utf8_lossy(&head[..n]).to_string();
            let range = format!("range: bytes={prefix}-");
            if !request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&range))
            {
                return None;
            }
            let rest = &expected[prefix..];
            let response = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                rest.len(),
                prefix,
                expected.len() - 1,
                expected.len()
            );
            stream.write_all(response.as_bytes()).ok()?;
            stream.write_all(rest).ok()?;
            Some(request)
        });

        recover_interrupted(&root).unwrap();
        assert!(
            model.join(".tmp/a.onnx.part").is_file(),
            "докачка сохранена"
        );
        fs::create_dir_all(&staging).unwrap();
        let file = ModelFile {
            filename: "a.onnx".to_string(),
            url_hf: url,
            mirrors: vec![],
            size_bytes: body.len() as u64,
            sha256: crate::hex::encode(Sha256::digest(&body)),
            optional: false,
        };
        fetch_file(&model.join(".tmp"), &staging, &file, &|_| {}).expect("докачка");
        assert_eq!(fs::read(staging.join("a.onnx")).unwrap(), body);
        assert!(!model.join(".tmp/a.onnx.part").exists());
        assert!(server.join().unwrap().is_some(), "запрос Range");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn failed_publish_keeps_previous_version() {
        let tmp = testdir::new("publish-fail");
        let root = models_dir(&tmp);
        fs::create_dir_all(root.join("m")).unwrap();
        fs::write(root.join("m/a.onnx"), b"old").unwrap();
        // Без нового каталога rename невозможен — откат к старой версии.
        assert!(publish(&root, &root.join("m.new"), "m").is_err());
        assert_eq!(fs::read(root.join("m/a.onnx")).unwrap(), b"old".to_vec());
    }

    #[test]
    fn newest_old_copy_is_restored_when_model_dir_is_gone() {
        let tmp = testdir::new("recover-old");
        let root = models_dir(&tmp);
        for (pid, payload) in [(10, b"older".as_slice()), (20, b"newest".as_slice())] {
            let dir = root.join(format!("m.old-{pid}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("a.onnx"), payload).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        recover_interrupted(&root).unwrap();
        assert_eq!(
            fs::read(root.join("m").join("a.onnx")).unwrap(),
            b"newest".to_vec(),
            "восстановлен самый свежий каталог"
        );
        assert!(!root.join("m.old-10").exists());
        assert!(!root.join("m.old-20").exists());
    }

    #[test]
    fn extra_old_copies_are_dropped_when_model_dir_exists() {
        let tmp = testdir::new("recover-extra");
        let root = models_dir(&tmp);
        fs::create_dir_all(root.join("m")).unwrap();
        fs::create_dir_all(root.join("m.old-1")).unwrap();
        recover_interrupted(&root).unwrap();
        assert!(root.join("m").is_dir());
        assert!(!root.join("m.old-1").exists());
    }

    #[test]
    fn unknown_model_errors() {
        let tmp = testdir::new("nf");
        let r = download_or_update_model(&tmp, crate::bundled(), "no-such-model", &|_| {});
        assert!(matches!(r, Err(ModelsError::NotFound(_))));
    }

    #[test]
    fn verify_file_rejects_truncated_download() {
        let tmp = testdir::new("verify");
        let mut file = model_file("a.onnx");
        file.size_bytes = 8;
        file.sha256 = String::new();
        let path = tmp.join("a.onnx");
        fs::write(&path, b"123").unwrap();
        assert!(verify_file(&path, &file).is_err(), "недокачанный файл");
        fs::write(&path, b"12345678").unwrap();
        assert!(
            verify_file(&path, &file).is_ok_and(|_| true),
            "размер сошёлся, sha — заглушка"
        );
    }
}
