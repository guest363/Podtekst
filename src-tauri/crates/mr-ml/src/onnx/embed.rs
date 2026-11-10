//! Голосовые эмбеддинги через ONNX-экстрактор 3D-Speaker ERes2NetV2
//! (`eres2netv2_emb.onnx`, пространство `eres2netv2-192-v1`).
//! Вход — fbank [N, T, 80] (см. `features::eres2netv2_fbank`), выход — N
//! 192-мерных L2-нормализованных вектора. Старые pyannote/wespeaker-модели
//! (вход [N,T,80], 256-dim) также принимаются — пространство различается
//! размерностью вектора.

use std::collections::{BTreeMap, HashMap};

use super::features::eres2netv2_fbank;
use super::session::OrtSession;
use crate::{l2_normalize, EMBEDDING_DIM};

const LEGACY_EMBEDDING_DIM: usize = 256;
const MIN_SAMPLES: usize = 6_400;
/// Батч только для окон одинаковой длины: ERes2NetV2 чувствителен к нулевому
/// паддингу, поэтому разнородные окна считаются по одному.
pub const EMBED_BATCH: usize = 8;

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
        let mut model = Self {
            session,
            input_name,
        };
        // Первый прогон выделяет арены ORT; без него он падает на первой реплике.
        let started = std::time::Instant::now();
        if let Err(error) = model.embed(&vec![0.0; 16_000]) {
            log::debug!(
                target: "model",
                error = error;
                "ev=model.warmup"
            );
        } else {
            log::debug!(
                target: "model",
                elapsed_ms = started.elapsed().as_millis() as u64;
                "ev=model.warmup"
            );
        }
        Ok(model)
    }

    /// Короткие фрагменты (< 0.4 с) отклоняются из-за недостатка речевых данных.
    pub fn embed(&mut self, samples_16k: &[f32]) -> Result<Option<Vec<f64>>, String> {
        Ok(self.embed_batch(&[samples_16k])?.pop().flatten())
    }

    /// Батчевый инференс: окна одной длины идут одним тензором [N, T, 80],
    /// паддинга нет. Невалидные окна остаются `None` и не сдвигают строки.
    pub fn embed_batch(&mut self, chunks: &[&[f32]]) -> Result<Vec<Option<Vec<f64>>>, String> {
        let mut results = vec![None; chunks.len()];
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (index, chunk) in chunks.iter().enumerate() {
            if chunk.len() >= MIN_SAMPLES {
                groups.entry(chunk.len()).or_default().push(index);
            }
        }
        for indices in groups.into_values() {
            for batch in indices.chunks(EMBED_BATCH) {
                self.run_group(chunks, batch, &mut results)?;
            }
        }
        Ok(results)
    }

    fn run_group(
        &mut self,
        chunks: &[&[f32]],
        indices: &[usize],
        results: &mut [Option<Vec<f64>>],
    ) -> Result<(), String> {
        let mut features = Vec::with_capacity(indices.len());
        let mut frames = 0usize;
        for &index in indices {
            let fbank = eres2netv2_fbank(chunks[index]);
            if fbank.is_empty() || (frames != 0 && fbank.len() != frames) {
                return Ok(());
            }
            frames = fbank.len();
            features.push(fbank);
        }
        if features.is_empty() {
            return Ok(());
        }

        let batch = features.len();
        let mut flat = Vec::with_capacity(batch * frames * 80);
        for fbank in &features {
            for values in fbank {
                flat.extend_from_slice(values);
            }
        }
        let shape = vec![batch as i64, frames as i64, 80];
        let mut inputs = HashMap::new();
        inputs.insert(self.input_name.clone(), (shape, flat));
        let out = self.session.run_f32(inputs)?;
        let dim = [EMBEDDING_DIM, LEGACY_EMBEDDING_DIM]
            .into_iter()
            .find(|&dim| out.values().any(|(_, data)| data.len() == batch * dim))
            .ok_or_else(|| "embedding-модель вернула неожиданную размерность".to_string())?;
        let data = out
            .values()
            .find(|(_, data)| data.len() == batch * dim)
            .map(|(_, data)| data.as_slice())
            .ok_or_else(|| "embedding-модель не вернула вектор".to_string())?;
        for (row, &index) in indices.iter().enumerate() {
            let vector: Vec<f64> = data[row * dim..(row + 1) * dim]
                .iter()
                .map(|&x| x as f64)
                .collect();
            let norm = vector.iter().map(|x| x * x).sum::<f64>().sqrt();
            // Нулевой вектор модель отдаёт на тишине: он не несёт голоса и,
            // попав в прототипы, одинаково «близок» ко всем дикторам.
            if norm.is_finite() && norm >= 1e-6 {
                results[index] = Some(l2_normalize(&vector));
            }
        }
        Ok(())
    }
}
