mod bounds;
mod pauses;
mod speaker;

pub(crate) use bounds::split_utterance_by_bounds;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use bounds::split_words_by_bounds;
pub(crate) use pauses::{split_at_pauses, split_by_word_pauses};
pub(crate) use speaker::{local_region, split_long_by_speaker_change};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use speaker::{speaker_change_cuts, split_window_region};
