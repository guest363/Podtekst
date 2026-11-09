use serde_json::Value;

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("вшитый JSON валиден")
}

#[test]
fn base_tauri_config_has_no_summary_runtime() {
    let conf = json(include_str!("../tauri.conf.json"));
    let bundle = &conf["bundle"];
    assert!(bundle.get("externalBin").is_none(), "externalBin запрещён");
    let resources = bundle["resources"].as_object().expect("resources — карта");
    assert!(
        !resources.iter().any(|(key, value)| key.contains("runtime")
            || value.as_str().unwrap_or("").contains("runtime")),
        "базовый конфиг должен собираться без runtime"
    );
}

#[test]
fn release_config_adds_runtime_resources() {
    let conf = json(include_str!("../tauri.summary.conf.json"));
    let resources = conf["bundle"]["resources"]
        .as_object()
        .expect("табличка ресурсов");
    assert_eq!(
        resources
            .get("../../binaries/runtime")
            .and_then(Value::as_str),
        Some("runtime")
    );
}

#[test]
fn runtime_manifest_is_complete_per_target() {
    let manifest = json(include_str!("../summary-runtime-manifest.json"));
    assert_eq!(manifest["layout"], "resources_dir");
    assert_eq!(manifest["runtime"], "llama.cpp");
    assert_eq!(manifest["build"], "12182");
    assert_eq!(manifest["commit"], "fd27b2034");
    assert_eq!(manifest["license"], "MIT");
    assert_eq!(manifest["launcher"], "llama-server");
    assert!(manifest["nightly"].as_bool().unwrap_or(false));

    let targets = manifest["targets"].as_object().expect("targets");
    assert_eq!(targets.len(), 3);
    for (key, triple) in [
        ("linux-x86_64", "x86_64-unknown-linux-gnu"),
        ("macos-arm64", "aarch64-apple-darwin"),
        ("windows-x86_64", "x86_64-pc-windows-msvc"),
    ] {
        let target = targets.get(key).unwrap_or_else(|| panic!("нет {key}"));
        assert_eq!(target["triple"], triple);
        assert!(target["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/b12182/"));
        let sha = target["sha256"].as_str().expect("sha256");
        assert_eq!(sha.len(), 64);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        let prune = target["prune"].as_array().expect("prune");
        assert!(prune
            .iter()
            .any(|p| p.as_str() == Some("llama-server") || p.as_str() == Some("llama-server.exe")));
        let installed = target["installed_bytes"].as_u64().expect("installed_bytes");
        let compressed = target["compressed_bytes"]
            .as_u64()
            .expect("compressed_bytes");
        assert!(installed <= 60 * 1024 * 1024, "{key} больше 60 МБ");
        assert!(compressed <= 30 * 1024 * 1024, "{key} архив больше 30 МБ");
        assert!(target["license_files"]
            .as_array()
            .map(|list| !list.is_empty())
            .unwrap_or(false));
    }
}

#[test]
fn gpu_pack_manifest_is_pinned_and_small() {
    let runtime = json(include_str!("../summary-runtime-manifest.json"));
    let manifest = json(include_str!("../summary-gpu-manifest.json"));
    assert_eq!(manifest["build"], runtime["build"]);
    assert_eq!(manifest["commit"], runtime["commit"]);
    assert_eq!(manifest["version"], runtime["version"]);
    let packs = manifest["packs"].as_object().expect("packs");
    assert!(packs.contains_key("linux-x86_64"));
    assert!(packs.contains_key("windows-x86_64"));
    assert!(!packs.contains_key("macos-arm64"), "macOS Metal встроен");
    for (key, pack) in packs {
        assert!(pack["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/b12182/"));
        let sha = pack["sha256"].as_str().expect("sha256");
        assert_eq!(sha.len(), 64, "{key}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(pack["compressed_bytes"].as_u64().unwrap_or(0) <= 48 * 1024 * 1024);
        assert!(pack["installed_bytes"].as_u64().unwrap_or(0) <= 60 * 1024 * 1024);
        assert!(matches!(
            pack["format"].as_str(),
            Some("tar.gz") | Some("zip")
        ));
        assert!(!pack["library"].as_str().unwrap_or("").is_empty());
        assert!(pack["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with(pack["asset"].as_str().unwrap_or("")));
    }
}
