mod labels;
mod merge;

#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use labels::switch_penalty;
pub(crate) use labels::{
    constraint_retry_needed, fallback_info, fill_labels_by_time, fill_unit_labels_by_time,
    log_cluster_summary, rms_quality,
};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use merge::merge_quiet_short;
pub(crate) use merge::{
    group_segments, merge_quiet_short_with_evidence, merge_tiny_clusters, short_join_threshold,
    smooth_labels_mono,
};
