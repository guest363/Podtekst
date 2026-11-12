use std::path::{Path, PathBuf};

use mr_ml::cosine_similarity;
use mr_ml::onnx::embed::EmbeddingExtractor;
use serde::Serialize;

#[derive(Serialize)]
struct Pair {
    left: usize,
    right: usize,
    full_cosine: f64,
    within_halves: [f64; 2],
    cross_halves: [f64; 4],
    cross_median_ge_within_min: bool,
}

#[derive(Serialize)]
struct Probe {
    mode: &'static str,
    pairs: Vec<Pair>,
}

fn fingerprint(model: &mut EmbeddingExtractor, samples: &[f32]) -> Result<Vec<f64>, String> {
    model
        .embed(samples)?
        .ok_or_else(|| "недостаточно данных для эмбеддинга".to_owned())
}

fn probe(
    model: &mut EmbeddingExtractor,
    paths: &[PathBuf],
    normalized: bool,
) -> Result<Probe, String> {
    let mut clips = Vec::new();
    for path in paths {
        let mut samples = mr_audio::decode::decode_to_mono_16k(path).map_err(|e| e.to_string())?;
        if normalized {
            let original = samples.clone();
            let stats = mr_audio::dsp::agc_rms_16k(&mut samples);
            if stats.amplified_fraction == 0.0 {
                samples = original;
            }
        }
        let middle = samples.len() / 2;
        clips.push([
            fingerprint(model, &samples)?,
            fingerprint(model, &samples[..middle])?,
            fingerprint(model, &samples[middle..])?,
        ]);
    }
    let mut pairs = Vec::new();
    for (left, a) in clips.iter().enumerate() {
        for (right, b) in clips.iter().enumerate().skip(left + 1) {
            let within_halves = [
                cosine_similarity(&a[1], &a[2]),
                cosine_similarity(&b[1], &b[2]),
            ];
            let cross_halves = [
                cosine_similarity(&a[1], &b[1]),
                cosine_similarity(&a[1], &b[2]),
                cosine_similarity(&a[2], &b[1]),
                cosine_similarity(&a[2], &b[2]),
            ];
            let mut sorted = cross_halves;
            sorted.sort_by(f64::total_cmp);
            pairs.push(Pair {
                left,
                right,
                full_cosine: cosine_similarity(&a[0], &b[0]),
                within_halves,
                cross_halves,
                cross_median_ge_within_min: (sorted[1] + sorted[2]) * 0.5
                    >= within_halves[0].min(within_halves[1]),
            });
        }
    }
    Ok(Probe {
        mode: if normalized { "frontend" } else { "raw" },
        pairs,
    })
}

fn run(model_path: &Path, clips_dir: &Path, output: &Path) -> Result<(), String> {
    let mut paths: Vec<_> = std::fs::read_dir(clips_dir)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "wav"));
    paths.sort();
    if paths.len() < 2 {
        return Err("нужно минимум два WAV".to_owned());
    }
    let hashes: Vec<_> = paths
        .iter()
        .map(|path| file_hash(path))
        .collect::<Result<_, _>>()?;
    let model_hash = file_hash(model_path)?;
    let mut model = EmbeddingExtractor::load(model_path)?;
    let probes = [
        probe(&mut model, &paths, false)?,
        probe(&mut model, &paths, true)?,
    ];
    let result = serde_json::json!({
        "schema": 1,
        "clip_order": "sorted_wav_paths",
        "clips": paths.len(),
        "clip_sha256": hashes,
        "model_sha256": model_hash,
        "diagnostic_only": true,
        "probes": probes,
    });
    let bytes = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
    std::fs::write(output, bytes).map_err(|e| e.to_string())
}

fn file_hash(path: &Path) -> Result<String, String> {
    mr_models::sha256_of_file(path).map_err(|error| error.to_string())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [model, clips, output] => run(Path::new(model), Path::new(clips), Path::new(output)),
        _ => Err("аргументы: MODEL.onnx CLIPS_DIR OUTPUT.json".to_owned()),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
