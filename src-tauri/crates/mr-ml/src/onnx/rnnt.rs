//! Контракт ONNX-файлов GigaAM-v4 (`guest363/gigaam-v4-e2e-rnnt-int8-onnx`)
//! и обратная совместимость с GigaAM-v3 (`istupakov/gigaam-v3-onnx`).
//!
//! Формат v4 (single-head `e2e`, pre-fused):
//! * `rnnt_encoder.onnx`: `audio_signal` [1, 64, T] f32 (64-полосный mel, см.
//!   `features::nemo_mel_v4`) + `length` [1] i64 → `encoded_len` [1] i64,
//!   `head_outputs` [1, 1, Te, 320] f32.
//! * `rnnt_advance.onnx`: `projected` [1, 1, Te, 320] f32 + `lengths` [1] i64
//!   → `tokens` [steps, 1, 1] i64.
//!
//! Формат v3 (NeMo/sherpa-style):
//! * encoder + decoder + joint (`gigaam_{enc,dec,joint}_q.onnx`).
//! * vocab `gigaam_vocab.txt`: `<piece> <id>`, `▁` = граница слова, blank = 1024.

use std::collections::HashMap;
use std::path::Path;

use super::accel::{ExecutionProviderKind, SessionProfile};
use super::features;
use super::session::{outputs_are_finite, NamedTensors, OrtSession, ShapeDataI64};

pub const BLANK_ID: i64 = 1024;

pub type DecoderState = (Vec<f32>, Vec<f32>, Vec<f32>);

/// Метрики последнего декодирования: blank-доля объясняет пустой текст
/// на тихой речи без отдельного прогона модели.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub frames: usize,
    pub tokens: usize,
    pub blank: usize,
}

enum RnntBackend {
    V4 { adv: OrtSession },
    V3 { dec: OrtSession, joint: OrtSession },
}

pub struct GigaAM {
    enc: OrtSession,
    backend: RnntBackend,
    enc_input: String,
    enc_output: String,
    vocab: Vec<String>,
    last_stats: DecodeStats,
}

impl GigaAM {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let v4_enc = dir.join("rnnt_encoder.onnx");
        let v4_adv = dir.join("rnnt_advance.onnx");
        let is_v4 = v4_enc.is_file() && v4_adv.is_file();
        let enc_path = if is_v4 {
            v4_enc
        } else {
            dir.join("gigaam_enc_q.onnx")
        };
        // ЗАЧЕМ: INT8-граф с динамической длиной на GPU (DirectML и CUDA) медленнее CPU
        // из-за переключения ядра по длине сегмента и даёт численный дрейф на коротких фразах.
        let enc = OrtSession::load_with_probe(&enc_path, 0, SessionProfile::CpuOnly, |session| {
            let input_name = session
                .input_names
                .first()
                .cloned()
                .ok_or_else(|| "у encoder нет входов".to_string())?;
            let mut f32_in = HashMap::new();
            f32_in.insert(
                input_name,
                (
                    vec![1i64, features::GIGAAM_MEL_BANDS as i64, 100],
                    vec![0.0f32; features::GIGAAM_MEL_BANDS * 100],
                ),
            );
            let mut i64_in: HashMap<String, ShapeDataI64> = HashMap::new();
            if session.has_input("length") {
                i64_in.insert("length".to_string(), (vec![1i64], vec![100i64]));
            }
            let out = session.run_mixed(f32_in, i64_in)?;
            if !outputs_are_finite(&out) {
                return Err("gigaam_enc probe: нечисловой выход".to_string());
            }
            Ok(())
        })?;
        let backend = if is_v4 {
            let adv = OrtSession::load_with_profile(&v4_adv, 0, SessionProfile::CpuOnly)?;
            RnntBackend::V4 { adv }
        } else {
            // Декодер и joint — крошечные матрицы на один кадр/токен: на пуле
            // потоков синхронизация дороже вычислений, поэтому по одному потоку.
            let dec = OrtSession::load_with_profile(
                &dir.join("gigaam_dec_q.onnx"),
                1,
                SessionProfile::CpuOnly,
            )?;
            let joint = OrtSession::load_with_profile(
                &dir.join("gigaam_joint_q.onnx"),
                1,
                SessionProfile::CpuOnly,
            )?;
            RnntBackend::V3 { dec, joint }
        };
        let enc_input = enc
            .input_names
            .first()
            .cloned()
            .ok_or_else(|| "у encoder нет входов".to_string())?;
        let enc_output = if is_v4 {
            "head_outputs".to_string()
        } else {
            enc.output_names
                .first()
                .cloned()
                .ok_or_else(|| "у encoder нет выходов".to_string())?
        };
        let vocab_text =
            std::fs::read_to_string(dir.join("gigaam_vocab.txt")).map_err(|e| e.to_string())?;
        let mut vocab = vec![String::new(); 1024];
        for line in vocab_text.lines() {
            let mut it = line.split_whitespace();
            if let (Some(piece), Some(id_s)) = (it.next(), it.next()) {
                if let Ok(id) = id_s.parse::<usize>() {
                    if id < 1024 {
                        vocab[id] = piece.to_string();
                    }
                }
            }
        }
        Ok(Self {
            enc,
            backend,
            enc_input,
            enc_output,
            vocab,
            last_stats: DecodeStats::default(),
        })
    }

    pub fn is_v4(&self) -> bool {
        matches!(self.backend, RnntBackend::V4 { .. })
    }

    /// Мел-спектрограмма под загруженную версию модели (`v4` без preemphasis, `v3` с 0.97).
    pub fn compute_mel(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        if self.is_v4() {
            super::features::nemo_mel_v4(samples)
        } else {
            super::features::nemo_mel(samples)
        }
    }

    pub fn last_stats(&self) -> DecodeStats {
        self.last_stats
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.enc.provider()
    }

    fn joint_logits(&mut self, enc_frame: &[f32], dec_vec: &[f32]) -> Result<Vec<f32>, String> {
        let RnntBackend::V3 { joint, .. } = &mut self.backend else {
            return Err("joint недоступен для GigaAM-v4".to_string());
        };
        let mut enc_in = HashMap::new();
        enc_in.insert("enc".to_string(), (vec![1i64, 768, 1], enc_frame.to_vec()));
        let mut dec_in = HashMap::new();
        dec_in.insert("dec".to_string(), (vec![1i64, 320, 1], dec_vec.to_vec()));
        enc_in.extend(dec_in);
        let out = joint.run_f32(enc_in)?;
        out.into_values()
            .next()
            .map(|(_, d)| d)
            .filter(|d| d.len() == 1025)
            .ok_or_else(|| "неожиданный выход joint".to_string())
    }

    fn decoder_step(&mut self, token: i64, h: &[f32], c: &[f32]) -> Result<DecoderState, String> {
        let RnntBackend::V3 { dec, .. } = &mut self.backend else {
            return Err("decoder недоступен для GigaAM-v4".to_string());
        };
        let f32_in: NamedTensors = HashMap::from([
            ("h.1".to_string(), (vec![1i64, 1, 320], h.to_vec())),
            ("c.1".to_string(), (vec![1i64, 1, 320], c.to_vec())),
        ]);
        let i64_in = HashMap::from([("x".to_string(), (vec![1i64, 1], vec![token]))]);
        let out = dec.run_mixed(f32_in, i64_in)?;
        let get = |k: &str| {
            out.get(k)
                .map(|(_, d)| d.clone())
                .filter(|d| d.len() == 320)
                .ok_or_else(|| format!("нет выхода decoder.{k}"))
        };
        Ok((get("dec")?, get("h")?, get("c")?))
    }

    /// Фичи: 64-dim HTK mel (`nemo_mel_v4` для v4, `nemo_mel` для v3), упаковка [1, 64, T].
    pub fn transcribe_segment(&mut self, samples: &[f32]) -> Result<String, String> {
        self.transcribe_segment_penalized(samples, 0.0)
    }

    /// То же, но с штрафом на blank: положительное значение поощряет токены
    /// (нужно для тихой короткой речи, где жадный декодер выбирает blank).
    pub fn transcribe_segment_penalized(
        &mut self,
        samples: &[f32],
        blank_penalty: f32,
    ) -> Result<String, String> {
        if samples.len() < 1600 {
            return Ok(String::new());
        }
        let mel = match self.backend {
            RnntBackend::V4 { .. } => super::features::nemo_mel_v4(samples),
            RnntBackend::V3 { .. } => super::features::nemo_mel(samples),
        };
        self.transcribe_mel_penalized(&mel, blank_penalty)
    }

    pub fn transcribe_mel_penalized(
        &mut self,
        mel: &[Vec<f32>],
        blank_penalty: f32,
    ) -> Result<String, String> {
        self.last_stats = DecodeStats::default();
        if mel.is_empty() {
            return Ok(String::new());
        }
        features::ensure_mel_width(mel, features::GIGAAM_MEL_BANDS)?;
        let tf = mel.len();
        let mut flat = Vec::with_capacity(features::GIGAAM_MEL_BANDS * tf);
        for c in 0..features::GIGAAM_MEL_BANDS {
            for row in mel.iter() {
                flat.push(row[c]);
            }
        }
        let mut f32_in = HashMap::new();
        f32_in.insert(self.enc_input.clone(), (vec![1i64, 64, tf as i64], flat));
        let mut i64_in: HashMap<String, ShapeDataI64> = HashMap::new();
        if self.enc.has_input("length") {
            i64_in.insert("length".to_string(), (vec![1i64], vec![tf as i64]));
        }
        let mut enc_out = self.enc.run_mixed(f32_in, i64_in)?;
        if let RnntBackend::V4 { adv } = &mut self.backend {
            let (head_shape, head_data) = enc_out
                .remove("head_outputs")
                .ok_or_else(|| "нет выхода encoder.head_outputs".to_string())?;
            let (_, len_data) = enc_out
                .remove("encoded_len")
                .ok_or_else(|| "нет выхода encoder.encoded_len".to_string())?;
            let enc_len = len_data.first().copied().unwrap_or(0.0).round() as i64;
            let f32_adv = HashMap::from([("projected".to_string(), (head_shape, head_data))]);
            let i64_adv = HashMap::from([("lengths".to_string(), (vec![1i64], vec![enc_len]))]);
            let adv_out = adv.run_mixed(f32_adv, i64_adv)?;
            let (_, tok_data) = adv_out
                .get("tokens")
                .ok_or_else(|| "нет выхода advance.tokens".to_string())?;
            let ids: Vec<i64> = tok_data
                .iter()
                .map(|&v| v.round() as i64)
                .filter(|&id| id >= 0 && id != BLANK_ID)
                .collect();
            let frames = enc_len.max(0) as usize;
            self.last_stats = DecodeStats {
                frames,
                tokens: ids.len(),
                blank: frames.saturating_sub(ids.len()),
            };
            return Ok(decode_pieces(&ids, &self.vocab));
        }
        let (enc_shape, enc_data) = enc_out
            .get(&self.enc_output)
            .ok_or_else(|| "нет выхода encoder".to_string())?;
        // NeMo channel-first [1, 768, Te] либо [1, Te, 768]: приводим к кадрам [Te, 768]
        if enc_data.is_empty() || enc_shape.len() != 3 {
            return Err(format!("странный выход encoder: {enc_shape:?}"));
        }
        let frames_data: Vec<f32> = if enc_shape[1] == 768 {
            let te = enc_shape[2] as usize;
            let mut f = vec![0.0f32; te * 768];
            for t in 0..te {
                for c in 0..768 {
                    f[t * 768 + c] = enc_data[c * te + t];
                }
            }
            f
        } else {
            enc_data.clone()
        };
        if !frames_data.len().is_multiple_of(768) || frames_data.is_empty() {
            return Err(format!("странный выход encoder: {enc_shape:?}"));
        }
        let frames = frames_data.len() / 768;

        let mut h = vec![0.0f32; 320];
        let mut c = vec![0.0f32; 320];
        let (mut g, nh, nc) = self.decoder_step(BLANK_ID, &h, &c)?;
        h = nh;
        c = nc;
        let mut ids: Vec<i64> = Vec::new();
        let mut blanks = 0usize;
        for t in 0..frames {
            let enc_frame = &frames_data[t * 768..(t + 1) * 768];
            // Цикл joiner: пока не blank — эмитим токены (защита от зацикливания)
            for _ in 0..16 {
                let mut logits = self.joint_logits(enc_frame, &g)?;
                if blank_penalty != 0.0 {
                    logits[BLANK_ID as usize] -= blank_penalty;
                }
                let y = argmax(&logits) as i64;
                if y == BLANK_ID {
                    // Blank считаем по факту: `frames - tokens` врёт, когда
                    // один кадр эмитит несколько токенов.
                    blanks += 1;
                    break;
                }
                ids.push(y);
                let (ng, nh, nc) = self.decoder_step(y, &h, &c)?;
                g = ng;
                h = nh;
                c = nc;
            }
        }
        self.last_stats = DecodeStats {
            frames,
            tokens: ids.len(),
            blank: blanks,
        };
        Ok(decode_pieces(&ids, &self.vocab))
    }
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate().skip(1) {
        if x > v[best] {
            best = i;
        }
    }
    best
}

fn decode_pieces(ids: &[i64], vocab: &[String]) -> String {
    let mut s = String::new();
    for &id in ids {
        if id < 0 || (id as usize) >= vocab.len() {
            continue;
        }
        s.push_str(&vocab[id as usize]);
    }
    s.replace('▁', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_decode() {
        let vocab = vec![
            "<unk>".to_string(),
            ".".to_string(),
            "▁".to_string(),
            "привет".to_string(),
            "▁мир".to_string(),
        ];
        assert_eq!(decode_pieces(&[3, 4], &vocab), "привет мир");
        assert_eq!(decode_pieces(&[1024], &vocab), "");
    }
}
