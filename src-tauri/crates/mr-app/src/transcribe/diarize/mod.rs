pub(super) mod acoustic_units;
mod acoustic_windows;
mod constraint_links;
mod core;
mod coverage;
mod diagnostics;
mod profiles;
mod reconcile;
mod segments;
mod split;
#[cfg(test)]
mod tests;
mod text;
mod turn_anchors;

pub(crate) use core::diarize_channel;
pub(crate) use profiles::load_profiles;
pub(crate) use reconcile::{
    dedupe_cross_channel, merge_utterances, merge_verified_cards, reconcile_channels,
    refresh_speaker_stats,
};
pub(crate) use segments::{group_segments, merge_quiet_short_with_evidence};
pub(crate) use split::{
    split_at_pauses, split_by_word_pauses, split_long_by_speaker_change, split_utterance_by_bounds,
};
pub(crate) use text::{move_boundary_words, normalize_word, words_match};

#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use profiles::read_profiles_file;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use segments::merge_quiet_short;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use split::split_words_by_bounds;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use text::text_similarity;

#[cfg_attr(not(test), allow(unused_imports))]
pub(super) use segments::smooth_labels_mono;

pub(super) use profiles::pick_samples;
pub(super) use segments::rms_quality;
pub(super) use split::local_region;

// Калибровка ERes2NetV2 (onnx_real): на 0.6 с сходство со своим голосом 0.41–0.79.
const CORE_MIN_DUR: f64 = 1.0;
/// Замеры ERes2NetV2 (`onnx_real`): разные дикторы ≤ 0.22 (dist ≥ 0.78),
/// один голос ≥ 0.48 (dist ≤ 0.52). Замер на AMI (real10, 4 файла по 120 с):
/// слияние двух разных голосов в average linkage — 0.519, силуэт и чистота
/// разбиения по эталону максимальны при остановке перед ним (0.88 → 1.00);
/// 0.51 оставляет границу внутри измеренного диапазона «один голос» 0.48–0.52.
const CORE_LINKAGE_THRESHOLD: f64 = 0.51;

/// Обычный порог имени профиля. Кластеры 0,9–2 с получают имя только через
/// исключение `MIN_SHORT_NAME_SEC`.
pub(super) const MIN_NAMED_CLEAN_SEC: f64 = 2.0;

/// Нижняя граница непрерывной локальной речи для эмбеддинга: на более коротком
/// куске ERes2NetV2 не набирает стабильный вектор голоса.
pub(super) const MIN_LOCAL_EMBED_SEC: f64 = 0.8;

/// Короткий кластер можно назвать только при почти точном совпадении с
/// профилем: 1,5–2 с чистого аудио уже дают устойчивый вектор, но обычный
/// порог имени остаётся 2 с. Нижняя граница исключения — 0,9 с: так узнаётся
/// образец, записанный с той же реплики, которая позже стала отдельным
/// коротким кластером.
pub(super) const MIN_SHORT_NAME_SEC: f64 = 0.9;
pub(super) const SHORT_NAME_MIN_SIM: f64 = 0.95;
pub(super) const SHORT_NAME_MIN_MARGIN: f64 = 0.05;

struct AcoustUnit {
    utterance: usize,
    start: f64,
    end: f64,
    dur: f64,
    quality: f64,
    embedding: Vec<f64>,
}
