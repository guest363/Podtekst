//! Голосовые эмбеддинги через ONNX-экстрактор 3D-Speaker ERes2NetV2
//! (`eres2netv2_emb.onnx`, пространство `eres2netv2-192-v1`).
//! Вход — fbank [1, T, 80] (см. `features::eres2netv2_fbank`), выход — 192-мерный
//! L2-нормализованный вектор. Старые pyannote/wespeaker-модели (вход [1,T,80],
//! 256-dim) также принимаются — пространство различается размерностью вектора.

use std::collections::HashMap;

use super::features::eres2netv2_fbank;
use super::session::OrtSession;
use crate::l2_normalize;

pub struct EmbeddingExtractor {
    session: OrtSession,
    input_name: String,
}

impl EmbeddingExtractor {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let session = OrtSession::load(path, 0)?;
        let input_name = ["fbank_features", "speech", "fbank", "features", "x"]
            .iter()
            .find(|n| session.has_input(n))
            .map(|s| s.to_string())
            .or_else(|| session.input_names.first().cloned())
            .ok_or_else(|| "у embedding-модели нет входов".to_string())?;
        Ok(Self {
            session,
            input_name,
        })
    }

    /// Эмбеддинг целого фрагмента 16 кГц.
    /// Короткие фрагменты (< 0.4 с) отклоняются из-за недостатка речевых данных.
    pub fn embed(&mut self, samples_16k: &[f32]) -> Result<Option<Vec<f64>>, String> {
        if samples_16k.len() < (16000.0 * 0.4) as usize {
            return Ok(None);
        }
        let fb = eres2netv2_fbank(samples_16k);
        if fb.is_empty() {
            return Ok(None);
        }
        let t = fb.len();
        let flat: Vec<f32> = fb.into_iter().flatten().collect();
        let mut inputs = HashMap::new();
        inputs.insert(self.input_name.clone(), (vec![1i64, t as i64, 80], flat));
        let out = self.session.run_f32(inputs)?;
        let vec = out
            .values()
            .next()
            .map(|(_, d)| l2_normalize(&d.iter().map(|&x| x as f64).collect::<Vec<_>>()))
            .unwrap_or_default();
        if vec.is_empty() {
            return Ok(None);
        }
        Ok(Some(vec))
    }
}
