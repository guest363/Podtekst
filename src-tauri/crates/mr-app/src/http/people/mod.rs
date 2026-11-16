use std::sync::Mutex;

mod lists;
#[cfg(test)]
mod tests;
mod transcript_names;
mod voiceprints;

pub(super) use lists::{
    handle_people_add, handle_people_move, load_external_people, read_people_list, remove_person,
    same_name, save_external_people, update_people,
};
pub(crate) use lists::{migrate_voiceprints_v3, sync_voiceprint_people};
pub(super) use voiceprints::{
    handle_voiceprint_delete, handle_voiceprint_learn_sample, handle_voiceprint_sample_delete,
    handle_voiceprint_save, load_all_voiceprints, serve_voiceprint_detail,
};

/// Сериализует read-modify-write `voiceprints.json`: иначе два параллельных
/// сохранения голоса теряют одно из изменений.
static VOICEPRINTS_LOCK: Mutex<()> = Mutex::new(());
/// То же для списков участников (`people.json`, `external_people.json`):
/// `write_atomic` гарантирует целостность файла, но не слияние, поэтому
/// «прочитать → изменить → записать» из двух вкладок теряло добавленного гостя.
static PEOPLE_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn lock_voiceprints() -> std::sync::MutexGuard<'static, ()> {
    VOICEPRINTS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn lock_people() -> std::sync::MutexGuard<'static, ()> {
    PEOPLE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
