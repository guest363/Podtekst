//! Whisper large-v3-turbo (ONNX, квантованный) — мультиязычный fallback.
//! Протокол проверен живым прогоном (encoder + merged-decoder с KV-кэшем):
//! * encoder: `input_features` [1, 128, 3000] → `last_hidden_state` [1, 1500, 1280]
//! * decoder step 0: prompt [sot, lang, transcribe, notimestamps], `use_cache_branch=false`,
//!   пустые past (decoder: [1,20,0,64], encoder: [1,20,Tenc,64])
//! * шаги 1..N: только последний токен, `use_cache_branch=true`,
//!   past = present-выходы предыдущего шага; стоп — `<|endoftext|>`.

use std::collections::HashMap;
use std::path::Path;

use super::accel::{ExecutionProviderKind, SessionProfile};
use super::features::whisper_mel;
use super::session::{outputs_are_finite, NamedTensors, OrtSession};

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
        let (enc, dec) = std::thread::scope(|scope| {
            let enc = scope.spawn(|| {
                // shortcut: DirectML для quantized Whisper не замерялся, перевести на GpuPreferred после замера
                OrtSession::load_with_probe(
                    &dir.join("whisper_enc_q.onnx"),
                    0,
                    SessionProfile::CudaPreferred,
                    |session| {
                        let input_name = pick(
                            &session.input_names,
                            &["input_features", "audio_features", "mel"],
                        )?;
                        let mut enc_in = HashMap::new();
                        enc_in.insert(
                            input_name,
                            (vec![1i64, 128, 3000], vec![0.0f32; 128 * 3000]),
                        );
                        let out = session.run_f32(enc_in)?;
                        if !outputs_are_finite(&out) {
                            return Err("whisper_enc probe: нечисловой выход".to_string());
                        }
                        Ok(())
                    },
                )
            });
            let dec = scope.spawn(|| {
                OrtSession::load_with_profile(
                    &dir.join("whisper_dec_merged_q.onnx"),
                    0,
                    SessionProfile::CpuOnly,
                )
            });
            (
                enc.join()
                    .unwrap_or_else(|_| Err("Whisper: загрузка энкодера упала".to_string())),
                dec.join()
                    .unwrap_or_else(|_| Err("Whisper: загрузка декодера упала".to_string())),
            )
        });
        let enc = enc?;
        let dec = dec?;
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

    pub fn provider(&self) -> ExecutionProviderKind {
        self.enc.provider()
    }

    pub fn transcribe_segment(
        &mut self,
        samples: &[f32],
        language: &str,
    ) -> Result<String, String> {
        self.transcribe_segment_with_prompt(samples, language, None)
    }

    pub fn transcribe_segment_with_prompt(
        &mut self,
        samples: &[f32],
        language: &str,
        prompt: Option<&str>,
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
        const N_FRAMES: usize = 3000;
        const N_MELS: usize = 128;
        // Ноль в нормализованной мел-шкале Whisper — это шум 10^-4, а не
        // тишина: 26 секунд такого «шума» провоцируют догадки в хвосте.
        let mut flat = vec![whisper_silence_floor(&mel); N_MELS * N_FRAMES];
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
        const MAX_PROMPT_TOKENS: usize = 224;
        let transcribe = self.bpe.special("<|transcribe|>")?;
        let mut tokens = Vec::new();
        if let Some(text) = prompt.map(str::trim).filter(|s| !s.is_empty()) {
            if let Ok(sot_prev) = self.bpe.special("<|startofprev|>") {
                // Ведущий пробел — контракт Whisper ByteLevel BPE для первого слова подсказки.
                let mut prompt_ids = self.bpe.encode(&format!(" {text}"));
                if !prompt_ids.is_empty() {
                    prompt_ids.truncate(MAX_PROMPT_TOKENS);
                    tokens.push(sot_prev);
                    tokens.extend(prompt_ids);
                }
            }
        }
        tokens.push(self.sot);
        if !language.is_empty() && language != "auto" {
            tokens.push(self.bpe.special(&format!("<|{language}|>"))?);
        }
        tokens.push(transcribe);
        tokens.push(self.no_timestamps);
        let prompt_len = tokens.len();
        let mut kv_cache: NamedTensors = HashMap::new();
        // Контракт модели: 448 позиций на prompt + генерацию (max_target_positions
        // Whisper). На 448-й прошлой позиции merged-декодер отдаёт пустой тензор
        // и падает на Reshape позиционных эмбеддингов.
        const MAX_TOKENS: usize = 448;
        let max_steps = MAX_TOKENS.saturating_sub(prompt_len);

        for step in 0..max_steps {
            let first = step == 0;
            let ids: Vec<i64> = if first {
                tokens.clone()
            } else {
                vec![*tokens.last().unwrap()]
            };
            let t_ids = ids.len() as i64;
            let (logits, present) =
                self.dec_step(enc_states, ids.as_slice(), &kv_cache, first, t_enc)?;
            let vocab = logits.len() / t_ids as usize;
            let last = &logits[(t_ids as usize - 1) * vocab..t_ids as usize * vocab];
            let next = argmax(last) as i64;
            // В cache-ветке merged-декодер отдаёт present encoder пустым:
            // состояния энкодера не меняются, и их нельзя затирать.
            for (name, value) in present {
                if !value.1.is_empty() {
                    kv_cache.insert(name, value);
                }
            }
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

    fn dec_step(
        &mut self,
        enc_states: &[f32],
        ids: &[i64],
        kv_cache: &NamedTensors,
        first: bool,
        t_enc: i64,
    ) -> Result<(Vec<f32>, NamedTensors), String> {
        // past: decoder — пустые [1,20,0,64] на первом шаге, иначе из кэша;
        // encoder — нули [1,20,Tenc,64] на первом шаге, иначе из кэша.
        // Все значения передаются срезами: копирование KV-кэша на каждом
        // шаге было основной стоимостью декодера.
        let zeros_len = (20 * t_enc * 64) as usize;
        let zeros = first.then(|| vec![0.0f32; zeros_len]);
        let mut f32_all: Vec<(&str, Vec<i64>, &[f32])> =
            Vec::with_capacity(1 + self.past_names.len());
        f32_all.push((self.state_input.as_str(), vec![1, t_enc, 1280], enc_states));
        for past in &self.past_names {
            if past.contains(".decoder.") {
                if first {
                    f32_all.push((past.as_str(), vec![1, 20, 0, 64], &[]));
                } else if let Some(v) = kv_cache.get(&present_of(past)) {
                    f32_all.push((past.as_str(), v.0.clone(), v.1.as_slice()));
                } else {
                    return Err(format!("нет кэша для {past}"));
                }
            } else if first {
                f32_all.push((
                    past.as_str(),
                    vec![1, 20, t_enc, 64],
                    zeros.as_deref().unwrap_or(&[]),
                ));
            } else if let Some(v) = kv_cache.get(&present_of(past)) {
                f32_all.push((past.as_str(), v.0.clone(), v.1.as_slice()));
            } else {
                return Err(format!("нет кэша для {past}"));
            }
        }
        let t_ids = ids.len() as i64;
        let cache_branch = [!first];
        let mut out = self.dec.run_mixed_views(
            f32_all,
            vec![(self.ids_input.as_str(), vec![1, t_ids], ids)],
            vec![(self.cache_input.as_str(), vec![1], cache_branch.as_slice())],
        )?;
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

/// Контракт `whisper_mel`: тишина — `global_max - 8 дБ`, затем `(x + 4) / 4`.
fn whisper_silence_floor(mel: &[Vec<f32>]) -> f32 {
    let max = mel
        .iter()
        .flatten()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    ((-10.0f32).max(max * 4.0 - 12.0) + 4.0) / 4.0
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

/// GPT-2 byte-level BPE (как в Whisper): кодек `token ↔ id`, слияния и спецтокены `<|...|>`.
pub struct Gpt2Bpe {
    id_to_token: Vec<String>,
    token_to_id: HashMap<String, i64>,
    merges: HashMap<(String, String), usize>,
    special: HashMap<String, i64>,
    byte_map: HashMap<char, u8>,
    byte_to_char: [char; 256],
}

fn gpt2_byte_tables() -> (HashMap<char, u8>, [char; 256]) {
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
    let mut char_to_byte = HashMap::with_capacity(256);
    let mut byte_to_char = ['\0'; 256];
    for (b, c) in bs.iter().zip(&cs) {
        if let Some(ch) = char::from_u32(*c) {
            char_to_byte.insert(ch, *b as u8);
            byte_to_char[*b as usize] = ch;
        }
    }
    (char_to_byte, byte_to_char)
}

fn match_contraction(rest: &str) -> Option<usize> {
    const CONTRS: [&str; 7] = ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"];
    CONTRS
        .iter()
        .find(|pfx| rest.starts_with(*pfx))
        .map(|pfx| pfx.len())
}

fn match_prefixed_run(rest: &str, pred: fn(char) -> bool) -> Option<usize> {
    let (space_len, body) = match rest.strip_prefix(' ') {
        Some(after) => (1, after),
        None => (0, rest),
    };
    let mut chars = body.chars();
    let first = chars.next()?;
    if !pred(first) {
        return None;
    }
    let mut consumed = first.len_utf8();
    for ch in chars {
        if pred(ch) {
            consumed += ch.len_utf8();
        } else {
            break;
        }
    }
    Some(space_len + consumed)
}

fn match_spaces(rest: &str) -> Option<usize> {
    let mut ws_len = 0usize;
    let mut ws_count = 0usize;
    let mut prev_ws_len = 0usize;
    for ch in rest.chars() {
        if ch.is_whitespace() {
            prev_ws_len = ws_len;
            ws_len += ch.len_utf8();
            ws_count += 1;
        } else {
            break;
        }
    }
    if ws_len == 0 {
        return None;
    }
    if ws_len < rest.len() && ws_count > 1 {
        Some(prev_ws_len)
    } else {
        Some(ws_len)
    }
}

fn gpt2_pretokenize(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let end = match_contraction(rest)
            .or_else(|| match_prefixed_run(rest, char::is_alphabetic))
            .or_else(|| match_prefixed_run(rest, char::is_numeric))
            .or_else(|| match_prefixed_run(rest, |c| !c.is_whitespace() && !c.is_alphanumeric()))
            .or_else(|| match_spaces(rest))
            .unwrap_or_else(|| rest.chars().next().map(char::len_utf8).unwrap_or(1));
        chunks.push(&text[i..i + end]);
        i += end;
    }
    chunks
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
        let mut token_to_id = HashMap::with_capacity(vocab.len());
        for (tok, id) in vocab {
            if let Some(i) = id.as_u64() {
                id_to_token[i as usize] = tok.clone();
                token_to_id.insert(tok.clone(), i as i64);
            }
        }
        let mut merges = HashMap::new();
        if let Some(arr) = model.get("merges").and_then(|x| x.as_array()) {
            for (rank, entry) in arr.iter().enumerate() {
                if let Some((a, b)) = entry.as_str().and_then(|s| s.split_once(' ')) {
                    merges.insert((a.to_string(), b.to_string()), rank);
                } else if let Some(pair) = entry.as_array() {
                    if let (Some(a), Some(b)) = (
                        pair.first().and_then(|x| x.as_str()),
                        pair.get(1).and_then(|x| x.as_str()),
                    ) {
                        merges.insert((a.to_string(), b.to_string()), rank);
                    }
                }
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
        let (byte_map, byte_to_char) = gpt2_byte_tables();
        Ok(Self {
            id_to_token,
            token_to_id,
            merges,
            special,
            byte_map,
            byte_to_char,
        })
    }

    pub fn special(&self, tok: &str) -> Result<i64, String> {
        self.special
            .get(tok)
            .copied()
            .ok_or_else(|| format!("нет спецтокена {tok}"))
    }

    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut out = Vec::new();
        for chunk in gpt2_pretokenize(text) {
            let mut word: Vec<String> = chunk
                .as_bytes()
                .iter()
                .map(|&b| self.byte_to_char[b as usize].to_string())
                .collect();
            if word.len() > 1 && !self.merges.is_empty() {
                loop {
                    let mut best_rank = usize::MAX;
                    let mut best_pair: Option<(String, String)> = None;
                    for pair in word.windows(2) {
                        let key = (pair[0].clone(), pair[1].clone());
                        if let Some(&rank) = self.merges.get(&key) {
                            if rank < best_rank {
                                best_rank = rank;
                                best_pair = Some(key);
                            }
                        }
                    }
                    let Some((a, b)) = best_pair else {
                        break;
                    };
                    let merged = format!("{a}{b}");
                    let mut next = Vec::with_capacity(word.len());
                    let mut idx = 0;
                    while idx < word.len() {
                        if idx + 1 < word.len() && word[idx] == a && word[idx + 1] == b {
                            next.push(merged.clone());
                            idx += 2;
                        } else {
                            next.push(word[idx].clone());
                            idx += 1;
                        }
                    }
                    word = next;
                    if word.len() < 2 {
                        break;
                    }
                }
            }
            for tok in word {
                if let Some(&id) = self.token_to_id.get(&tok) {
                    out.push(id);
                }
            }
        }
        out
    }

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
    fn padding_uses_silence_level_not_zero() {
        let tone: Vec<f32> = (0..16000 * 2)
            .map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 16000.0).sin() as f32)
            .collect();
        let mut thirty = tone.clone();
        thirty.resize(16000 * 30, 0.0);
        let full = whisper_mel(&thirty);
        let expected = full[full.len() - 1][0];
        let actual = whisper_silence_floor(&whisper_mel(&tone));
        assert!(
            (actual - expected).abs() < 1e-4,
            "тишина {actual}, хвост 30 с {expected}"
        );
    }

    #[test]
    fn bpe_decode_basic() {
        let bpe = Gpt2Bpe::parse(&mini_tokenizer()).unwrap();
        assert_eq!(bpe.special("<|startoftranscript|>").unwrap(), 50258);
        assert_eq!(bpe.decode(&[220, 31373]), "hello");
        assert_eq!(bpe.decode(&[50258, 220, 995, 50257]), "world");
        assert!(bpe.special("<|xx|>").is_err());
    }

    #[test]
    fn bpe_encode_applies_gpt2_merges_and_roundtrips() {
        let tok_json = serde_json::json!({
            "model": {
                "vocab": {
                    "Ġ": 1, "a": 2, "b": 3, "c": 4, "d": 5, ",": 6,
                    "Ġa": 7, "bc": 8, "Ġabc": 9, "Ġb": 10
                },
                "merges": ["Ġ a", "b c", "Ġa bc", "Ġ b"]
            },
            "added_tokens": [{"content": "<|startofprev|>", "id": 50362}]
        })
        .to_string();
        let bpe = Gpt2Bpe::parse(&tok_json).unwrap();
        assert_eq!(bpe.special("<|startofprev|>").unwrap(), 50362);
        let ids = bpe.encode(" abc, bd");
        assert_eq!(ids, vec![9, 6, 10, 5]);
        assert_eq!(bpe.decode(&ids), "abc, bd");
        assert_eq!(
            gpt2_pretokenize("don't  test!"),
            vec!["don", "'t", " ", " test", "!"]
        );
    }
}
