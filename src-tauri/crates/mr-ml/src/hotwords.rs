use std::collections::HashSet;

/// Словарь терминов для CTC-бонуса: чистые строки, без зависимости от ONNX,
/// чтобы конфиг и HTTP-слой пользовались ими и в сборке без моделей.
pub const MAX_HOTWORDS: usize = 500;
pub const MAX_HOTWORD_CHARS: usize = 64;
pub const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.55;

pub fn normalize_term(s: &str) -> String {
    s.to_lowercase()
        .replace('ё', "е")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn sanitize_hotwords(raw: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let n = normalize_term(r);
        if n.is_empty() || n.chars().count() > MAX_HOTWORD_CHARS {
            continue;
        }
        if seen.insert(n) {
            out.push(r.trim().to_string());
        }
        if out.len() >= MAX_HOTWORDS {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_caps_and_dedups() {
        let raw = vec![
            "  Сбер  ".to_string(),
            "сбер".to_string(),
            String::new(),
            "x".repeat(65),
            "Гiga".to_string(),
        ];
        assert_eq!(sanitize_hotwords(&raw), vec!["Сбер".to_string(), "Гiga".to_string()]);
        let many: Vec<String> = (0..600).map(|i| format!("слово{i}")).collect();
        assert_eq!(sanitize_hotwords(&many).len(), MAX_HOTWORDS);
        assert_eq!(normalize_term("Ёлка  Палка"), "елка палка");
    }
}
