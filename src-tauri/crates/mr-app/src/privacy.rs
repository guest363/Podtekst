use std::borrow::Cow;
use std::path::Path;

/// Виртуальные драйверы и SCK: их имена технические и нужны для диагностики.
const KNOWN_DEVICES: [&str; 5] = [
    "blackhole",
    "soundflower",
    "loopback",
    "virtual cable",
    "screencapturekit",
];

/// Слова, которые остаются в имени технического устройства рядом с драйвером.
const TECHNICAL_WORDS: [&str; 14] = [
    "blackhole",
    "soundflower",
    "loopback",
    "virtual",
    "cable",
    "screencapturekit",
    "системный",
    "звук",
    "audio",
    "mic",
    "микрофон",
    "stereo",
    "mono",
    "ch",
];

/// Пунктуация, которой путь может быть облеплен в логе: `"…"`, `(…)`, `-> …`.
const LEAD_JUNK: [char; 8] = ['"', '\'', '(', '[', '{', '<', '=', '«'];
const TAIL_JUNK: [char; 9] = ['"', '\'', ')', ']', '}', '>', ',', ';', '»'];
/// Путь с пробелами в имени: не больше шести слов, иначе это уже не путь.
const MAX_PATH_WORDS: usize = 6;

pub(crate) fn redact_path(path: &Path) -> String {
    redact_text(&path.to_string_lossy())
}

pub(crate) fn redact_text(text: &str) -> String {
    redact_with(text, home_dir().as_deref())
}

/// Техническое имя устройства: известный драйвер плюс только технические слова.
/// Личное название вроде «Loopback Studio Mic» проходит мимо (P12).
fn technical_device(lower: &str) -> bool {
    if !KNOWN_DEVICES.iter().any(|needle| lower.contains(needle)) {
        return false;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .all(|word| word.chars().any(|c| c.is_ascii_digit()) || TECHNICAL_WORDS.contains(&word))
}

pub(crate) fn mask_device_name(name: &str, index: usize) -> String {
    let lower = name.to_lowercase();
    if technical_device(&lower) {
        name.to_string()
    } else {
        format!("Устройство {}", index + 1)
    }
}

fn home_dir() -> Option<String> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

fn redact_with(text: &str, home: Option<&str>) -> String {
    let tokenized = redact_paths(text);
    match home {
        Some(home) if !home.is_empty() => tokenized.replace(home, "~"),
        _ => tokenized,
    }
}

pub(crate) fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Имя записи по умолчанию совпадает с темой встречи, поэтому в лог уходит
/// только токен: расширение сохраняем, имя заменяем хешем (P12).
pub(crate) fn path_token(name: &str) -> String {
    let hash = fnv1a(name) & 0xffff_ffff;
    match extension_of(name) {
        Some(extension) => format!("recording-{hash:08x}.{extension}"),
        None => format!("recording-{hash:08x}"),
    }
}

fn extension_of(name: &str) -> Option<String> {
    let (_, extension) = name.rsplit_once('.')?;
    let plausible = !extension.is_empty()
        && extension.len() <= 8
        && extension.chars().all(|c| c.is_alphanumeric());
    plausible.then(|| extension.to_ascii_lowercase())
}

fn last_segment(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// P12: в свободном тексте (app.log, crash-отчёт, диагностика) полные пути
/// заменяются на токены. Срабатывает на абсолютные пути с расширением файла:
/// `/api/status` и `и/или` остаются как есть.
pub(crate) fn redact_paths(text: &str) -> String {
    if !text.contains('/') && !text.contains('\\') {
        return text.to_string();
    }
    text.split('\n')
        .map(redact_line)
        .collect::<Vec<String>>()
        .join("\n")
}

fn redact_line(line: &str) -> String {
    // Пути печатаются через `{:?}`, то есть в кавычках, а имя записи может
    // содержать пробелы: кавычки обрабатываем целиком, остальное — по словам.
    let mut out = String::with_capacity(line.len());
    for (index, chunk) in line.split('"').enumerate() {
        if index > 0 {
            out.push('"');
        }
        let trimmed = chunk.trim();
        if index % 2 == 1 && looks_like_file_path(trimmed) {
            let lead = chunk.len() - chunk.trim_start().len();
            let tail = chunk.trim_end().len();
            out.push_str(&chunk[..lead]);
            out.push_str(&path_token(last_segment(trimmed)));
            out.push_str(&chunk[tail..]);
        } else {
            out.push_str(&redact_words(chunk));
        }
    }
    out
}

fn redact_word(word: &str) -> Cow<'_, str> {
    let start = word.len() - word.trim_start_matches(LEAD_JUNK).len();
    let head = &word[start..];
    let end = start + head.trim_end_matches(TAIL_JUNK).len();
    let path = &word[start..end];
    if !looks_like_file_path(path) {
        return Cow::Borrowed(word);
    }
    Cow::Owned(format!(
        "{}{}{}",
        &word[..start],
        path_token(last_segment(path)),
        &word[end..]
    ))
}

/// Имя записи может содержать пробелы: склеиваем слова, пока не появится
/// правдоподобное расширение файла.
fn redact_words(chunk: &str) -> String {
    let words: Vec<&str> = chunk.split(' ').collect();
    let mut out: Vec<Cow<'_, str>> = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        let trimmed = word.trim_start_matches(LEAD_JUNK);
        if absolute_start(trimmed) && extension_of(last_segment(trimmed)).is_none() {
            let mut end = index;
            let mut joined = trimmed.to_string();
            while end + 1 < words.len() && end - index < MAX_PATH_WORDS {
                end += 1;
                joined.push(' ');
                joined.push_str(words[end].trim_end_matches(TAIL_JUNK));
                if extension_of(last_segment(&joined)).is_some() {
                    break;
                }
            }
            if looks_like_file_path(&joined) {
                out.push(Cow::Owned(path_token(last_segment(&joined))));
                index = end + 1;
                continue;
            }
        }
        out.push(redact_word(word));
        index += 1;
    }
    out.join(" ")
}

fn absolute_start(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("~/")
        || path.starts_with(".\\")
        || is_drive_path(path)
}

fn looks_like_file_path(path: &str) -> bool {
    if !absolute_start(path) {
        return false;
    }
    let segments = path.split(['/', '\\']).filter(|s| !s.is_empty()).count();
    segments >= 2 && extension_of(last_segment(path)).is_some()
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_file_name_independently_of_home() {
        let text = "/home/user/recordings/2026-11-03.wav";
        let token = path_token("2026-11-03.wav");
        assert_eq!(redact_with(text, Some("/home/user")), token);
        assert_eq!(redact_with(text, None), token);
        assert_eq!(redact_with("/tmp/без-дома", None), "/tmp/без-дома");
    }

    #[test]
    fn masks_personal_device_names() {
        assert_eq!(mask_device_name("AirPods Pro Test", 0), "Устройство 1");
        assert_eq!(mask_device_name("BlackHole 2ch", 1), "BlackHole 2ch");
        assert_eq!(
            mask_device_name("Системный звук (ScreenCaptureKit)", 0),
            "Системный звук (ScreenCaptureKit)"
        );
        // Техническое слово в личном названии не делает устройство техническим.
        assert_eq!(mask_device_name("Loopback Studio Mic", 0), "Устройство 1");
        assert_eq!(
            mask_device_name("Гарнитура (virtual cable)", 0),
            "Устройство 1"
        );
    }

    #[test]
    fn redacts_path_with_known_home() {
        let path = Path::new("/tmp/example/recording.wav");
        let redacted = redact_path(path);
        assert!(redacted.starts_with("recording-"), "{redacted}");
        assert!(redacted.ends_with(".wav"), "{redacted}");
    }

    #[test]
    fn hides_file_names_in_free_text_and_keeps_technical_suffixes() {
        let line = "[ATTACH_VIDEO] Получен видеопоток: 100 байт для аудио \"/home/u/Встреча с командой.wav\"";
        let redacted = redact_text(line);
        assert!(!redacted.contains("Встреча"), "{redacted}");
        assert!(!redacted.contains("/home/u"), "{redacted}");
        assert!(redacted.contains("recording-"), "{redacted}");
        assert!(redacted.contains(".wav\""), "{redacted}");
    }

    #[test]
    fn redacts_windows_and_relative_home_paths() {
        for line in [
            r"C:\Users\user\Music\Встреча.mp4",
            "~/Recordings/Встреча.mp4",
        ] {
            let redacted = redact_text(line);
            assert!(!redacted.contains("Встреча"), "{redacted}");
            assert!(redacted.contains("recording-"), "{redacted}");
        }
    }

    #[test]
    fn keeps_urls_routes_and_plain_words() {
        for line in [
            "GET /api/status 200",
            "https://tips.tips/000490393",
            "режим моно/стерео",
            "Кратко/Подробно",
        ] {
            assert_eq!(redact_text(line), line);
        }
    }

    #[test]
    fn token_is_stable_and_idempotent() {
        let once = path_token("Встреча с командой.wav");
        assert_eq!(once, path_token("Встреча с командой.wav"));
        let line = format!("audio {once} saved");
        assert_eq!(redact_text(&line), line);
    }
}
