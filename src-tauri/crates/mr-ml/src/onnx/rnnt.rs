//! GigaAM-v3 e2e RNNT (ONNX int8): encoder + LSTM-decoder + joint, greedy decode.
//!
//! Формат файлов `istupakov/gigaam-v3-onnx` (NeMo/sherpa-style):
//! * encoder: `audio_signal` [1, T] f32 (сырой waveform 16 кГц!) + `length` [1] i64
//!   → `encoded` [1, Te, 768] (+ `encoded_len`)
//! * decoder: token `x` [1,1] i64 + `h.1`/`c.1` [1,1,320] → `dec` + новые `h`/`c`
//! * joint: `enc` [1,768,1] + `dec` [1,320,1] → logits [1,1,1,1025]
//! * vocab `v3_e2e_rnnt_vocab.txt`: `<piece> <id>`, `▁` = граница слова, blank = 1024.

use std::collections::HashMap;
use std::path::Path;

use super::session::{NamedTensors, OrtSession, ShapeDataI64};

pub const BLANK_ID: i64 = 1024;

pub type DecoderState = (Vec<f32>, Vec<f32>, Vec<f32>);

pub struct GigaAM {
    enc: OrtSession,
    dec: OrtSession,
    joint: OrtSession,
    enc_input: String,
    enc_output: String,
    vocab: Vec<String>,
}

impl GigaAM {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let enc = OrtSession::load(&dir.join("gigaam_enc_q.onnx"), 0)?;
        // Декодер и joint — крошечные матрицы на один кадр/токен: на пуле
        // потоков синхронизация дороже вычислений, поэтому по одному потоку.
        let dec = OrtSession::load(&dir.join("gigaam_dec_q.onnx"), 1)?;
        let joint = OrtSession::load(&dir.join("gigaam_joint_q.onnx"), 1)?;
        let enc_input = enc
            .input_names
            .first()
            .cloned()
            .ok_or_else(|| "у encoder нет входов".to_string())?;
        let enc_output = enc
            .output_names
            .first()
            .cloned()
            .ok_or_else(|| "у encoder нет выходов".to_string())?;
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
            dec,
            joint,
            enc_input,
            enc_output,
            vocab,
        })
    }

    fn joint_logits(&mut self, enc_frame: &[f32], dec_vec: &[f32]) -> Result<Vec<f32>, String> {
        let mut enc_in = HashMap::new();
        enc_in.insert("enc".to_string(), (vec![1i64, 768, 1], enc_frame.to_vec()));
        let mut dec_in = HashMap::new();
        dec_in.insert("dec".to_string(), (vec![1i64, 320, 1], dec_vec.to_vec()));
        enc_in.extend(dec_in);
        let out = self.joint.run_f32(enc_in)?;
        out.into_values()
            .next()
            .map(|(_, d)| d)
            .filter(|d| d.len() == 1025)
            .ok_or_else(|| "неожиданный выход joint".to_string())
    }

    fn decoder_step(&mut self, token: i64, h: &[f32], c: &[f32]) -> Result<DecoderState, String> {
        let f32_in: NamedTensors = HashMap::from([
            ("h.1".to_string(), (vec![1i64, 1, 320], h.to_vec())),
            ("c.1".to_string(), (vec![1i64, 1, 320], c.to_vec())),
        ]);
        let i64_in = HashMap::from([("x".to_string(), (vec![1i64, 1], vec![token]))]);
        let out = self.dec.run_mixed(f32_in, i64_in)?;
        let get = |k: &str| {
            out.get(k)
                .map(|(_, d)| d.clone())
                .filter(|d| d.len() == 320)
                .ok_or_else(|| format!("нет выхода decoder.{k}"))
        };
        Ok((get("dec")?, get("h")?, get("c")?))
    }

    /// Фичи: 64-dim HTK mel (см. `features::nemo_mel`), упаковка [1, 64, T] (NeMo channel-first).
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
        let mel = super::features::nemo_mel(samples);
        if mel.is_empty() {
            return Ok(String::new());
        }
        let tf = mel.len();
        let mut flat = Vec::with_capacity(64 * tf);
        for c in 0..64 {
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
        let enc_out = self.enc.run_mixed(f32_in, i64_in)?;
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
                    break;
                }
                ids.push(y);
                let (ng, nh, nc) = self.decoder_step(y, &h, &c)?;
                g = ng;
                h = nh;
                c = nc;
            }
        }
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
