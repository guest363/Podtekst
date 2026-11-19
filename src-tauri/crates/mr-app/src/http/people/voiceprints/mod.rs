mod handlers;
mod samples;
mod snippets;
mod store;

pub(crate) use handlers::{
    delete_voiceprint_internal, handle_voiceprint_delete, handle_voiceprint_learn_sample,
    handle_voiceprint_save,
};
pub(crate) use samples::{handle_voiceprint_sample_delete, serve_voiceprint_detail};
pub(crate) use snippets::load_all_voiceprints;
pub(super) use store::{
    delete_voiceprint_files, pack_voiceprint_file, parse_profiles, prototypes_centroid,
    store_voiceprint, vp_v3, vp_v3_with_space, PrototypeSample,
};

#[cfg(test)]
pub(crate) use samples::remove_prototype;
