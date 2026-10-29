//! ONNX-инференс для Meeting Recorder (фича `onnx`, крейт `ort`).
//!
//! Модели (см. `mr-models/models-manifest.json`):
//! * `vad-silero-v6` — Silero VAD v6.2.3, streaming с состоянием [2,1,128]
//! * `asr-gigaam-v3-rnnt-int8` — encoder/decoder/joint + greedy RNNT decode
//! * `diar-pyannote-segmentation-3` — PyAnnote 3.0 Powerset, речь и перекрытия
//! * `diar-eres2netv2-emb` — ERes2NetV2 (3D-Speaker) по fbank [1,T,80], 192-dim

pub mod embed;
pub mod features;
pub mod rnnt;
pub mod segmentation;
pub mod session;
pub mod vad;
pub mod whisper;

pub use session::OrtSession;
