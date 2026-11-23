mod auto_detect;
mod boot;
mod cli;
mod config;
mod crash;
mod desktop;
mod diagnostics;
mod ffmpeg_dl;
mod fsutil;
mod hotwords;
mod http;
mod ipc;
mod journal;
mod logging;
mod media_registry;
mod metrics;
mod privacy;
mod recording;
pub(crate) mod screen;
mod summary;
mod sysmem;
mod system;
#[cfg(feature = "tauri-ui")]
mod tauri_shell;
#[cfg(feature = "onnx")]
pub mod transcribe;
mod voiceprints;
#[cfg(feature = "onnx")]
pub use transcribe::{transcribe_file, TranscribeOptions};

#[cfg(test)]
mod tests;

use config::configured_record_mode;

#[cfg(feature = "tauri-ui")]
pub use boot::mobile_main;
pub(crate) use boot::opener_url;
pub use boot::run_cli;
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use boot::set_transcribing_for_test;
#[doc(hidden)]
pub use boot::{shutdown_summary_for_test, spawn_test_http_server};

#[cfg(test)]
pub(crate) use tests::TEST_ENV_LOCK;
