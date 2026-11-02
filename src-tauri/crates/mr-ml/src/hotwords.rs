use std::collections::HashSet;

/// Без зависимости от ONNX: конфиг и HTTP собираются и без моделей.
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

pub struct HotwordEntry {
    pub boost: String,
    pub from: Option<Vec<String>>,
    pub to_display: String,
}

pub fn parse_hotwords(raw: &[String]) -> Vec<HotwordEntry> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for r in sanitize_hotwords(raw) {
        if let Some((a, b)) = r.split_once("=>") {
            let from: Vec<String> = normalize_term(a)
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(cmp_norm_word)
                .collect();
            let to_display = b.trim().to_string();
            let boost = normalize_term(b);
            if from.is_empty() || boost.is_empty() {
                continue;
            }
            if from.join(" ") == boost {
                if seen.insert((String::new(), boost.clone())) {
                    out.push(HotwordEntry {
                        boost,
                        from: None,
                        to_display,
                    });
                }
            } else if seen.insert((from.join(" "), boost.clone())) {
                out.push(HotwordEntry {
                    boost,
                    from: Some(from),
                    to_display,
                });
            }
            continue;
        }
        let boost = normalize_term(&r);
        if !boost.is_empty() && seen.insert((String::new(), boost.clone())) {
            out.push(HotwordEntry {
                boost,
                from: None,
                to_display: r,
            });
        }
    }
    out
}

fn cmp_norm_word(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
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
        assert_eq!(
            sanitize_hotwords(&raw),
            vec!["Сбер".to_string(), "Гiga".to_string()]
        );
        let many: Vec<String> = (0..600).map(|i| format!("слово{i}")).collect();
        assert_eq!(sanitize_hotwords(&many).len(), MAX_HOTWORDS);
        assert_eq!(normalize_term("Ёлка  Палка"), "елка палка");
    }

    #[test]
    fn parse_pairs_and_plain() {
        let raw = vec![
            "Сбер".to_string(),
            "сбир => Сбер".to_string(),
            "юзер флоу => User Flow".to_string(),
            "Сбер => Сбербанк".to_string(),
            "битая => ".to_string(),
            "=> пусто".to_string(),
        ];
        let entries = parse_hotwords(&raw);
        assert_eq!(entries.len(), 4);
        assert!(entries[0].from.is_none());
        assert_eq!(entries[0].boost, "сбер");
        assert_eq!(entries[1].from, Some(vec!["сбир".to_string()]));
        assert_eq!(entries[1].boost, "сбер");
        assert_eq!(entries[1].to_display, "Сбер");
        assert_eq!(
            entries[2].from,
            Some(vec!["юзер".to_string(), "флоу".to_string()])
        );
        assert_eq!(entries[2].to_display, "User Flow");
        assert_eq!(entries[3].from, Some(vec!["сбер".to_string()]));
        assert_eq!(entries[3].boost, "сбербанк");
    }
}
