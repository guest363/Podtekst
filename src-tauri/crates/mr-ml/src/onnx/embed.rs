//! Голосовые эмбеддинги через ONNX-экстрактор 3D-Speaker ERes2NetV2
//! (`eres2netv2_emb.onnx`, пространство `eres2netv2-192-v1`).
//! Вход — fbank [1, T, 80] (см. `features::eres2netv2_fbank`), выход — 192-мерный
//! L2-нормализованный вектор. Старые pyannote/wespeaker-модели (вход [1,T,80],
//! 256-dim) также принимаются — пространство различается размерностью вектора.

use std::collections::HashMap;

use super::features::eres2netv2_fbank;
use super::session::OrtSession;
use crate::{l2_normalize, EMBEDDING_DIM};

const LEGACY_EMBEDDING_DIM: usize = 256;

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
        // У модели несколько выходов, порядок HashMap не гарантирован:
        // выбираем вектор по размерности пространства (192) или legacy pyannote (256).
        let Some((_, data)) = [EMBEDDING_DIM, LEGACY_EMBEDDING_DIM]
            .iter()
            .find_map(|&dim| out.values().find(|(_, d)| d.len() == dim))
        else {
            return Ok(None);
        };
        // Нулевой вектор модель отдаёт на тишине: он не несёт голоса и,
        // попав в прототипы, одинаково «близок» ко всем дикторам.
        let norm = data
            .iter()
            .map(|&x| (x as f64) * (x as f64))
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm < 1e-6 {
            return Ok(None);
        }
        Ok(Some(l2_normalize(
            &data.iter().map(|&x| x as f64).collect::<Vec<_>>(),
        )))
    }
}
