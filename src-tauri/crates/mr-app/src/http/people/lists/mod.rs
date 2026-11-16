mod external;
mod packs;

pub(crate) use external::{
    load_external_people, read_people_list, same_name, save_external_people, update_people,
};
pub(super) use packs::{ensure_person_in_pack, people_json_files, resolve_target_pack};
pub(crate) use packs::{
    handle_people_add, handle_people_move, migrate_voiceprints_v3, remove_person,
    sync_voiceprint_people,
};
