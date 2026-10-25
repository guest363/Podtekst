//! Whisper large-v3-turbo (ONNX, квантованный) — мультиязычный fallback.
//! Протокол проверен живым прогоном (encoder + merged-decoder с KV-кэшем):
//! * encoder: `input_features` [1, 128, 3000] → `last_hidden_state` [1, 1500, 1280]
//! * decoder step 0: prompt [sot, lang, transcribe, notimestamps], `use_cache_branch=false`,
//!   пустые past (decoder: [1,20,0,64], encoder: [1,20,Tenc,64])
//! * шаги 1..N: только последний токен, `use_cache_branch=true`,
//!   past = present-выходы предыдущего шага; стоп — `<|endoftext|>`.

use std::collections::HashMap;
use std::path::Path;

use super::features::whisper_mel;
use super::session::{NamedTensors, OrtSession, ShapeDataI64};

pub struct Whisper {
    enc: OrtSession,
    dec: OrtSession,
    enc_input: String,
    enc_output: String,
    ids_input: String,
    state_input: String,
    cache_input: String,
    past_names: Vec<String>,
    present_names: Vec<String>,
    bpe: Gpt2Bpe,
    sot: i64,
    eot: i64,
    no_timestamps: i64,
}

impl Whisper {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let enc = OrtSession::load(&dir.join("whisper_enc_q.onnx"), 0)?;
        let dec = OrtSession::load(&dir.join("whisper_dec_merged_q.onnx"), 0)?;
        let enc_input = pick(
            &enc.input_names,
            &["input_features", "audio_features", "mel"],
        )?;
        let enc_output = pick(
            &enc.output_names,
            &["last_hidden_state", "encoder_last_hidden_state"],
        )?;
        let ids_input = pick(
            &dec.input_names,
            &["input_ids", "tokens", "decoder_input_ids"],
        )?;
        let state_input = dec
            .input_names
            .iter()
            .find(|n| **n != ids_input && !n.starts_with("past_") && **n != "use_cache_branch")
            .cloned()
            .unwrap_or_else(|| "encoder_hidden_states".to_string());
        let cache_input = dec
            .input_names
            .iter()
            .find(|n| *n == "use_cache_branch" || *n == "use_cache")
            .cloned()
            .unwrap_or_else(|| "use_cache_branch".to_string());
        let mut past_names: Vec<String> = dec
            .input_names
            .iter()
            .filter(|n| n.starts_with("past_key_values"))
            .cloned()
            .collect();
        past_names.sort();
        let mut present_names: Vec<String> = dec
            .output_names
            .iter()
            .filter(|n| n.starts_with("present"))
            .cloned()
            .collect();
        present_names.sort();
        if past_names.is_empty() || present_names.is_empty() {
            return Err("whisper-decoder без KV-кэша не поддерживается".to_string());
        }

        let tok_text = std::fs::read_to_string(dir.join("whisper_tokenizer.json"))
            .map_err(|e| format!("tokenizer.json: {e}"))?;
        let bpe = Gpt2Bpe::parse(&tok_text)?;
        let sot = bpe.special("<|startoftranscript|>")?;
        let eot = bpe.special("<|endoftext|>")?;
        let no_timestamps = bpe.special("<|notimestamps|>")?;
        Ok(Self {
            enc,
            dec,
            enc_input,
            enc_output,
            ids_input,
            state_input,
            cache_input,
            past_names,
            present_names,
            bpe,
            sot,
            eot,
            no_timestamps,
        })
    }

    pub fn transcribe_segment(
        &mut self,
        samples: &[f32],
        language: &str,
    ) -> Result<String, String> {
        if samples.len() < 1600 {
            return Ok(String::new());
        }
        // 30-секундное окно (длинные сегменты нарезает вызывающий код)
        let win: Vec<f32> = samples.iter().take(16000 * 30).copied().collect();
        let mel = whisper_mel(&win);
        if mel.is_empty() {
            return Ok(String::new());
        }
        // [1, 128, 3000] с нулевым паддингом, channel-first
        const N_FRAMES: usize = 3000;
        const N_MELS: usize = 128;
        let mut flat = vec![0.0f32; N_MELS * N_FRAMES];
        for (t, row) in mel.iter().take(N_FRAMES).enumerate() {
            for c in 0..N_MELS.min(row.len()) {
                flat[c * N_FRAMES + t] = row[c];
            }
        }
        let mut enc_in = HashMap::new();
        enc_in.insert(
            self.enc_input.clone(),
            (vec![1i64, N_MELS as i64, N_FRAMES as i64], flat),
        );
        let enc_out = self.enc.run_f32(enc_in)?;
        let (enc_shape, enc_states) = enc_out
            .get(&self.enc_output)
            .ok_or_else(|| "нет выхода whisper-encoder".to_string())?;
        let t_enc = if enc_shape.len() >= 2 {
            enc_shape[enc_shape.len() - 2]
        } else {
            0
        };
        if t_enc <= 0 || enc_states.is_empty() {
            return Err("пустые состояния whisper-encoder".to_string());
        }

        // «auto»/пустой язык — без токена языка: модель сама эмитит его
        // первым сгенерированным токеном (спецтокены decode() пропускает).
        let transcribe = self.bpe.special("<|transcribe|>")?;
        let mut tokens = vec![self.sot];
        if !language.is_empty() && language != "auto" {
            tokens.push(self.bpe.special(&format!("<|{language}|>"))?);
        }
        tokens.push(transcribe);
        tokens.push(self.no_timestamps);
        let prompt_len = tokens.len();
        // present-выходы прошлого шага → past-входы следующего
        let mut kv_cache: NamedTensors = HashMap::new();

        for step in 0..448 {
            let first = step == 0;
            let ids: Vec<i64> = if first {
                tokens.clone()
            } else {
                vec![*tokens.last().unwrap()]
            };
            let t_ids = ids.len() as i64;
            // Состояния энкодера нужны на каждом шаге — одна копия на шаг
            // (внутрь рантайма они уходят по значению без второго копирования).
            let mut f32_in = HashMap::new();
            f32_in.insert(
                self.state_input.clone(),
                (vec![1i64, t_enc, 1280], enc_states.clone()),
            );
            let mut i64_in = HashMap::new();
            i64_in.insert(self.ids_input.clone(), (vec![1i64, t_ids], ids));
            let (logits, present) = self.dec_step(f32_in, i64_in, &mut kv_cache, first, t_enc)?;
            let vocab = logits.len() / t_ids as usize;
            let last = &logits[(t_ids as usize - 1) * vocab..t_ids as usize * vocab];
            let next = argmax(last) as i64;
            kv_cache = present;
            if next == self.eot {
                break;
            }
            tokens.push(next);
        }
        let body = if tokens.len() > prompt_len {
            &tokens[prompt_len..]
        } else {
            &[]
        };
        Ok(self.bpe.decode(body))
    }

    /// Один шаг merged-декодера с KV-кэшем. Возвращает (логиты, present-кэш).
    fn dec_step(
        &mut self,
        mut f32_all: NamedTensors,
        i64_in: HashMap<String, ShapeDataI64>,
        kv_cache: &mut NamedTensors,
        first: bool,
        t_enc: i64,
    ) -> Result<(Vec<f32>, NamedTensors), String> {
        // past: decoder — пустые [1,20,0,64] на первом шаге, иначе из кэша;
        // encoder — нули [1,20,Tenc,64] на первом шаге, иначе из кэша.
        for past in &self.past_names {
            if past.contains(".decoder.") {
                if first {
                    f32_all.insert(past.clone(), (vec![1i64, 20, 0, 64], vec![]));
                } else if let Some(v) = kv_cache.remove(&present_of(past)) {
                    f32_all.insert(past.clone(), v);
                } else {
                    return Err(format!("нет кэша для {past}"));
                }
            } else if first {
                let n = (20 * t_enc * 64) as usize;
                f32_all.insert(past.clone(), (vec![1i64, 20, t_enc, 64], vec![0.0; n]));
            } else if let Some(v) = kv_cache.remove(&present_of(past)) {
                f32_all.insert(past.clone(), v);
            } else {
                return Err(format!("нет кэша для {past}"));
            }
        }
        let mut bool_in = HashMap::new();
        bool_in.insert(self.cache_input.clone(), (vec![1i64], vec![!first]));
        let mut out = self.dec.run_mixed_full(f32_all, i64_in, bool_in)?;
        // Логиты — выход "logits"; present-кэш — по списку из графа
        let mut logits: Option<Vec<f32>> = None;
        let mut present = HashMap::new();
        for name in &self.present_names {
            if let Some(val) = out.remove(name) {
                present.insert(name.clone(), val);
            }
        }
        if let Some((_, data)) = out.remove("logits") {
            logits = Some(data);
        }
        Ok((logits.ok_or_else(|| "нет логитов".to_string())?, present))
    }
}

fn present_of(past_name: &str) -> String {
    past_name.replacen("past_key_values", "present", 1)
}

fn pick(names: &[String], cands: &[&str]) -> Result<String, String> {
    cands
        .iter()
        .find(|c| names.iter().any(|n| n == *c))
        .map(|s| s.to_string())
        .or_else(|| names.first().cloned())
        .ok_or_else(|| "у модели нет входов".to_string())
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

/// GPT-2 byte-level BPE (как в Whisper): вокод `token → id`, спецтокены `<|...|>`.
pub struct Gpt2Bpe {
    id_to_token: Vec<String>,
    special: HashMap<String, i64>,
    /// Обратное byte-mapping GPT-2: unicode char → исходный байт.
    byte_map: HashMap<char, u8>,
}

fn gpt2_byte_map() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (0x21..=0x7E).collect();
    bs.extend(0xA1..=0xAC);
    bs.extend(0xAE..=0xFF);
    let mut cs = bs.clone();
    let mut n = 0u32;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(0x100 + n);
            n += 1;
        }
    }
    bs.iter()
        .zip(&cs)
        .filter_map(|(b, c)| char::from_u32(*c).map(|ch| (ch, *b as u8)))
        .collect()
}

impl Gpt2Bpe {
    pub fn parse(tokenizer_json: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(tokenizer_json).map_err(|e| format!("tokenizer.json: {e}"))?;
        let model = v.get("model").ok_or("нет model в tokenizer.json")?;
        let vocab = model
            .get("vocab")
            .and_then(|x| x.as_object())
            .ok_or("нет vocab")?;
        let mut max_id = 0usize;
        for id in vocab.values().filter_map(|x| x.as_u64()) {
            max_id = max_id.max(id as usize);
        }
        let mut id_to_token = vec![String::new(); max_id + 1];
        for (tok, id) in vocab {
            if let Some(i) = id.as_u64() {
                id_to_token[i as usize] = tok.clone();
            }
        }
        let mut special = HashMap::new();
        if let Some(added) = v.get("added_tokens").and_then(|x| x.as_array()) {
            for t in added {
                let (Some(content), Some(id)) = (
                    t.get("content").and_then(|x| x.as_str()),
                    t.get("id").and_then(|x| x.as_i64()),
                ) else {
                    continue;
                };
                special.insert(content.to_string(), id);
                let i = id as usize;
                if i >= id_to_token.len() {
                    id_to_token.resize(i + 1, String::new());
                }
                id_to_token[i] = content.to_string();
            }
        }
        Ok(Self {
            id_to_token,
            special,
            byte_map: gpt2_byte_map(),
        })
    }

    pub fn special(&self, tok: &str) -> Result<i64, String> {
        self.special
            .get(tok)
            .copied()
            .ok_or_else(|| format!("нет спецтокена {tok}"))
    }

    /// Декодирует ids в текст (Ġ → пробел, `<0x..>` → байты, спецтокены пропускаются).
    pub fn decode(&self, ids: &[i64]) -> String {
        let mut bytes: Vec<u8> = Vec::new();
        for &id in ids {
            let tok = self
                .id_to_token
                .get(id as usize)
                .map(|s| s.as_str())
                .unwrap_or("");
            if tok.is_empty() || (tok.starts_with("<|") && tok.ends_with("|>")) {
                continue;
            }
            if let Some(hex) = tok.strip_prefix("<0x").and_then(|s| s.strip_suffix('>')) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    bytes.push(b);
                    continue;
                }
            }
            for ch in tok.chars() {
                // GPT-2 byte-level mapping: 'Ġ' (U+0120) = пробел 0x20, остальные —
                // через обратную таблицу (U+0100.. = управляющие/непечатные байты).
                let cp = ch as u32;
                if cp == 0x0120 {
                    bytes.push(b' ');
                } else if let Some(&b) = self.byte_map.get(&ch) {
                    bytes.push(b);
                } else {
                    bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                }
            }
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mini_tokenizer() -> String {
        serde_json::json!({
            "model": {"vocab": {"hello": 31373, "Ġ": 220, "world": 995}},
            "added_tokens": [
                {"content": "<|startoftranscript|>", "id": 50258},
                {"content": "<|endoftext|>", "id": 50257},
                {"content": "<|transcribe|>", "id": 50359},
                {"content": "<|notimestamps|>", "id": 50363},
                {"content": "<|ru|>", "id": 50263}
            ]
        })
        .to_string()
    }

    #[test]
    fn bpe_decode_basic() {
        let bpe = Gpt2Bpe::parse(&mini_tokenizer()).unwrap();
        assert_eq!(bpe.special("<|startoftranscript|>").unwrap(), 50258);
        assert_eq!(bpe.decode(&[220, 31373]), "hello");
        assert_eq!(bpe.decode(&[50258, 220, 995, 50257]), "world");
        assert!(bpe.special("<|xx|>").is_err());
    }
}
