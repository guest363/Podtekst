//! Без файлов тесты вежливо пропускаются (CI без сети).

#![cfg(feature = "onnx")]

use std::path::PathBuf;

fn models_dir() -> Option<PathBuf> {
    let d = std::env::var("MR_MODELS_DIR").unwrap_or_else(|_| "/tmp/opencode/models".to_string());
    let p = PathBuf::from(d);
    p.is_dir().then_some(p)
}

fn model_file(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    (p.is_file() && p.metadata().map(|m| m.len() > 1000).unwrap_or(false)).then_some(p)
}

/// Файл VAD-модели: плоский тестовый каталог или app-раскладка
/// `models/<id>/<file>` (vad-silero-v6).
fn vad_model_file(dir: &std::path::Path) -> Option<PathBuf> {
    ["silero_vad.onnx", "vad-silero-v6/silero_vad.onnx"]
        .iter()
        .find_map(|f| model_file(dir, f))
}

/// Файл embedding-модели: плоский тестовый каталог или app-раскладка
/// `models/<id>/<file>` (как в `<app_data>/models/diar-eres2netv2-emb/`).
fn embedding_model_file(dir: &std::path::Path) -> Option<PathBuf> {
    [
        "eres2netv2_emb.onnx",
        "diar-eres2netv2-emb/eres2netv2_emb.onnx",
        "pyannote_emb.onnx",
        "diar-pyannote-emb/pyannote_emb.onnx",
    ]
    .iter()
    .find_map(|f| model_file(dir, f))
}

fn segmentation_model_file(dir: &std::path::Path) -> Option<PathBuf> {
    [
        "pyannote-segmentation-3.0.onnx",
        "diar-pyannote-segmentation-3/pyannote-segmentation-3.0.onnx",
    ]
    .iter()
    .find_map(|f| model_file(dir, f))
}

#[test]
fn pyannote_segmentation_matches_powerset_contract() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let Some(path) = segmentation_model_file(&dir) else {
        eprintln!("SKIP: нет PyAnnote segmentation");
        return;
    };
    let mut model = mr_ml::onnx::segmentation::SegmentationModel::load(&path).expect("load");
    let result = model.segment(&vec![0.0; 16000 * 10]).expect("segment");
    assert_eq!(result.windows.len(), 1);
    assert_eq!(result.windows[0].classes.len(), 589);
    assert_eq!(result.frames.len(), 589);
    assert!(result.frames.iter().all(|&class| class < 7));

    let sliding = model
        .segment(&vec![0.0; 16000 * 10 + 8000])
        .expect("sliding");
    assert_eq!(sliding.windows.len(), 2);
    assert_eq!(sliding.windows[0].classes.len(), 589);
    assert_eq!(sliding.windows[1].start_sample, 16000);
    assert_eq!(sliding.windows[1].classes.len(), 560);
    assert!(sliding.frames.len() > 589);
}

fn read_wav_mono16(path: &std::path::Path) -> Option<(Vec<f32>, u32)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let (mut sr, mut ch, mut bits) = (0u32, 0u16, 0u16);
    let mut data: &[u8] = &[];
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            ch = u16::from_le_bytes([body[2], body[3]]);
            sr = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = body;
        }
        pos += 8 + size + (size % 2);
    }
    if data.is_empty() || bits != 16 || sr == 0 {
        return None;
    }
    let mut mono = Vec::with_capacity(data.len() / 2 / ch.max(1) as usize);
    let step = ch.max(1) as usize;
    for (i, s) in data.chunks(2).enumerate() {
        if i % step == 0 {
            mono.push(i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0);
        }
    }
    // Ресемплинг не делаем: тесты используют только 16 кГц
    if sr != 16000 {
        return None;
    }
    Some((mono, sr))
}

fn test_wav() -> Option<Vec<f32>> {
    let p =
        std::env::var("MR_TEST_WAV").unwrap_or_else(|_| "/tmp/opencode/mic_test.wav".to_string());
    read_wav_mono16(std::path::Path::new(&p)).map(|(s, _)| s)
}

#[test]
fn encoder_io_contract() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let Some(enc_path) = model_file(&dir, "gigaam_enc_q.onnx") else {
        eprintln!("SKIP: нет энкодера");
        return;
    };
    let mut enc = mr_ml::onnx::OrtSession::load(&enc_path, 1).expect("encoder load");
    println!(
        "enc inputs: {:?} outputs: {:?}",
        enc.input_names, enc.output_names
    );
    assert_eq!(enc.input_names.len(), 2);
    let t = 300usize;
    let mut f32_in = std::collections::HashMap::new();
    f32_in.insert(
        enc.input_names[0].clone(),
        (vec![1i64, 64, t as i64], vec![0.0f32; 64 * t]),
    );
    let mut i64_in = std::collections::HashMap::new();
    if let Some(ln) = enc.input_names.iter().find(|n| *n == "length").cloned() {
        i64_in.insert(ln, (vec![1i64], vec![t as i64]));
    }
    let out = enc.run_mixed(f32_in, i64_in).expect("encoder run");
    let (shape, data) = out.get("encoded").expect("выход encoded").clone();
    println!("enc out shape: {shape:?}");
    assert_eq!(shape.len(), 3);
    assert!(shape.contains(&768), "нет dim 768 в {shape:?}");
    assert!(data.iter().all(|x| x.is_finite()));
}

#[test]
fn vad_finds_speech_in_real_recording() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let (Some(vad_path), Some(wav)) = (vad_model_file(&dir), test_wav()) else {
        eprintln!("SKIP: нет vad или wav");
        return;
    };
    let mut vad = mr_ml::onnx::vad::SileroVad::load(&vad_path).expect("vad load");
    let probs = vad.process_all(&wav).expect("vad run");
    assert!(!probs.is_empty());
    // Энергия по окнам 512: верхняя дециль vs нижняя — речь должна оцениваться выше тишины
    let mut energy: Vec<(f32, f32)> = wav
        .chunks(512)
        .zip(probs.iter())
        .map(|(c, &p)| {
            let e = (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32).sqrt();
            (e, p)
        })
        .collect();
    energy.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let n = energy.len();
    let lo: f32 = energy[..n / 10].iter().map(|x| x.1).sum::<f32>() / (n / 10) as f32;
    let hi: f32 = energy[9 * n / 10..].iter().map(|x| x.1).sum::<f32>() / (n - 9 * n / 10) as f32;
    let max_p = probs.iter().cloned().fold(0.0f32, f32::max);
    println!("vad: тихие окна p={lo:.3}, громкие p={hi:.3}, max={max_p:.3}");
    assert!(hi > lo, "VAD не различает речь и тишину");
    let segs = mr_ml::onnx::vad::vad_segments_from_probs(&probs, 0.15, 0.25, 0.3);
    println!("vad: сегментов={} первый={:?}", segs.len(), segs.first());
    assert!(!segs.is_empty(), "VAD не нашёл речь в реальной записи");
}

#[test]
fn gigaam_transcribes_real_russian_speech() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let need = [
        "gigaam_enc_q.onnx",
        "gigaam_dec_q.onnx",
        "gigaam_joint_q.onnx",
        "gigaam_vocab.txt",
    ];
    if need.iter().any(|f| model_file(&dir, f).is_none()) {
        eprintln!("SKIP: нет файлов GigaAM");
        return;
    }
    let Some(wav) = test_wav() else {
        eprintln!("SKIP: нет wav");
        return;
    };
    let Some(vad_p) = vad_model_file(&dir) else {
        eprintln!("SKIP: нет vad");
        return;
    };
    let mut vad = mr_ml::onnx::vad::SileroVad::load(&vad_p).unwrap();
    let probs = vad.process_all(&wav).unwrap();
    let segs = mr_ml::onnx::vad::vad_segments_from_probs(&probs, 0.15, 0.25, 0.3);
    assert!(!segs.is_empty());
    let (s, e) = segs
        .iter()
        .map(|(a, b)| (*a, *b))
        .max_by(|a, b| (a.1 - a.0).partial_cmp(&(b.1 - b.0)).unwrap())
        .unwrap();
    let e = e.min(s + 12.0);
    let chunk = &wav[(s * 16000.0) as usize..((e * 16000.0) as usize).min(wav.len())];
    println!("gigaam: сегмент {s:.1}-{e:.1} с ({} сэмплов)", chunk.len());
    let mut giga = mr_ml::onnx::rnnt::GigaAM::load(&dir).expect("giga load");
    let text = giga.transcribe_segment(chunk).expect("transcribe");
    println!("gigaam: «{text}»");
    assert!(
        !text.trim().is_empty(),
        "GigaAM вернул пустой текст на реальной речи"
    );
    assert!(
        text.chars()
            .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c)),
        "в тексте нет кириллицы: {text}"
    );
}

#[test]
fn whisper_transcribes_russian_fallback() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    for f in [
        "whisper_enc_q.onnx",
        "whisper_dec_merged_q.onnx",
        "whisper_tokenizer.json",
    ] {
        if model_file(&dir, f).is_none() {
            eprintln!("SKIP: нет {f}");
            return;
        }
    }
    let Some(wav) = test_wav() else {
        eprintln!("SKIP: нет wav");
        return;
    };
    let mut ws = mr_ml::onnx::whisper::Whisper::load(&dir).expect("whisper load");
    let chunk = &wav[..(16000 * 8).min(wav.len())];
    let text = ws.transcribe_segment(chunk, "ru").expect("whisper");
    println!("whisper: «{text}»");
    assert!(!text.trim().is_empty(), "Whisper вернул пустой текст");
}

#[test]
fn embedding_is_deterministic_and_sane() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let Some(emb_path) = embedding_model_file(&dir) else {
        eprintln!("SKIP: нет эмбеддинга");
        return;
    };
    let Some(wav) = test_wav() else {
        eprintln!("SKIP: нет wav");
        return;
    };
    let is_eres2netv2 = emb_path
        .file_name()
        .map(|n| n != "pyannote_emb.onnx")
        .unwrap_or(true);
    let mut emb = mr_ml::onnx::embed::EmbeddingExtractor::load(&emb_path).expect("emb load");
    let chunk = &wav[..(16000 * 5).min(wav.len())];
    let e1 = emb.embed(chunk).expect("e1").expect("вектор");
    let e2 = emb.embed(chunk).expect("e2").expect("вектор");
    if is_eres2netv2 {
        assert_eq!(
            e1.len(),
            mr_ml::EMBEDDING_DIM,
            "ожидалось пространство {}",
            mr_ml::EMBEDDING_SPACE
        );
    } else {
        eprintln!("legacy pyannote-модель: dim={}", e1.len());
    }
    let cos = mr_ml::cosine_similarity(&e1, &e2);
    println!("emb: dim={} self-cos={cos:.4}", e1.len());
    assert!(cos > 0.999, "эмбеддинг недетерминирован: {cos}");
    let start = (16000 * 30).min(wav.len().saturating_sub(16000 * 5));
    let other = &wav[start..(start + 16000 * 5).min(wav.len())];
    if other.len() >= 6400 {
        let e3 = emb.embed(other).expect("e3").expect("вектор");
        let cross = mr_ml::cosine_similarity(&e1, &e3);
        println!("emb: cross-chunk cos={cross:.4}");
        assert!(cross.is_finite());
    }
    assert!(emb.embed(&wav[..1000]).expect("short").is_none());
}

/// Приёмочный тест ERes2NetV2 против официальных примеров 3D-Speaker
/// (`speech_eres2netv2_sv_zh-cn_16k-common`, Apache-2.0): эталонные эмбеддинги
/// лежат в `tests/fixtures/eres2netv2_reference.json`. Модель не нужна — только файл
/// `eres2netv2_emb.onnx` из `MR_MODELS_DIR`.
#[test]
fn eres2netv2_parity_with_official_reference() {
    let Some(dir) = models_dir() else {
        eprintln!("SKIP: нет моделей");
        return;
    };
    let Some(emb_path) = [
        "eres2netv2_emb.onnx",
        "diar-eres2netv2-emb/eres2netv2_emb.onnx",
    ]
    .iter()
    .find_map(|f| model_file(&dir, f)) else {
        eprintln!("SKIP: нет eres2netv2_emb.onnx");
        return;
    };
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let ref_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.join("eres2netv2_reference.json")).expect("reference.json"),
    )
    .expect("json");
    let load = |name: &str| {
        read_wav_mono16(&fx.join(name))
            .unwrap_or_else(|| panic!("фикстура {name}"))
            .0
    };
    let s1a = load("speaker1_a_cn_16k.wav");
    let s1b = load("speaker1_b_cn_16k.wav");
    let s2a = load("speaker2_a_cn_16k.wav");

    let mut ext = mr_ml::onnx::embed::EmbeddingExtractor::load(&emb_path).expect("emb load");
    let ref_emb = |key: &str| -> Vec<f64> {
        ref_json[key]["embedding"]
            .as_array()
            .expect("embedding")
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect()
    };
    let r1a = ref_emb("speaker1_a");

    // 1) Паритет Rust-конвейера с эталоном: косинус ≥ 0.98
    let e1a = ext.embed(&s1a).expect("embed").expect("вектор");
    assert_eq!(
        e1a.len(),
        ref_json["embedding_dim"].as_u64().unwrap() as usize
    );
    let parity = mr_ml::cosine_similarity(&e1a, &r1a);
    println!("eres2netv2: сходство с официальным эталоном = {parity:.4}");
    assert!(parity >= 0.98, "сходство с эталоном ниже 0.98: {parity}");

    let e1b = ext.embed(&s1b).expect("embed").expect("вектор");
    let e2a = ext.embed(&s2a).expect("embed").expect("вектор");
    let same = mr_ml::cosine_similarity(&e1a, &e1b);
    let cross = mr_ml::cosine_similarity(&e1a, &e2a);
    println!("eres2netv2: same-speaker={same:.3} cross-speaker={cross:.3}");
    assert!(same >= 0.45, "один диктор, два дубля: {same:.3}");
    assert!(cross <= 0.30, "разные дикторы слишком похожи: {cross:.3}");

    // 3) Стабильность коротких фрагментов 0.6/1.0/1.5/3.0 с: каждый кусок ближе
    //    к полному клипу своего диктора, чем к чужому
    for dur_s in [0.6f64, 1.0, 1.5, 3.0] {
        let n = (16000.0 * dur_s) as usize;
        for (i, w) in s1a.chunks(n).enumerate() {
            if w.len() < n {
                continue;
            }
            let e = ext.embed(w).expect("embed").expect("вектор");
            let own = mr_ml::cosine_similarity(&e, &e1a);
            let alien = mr_ml::cosine_similarity(&e, &e2a);
            println!("eres2netv2: {dur_s}s кусок #{i}: own={own:.3} alien={alien:.3}");
            assert!(own > alien, "{dur_s}s кусок #{i} ближе к чужому голосу");
            assert!(
                own >= 0.25,
                "{dur_s}s кусок #{i} далеко от своего: {own:.3}"
            );
        }
    }
}
