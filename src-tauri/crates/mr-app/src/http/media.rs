//! Список записей, кэши метаданных и раздача медиа (Range-стриминг).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde_json::json;

use super::{hdr, percent_decode};
use crate::config::save_dir;
use crate::recording::now_secs;

const AUDIO_EXTS: &[&str] = &["mp3", "wav", "m4a", "ogg", "flac", "aac", "opus"];
/// Видеорасширения (расшифровка = извлечение аудиодорожки).
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov", "avi", "webm", "m4v"];

pub(super) fn is_media_ext(ext: &str) -> bool {
    AUDIO_EXTS.contains(&ext) || VIDEO_EXTS.contains(&ext)
}

pub(super) fn is_video_ext(ext: &str) -> bool {
    VIDEO_EXTS.contains(&ext)
}

pub(super) fn is_video_path(p: &Path) -> bool {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .map(|e| is_video_ext(&e))
        .unwrap_or(false)
}

/// Android: видео требует внешнего ffmpeg, которого в APK нет. UI скрывает
/// импорт видео, а сервер дополнительно отклоняет такие файлы.
pub(super) fn video_supported() -> bool {
    !cfg!(target_os = "android")
}

/// Верхняя граница кэшей листинга: без неё удалённые записи оставались в памяти навсегда.
const CACHE_LIMIT: usize = 4096;

type FileCache = std::collections::HashMap<String, (u64, u64, f64)>;

fn cached_value(
    cache: &Mutex<FileCache>,
    key: String,
    len: u64,
    mtime: u64,
    compute: impl FnOnce() -> f64,
) -> f64 {
    if let Ok(guard) = cache.lock() {
        if let Some((l, m, d)) = guard.get(&key) {
            if *l == len && *m == mtime {
                return *d;
            }
        }
    }
    let d = compute();
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= CACHE_LIMIT {
            guard.clear(); // простое вытеснение: локальная библиотека записей мала
        }
        guard.insert(key, (len, mtime, d));
    }
    d
}

/// Длительность файла с кэшем по (путь, размер, mtime): `ffprobe` — внешний процесс,
/// а список записей запрашивается при каждом обновлении UI.
fn media_duration_cached(path: &Path, len: u64, mtime: u64) -> f64 {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<FileCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    cached_value(
        cache,
        path.to_string_lossy().to_string(),
        len,
        mtime,
        || mr_audio::decode::media_duration_sec(path).unwrap_or(0.0),
    )
}

/// Число спикеров из .transcript.json с кэшем: без него каждый листинг
/// парсил мегабайтные JSON всех записей.
fn speakers_count_cached(path: &Path, len: u64, mtime: u64) -> u64 {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<FileCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    cached_value(
        cache,
        path.to_string_lossy().to_string(),
        len,
        mtime,
        || {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|d| {
                    let meta = d.get("meta");
                    meta.and_then(|m| m.get("speakers_info"))
                        .and_then(|s| s.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                        .max(
                            meta.and_then(|m| m.get("speakers"))
                                .and_then(|s| s.as_array())
                                .map(|a| a.len())
                                .unwrap_or(0),
                        )
                })
                .unwrap_or(0) as f64
        },
    ) as u64
}

pub(super) fn recordings_list() -> Vec<serde_json::Value> {
    let dir = save_dir();
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    let registry = crate::media_registry::load_registry();
    for entry in rd.flatten() {
        let p = entry.path();
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !is_media_ext(&ext) || registry.is_hidden(&p) {
            continue;
        }
        let md = entry.metadata().ok();
        let (size_mb, mtime, file_len) = md
            .map(|m| {
                let sz = m.len() as f64 / 1048576.0;
                let len = m.len();
                let mt = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                ((sz * 100.0).round() / 100.0, mt, len)
            })
            .unwrap_or((0.0, 0, 0));
        let duration_sec = (media_duration_cached(&p, file_len, mtime) * 10.0).round() / 10.0;
        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let jp = dir.join(format!("{stem}.transcript.json"));
        let has_transcript = jp.exists() && dir.join(format!("{stem}.md")).exists();
        let jp_meta = jp.clone();
        let speakers_count = if jp.exists() {
            speakers_count_cached(&jp_meta, file_len, mtime)
        } else {
            0
        };
        // Дата изменения в локальном времени (`%Y-%m-%d %H:%M:%S`).
        let date = chrono::DateTime::from_timestamp(mtime as i64, 0)
            .map(|dt| {
                chrono::DateTime::<chrono::Local>::from(dt)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default();
        let is_video = is_video_ext(&ext);
        out.push(json!({
            "name": p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "path": p.to_string_lossy(),
            "is_video": is_video,
            "size_mb": size_mb,
            "duration_sec": duration_sec,
            "mtime": mtime,
            "date": date,
            "has_transcript": has_transcript,
            "speakers_count": speakers_count,
            "owned": registry.is_owned(&p),
        }));
    }
    out.sort_by_key(|v| std::cmp::Reverse(v.get("mtime").and_then(|x| x.as_u64()).unwrap_or(0)));
    out
}

/// Лимит JSON-тела запроса: локальный UI присылает килобайты,
pub(super) fn safe_file_name(raw: &str) -> String {
    let decoded = percent_decode(raw);
    let name = Path::new(&decoded)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let t = name.trim().to_string();
    if t.is_empty() || t == "." || t == ".." {
        format!("upload_{}.mp4", now_secs())
    } else {
        t
    }
}

pub(super) fn header_value(request: &tiny_http::Request, key: &str) -> Option<String> {
    request.headers().iter().find_map(|h| {
        let f = format!("{}", h.field);
        if f.eq_ignore_ascii_case(key) {
            Some(format!("{}", h.value))
        } else {
            None
        }
    })
}

/// Только свои: запрос с локального адреса и (если браузер его прислал)
/// с локального Origin. Закрывает CSRF из чужой вкладки и DNS-rebinding.
pub(super) fn local_request_allowed(request: &tiny_http::Request, port: u16) -> bool {
    let host_ok = header_value(request, "Host").is_some_and(|h| {
        let h = h.trim().to_ascii_lowercase();
        ["127.0.0.1", "localhost"]
            .iter()
            .any(|a| h == *a || h == format!("{a}:{port}"))
    });
    let origin_ok = header_value(request, "Origin").is_none_or(|o| {
        let o = o.trim().to_ascii_lowercase();
        ["127.0.0.1", "localhost"]
            .iter()
            .any(|a| o == format!("http://{a}:{port}"))
    });
    host_ok && origin_ok
}

pub(super) fn guess_media_type(p: &Path) -> String {
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "m4v" => "video/x-m4v",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Раздача медиа из папки записей с поддержкой Range (для перемотки и `#t=` фрагментов).
pub(super) fn serve_audio_file(request: tiny_http::Request, url_path: &str) {
    let enc = url_path.strip_prefix("/audio/").unwrap_or("");
    let safe = safe_file_name(enc);
    let dir = match save_dir().canonicalize() {
        Ok(d) => d,
        Err(_) => save_dir(),
    };
    let cand = dir.join(&safe);
    let real = match cand.canonicalize() {
        Ok(r) => r,
        Err(_) => {
            let resp =
                tiny_http::Response::from_string("Audio file not found").with_status_code(404);
            let _ = request.respond(resp);
            return;
        }
    };
    if !real.starts_with(&dir) || !real.is_file() {
        let resp = tiny_http::Response::from_string("Audio file not found").with_status_code(404);
        let _ = request.respond(resp);
        return;
    }
    let size = std::fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
    let ctype = guess_media_type(&real);
    let range = header_value(&request, "Range").unwrap_or_default();
    if range.starts_with("bytes=") {
        let spec = range.trim_start_matches("bytes=");
        let (s, e) = spec.split_once('-').unwrap_or((spec, ""));
        if let Ok(start) = s.trim().parse::<u64>() {
            let end = e
                .trim()
                .parse::<u64>()
                .unwrap_or(size.saturating_sub(1))
                .min(size.saturating_sub(1));
            if start < size && start <= end {
                let len = end - start + 1;
                let mut f = match std::fs::File::open(&real) {
                    Ok(f) => f,
                    Err(_) => {
                        let resp =
                            tiny_http::Response::from_string("Read error").with_status_code(500);
                        let _ = request.respond(resp);
                        return;
                    }
                };
                use std::io::Seek;
                if f.seek(std::io::SeekFrom::Start(start)).is_err() {
                    let resp = tiny_http::Response::from_string("Read error").with_status_code(500);
                    let _ = request.respond(resp);
                    return;
                }
                // Стримим с диска: `bytes=0-` от видеоплеера не должен
                // аллоцировать файл целиком в память.
                let reader = f.take(len);
                let resp = tiny_http::Response::new(
                    206.into(),
                    vec![
                        hdr(&format!("Content-Type: {ctype}")),
                        hdr(&format!("Content-Range: bytes {start}-{end}/{size}")),
                        hdr(&format!("Content-Length: {len}")),
                        hdr("Accept-Ranges: bytes"),
                    ],
                    reader,
                    Some(len as usize),
                    None,
                );
                let _ = request.respond(resp);
                return;
            }
        }
    }
    match std::fs::File::open(&real) {
        Ok(f) => {
            // Потоковая отдача: файл не грузится в память целиком (важно для видео в гигабайты).
            let resp = tiny_http::Response::new(
                200.into(),
                vec![
                    hdr(&format!("Content-Type: {ctype}")),
                    hdr(&format!("Content-Length: {size}")),
                    hdr("Accept-Ranges: bytes"),
                ],
                f,
                Some(size as usize),
                None,
            );
            let _ = request.respond(resp);
        }
        Err(_) => {
            let resp = tiny_http::Response::from_string("Read error").with_status_code(500);
            let _ = request.respond(resp);
        }
    }
}

/// Разобранный заголовок WAV: где PCM-данные и их формат.
struct WavInfo {
    audio_format: u16,
    channels: u16,
    rate: u32,
    bits: u16,
    data_offset: u64,
    data_len: u64,
}

/// Разбор RIFF-заголовка (первый килобайт файла): fmt + data чанки.
fn parse_wav_header(head: &[u8]) -> Option<WavInfo> {
    if head.len() < 12 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let rd16 = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    let rd32 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<(u64, u64)> = None;
    while pos + 8 <= head.len() {
        let id = &head[pos..pos + 4];
        let sz = rd32(&head[pos + 4..pos + 8]) as usize;
        let body = pos + 8;
        if id == b"fmt " && sz >= 16 && body + 16 <= head.len() {
            fmt = Some((
                rd16(&head[body..body + 2]),
                rd16(&head[body + 2..body + 4]),
                rd32(&head[body + 4..body + 8]),
                rd16(&head[body + 14..body + 16]),
            ));
        } else if id == b"data" {
            data = Some((body as u64, sz as u64));
            break;
        }
        pos = body + sz + (sz & 1); // чанки выровнены по 2 байта
    }
    let (audio_format, channels, rate, bits) = fmt?;
    let (data_offset, data_len) = data?;
    if rate == 0 || channels == 0 || bits % 8 != 0 || bits > 64 {
        return None;
    }
    Some(WavInfo {
        audio_format,
        channels,
        rate,
        bits,
        data_offset,
        data_len,
    })
}

/// Канонический 44-байтовый WAV-заголовок (PCM=1 или IEEE float=3).
fn wav_header_bytes(info: &WavInfo, data_bytes: u32) -> Vec<u8> {
    let block = u32::from(info.channels) * u32::from(info.bits) / 8;
    let byte_rate = info.rate * block;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&info.audio_format.to_le_bytes());
    h.extend_from_slice(&info.channels.to_le_bytes());
    h.extend_from_slice(&info.rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    // block_align в WAV — поле u16, иначе заголовок получается 46 байт и битым.
    h.extend_from_slice(&(block as u16).to_le_bytes());
    h.extend_from_slice(&info.bits.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_bytes.to_le_bytes());
    h
}

/// Верхний потолок фрагмента: локальный UI, но память ответа ограничиваем.
const MAX_SNIPPET_SEC: f64 = 300.0;

/// GET /api/media/snippet?file=NAME&start=SEC&end=SEC: фрагмент записи как
/// маленький WAV, нарезанный байтовым срезом PCM — без декодирования, поэтому
/// мгновенно даже на часовом файле. Клиентский `#t=`-seek по гигабайтному WAV
/// вешал WebView (он буферизовал весь файл), фрагмент решает это в корне.
pub(super) fn serve_media_snippet(request: tiny_http::Request, query: &str) {
    let bad = |request: tiny_http::Request, msg: &str, code: u16| {
        let resp = tiny_http::Response::from_string(msg).with_status_code(code);
        let _ = request.respond(resp);
    };
    let Some(f) = super::query_param(query, "file") else {
        bad(request, "No file specified", 400);
        return;
    };
    let start = super::query_param(query, "start")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let end = super::query_param(query, "end")
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let channel = super::query_param(query, "channel").and_then(|v| v.parse::<usize>().ok());
    if !(0.0..=MAX_SNIPPET_SEC).contains(&(end - start)) || start < 0.0 {
        bad(request, "Bad time range", 400);
        return;
    }
    let safe = safe_file_name(&f);
    let path = save_dir().join(&safe);
    if !inside_save_dir(&path) || !path.is_file() {
        bad(request, "Audio file not found", 404);
        return;
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
    {
        // Не-WAV нарезке байтами не поддаётся: фронт играет их через #t=.
        bad(request, "Snippet supports WAV only", 415);
        return;
    }
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => {
            bad(request, "Read error", 500);
            return;
        }
    };
    use std::io::{Read, Seek, SeekFrom};
    let mut head = vec![0u8; 4096];
    let got = file.read(&mut head).unwrap_or(0);
    head.truncate(got);
    let Some(info) = parse_wav_header(&head) else {
        bad(request, "Not a RIFF/WAVE file", 415);
        return;
    };
    if let Some(ch) = channel {
        if ch >= info.channels as usize {
            bad(request, "Bad channel", 400);
            return;
        }
    }
    let file_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let data_available = info.data_len.min(file_len.saturating_sub(info.data_offset));
    let block = u64::from(info.channels) * u64::from(info.bits) / 8;
    let frames = data_available / block.max(1);
    let first = ((start * info.rate as f64).floor() as u64).min(frames);
    let last =
        (((end - start).min(MAX_SNIPPET_SEC) * info.rate as f64).ceil() as u64 + first).min(frames);
    if last <= first {
        bad(request, "Empty fragment", 400);
        return;
    }
    let n_bytes = (last - first) * block;
    if file
        .seek(SeekFrom::Start(info.data_offset + first * block))
        .is_err()
    {
        bad(request, "Read error", 500);
        return;
    }
    let mut pcm = vec![0u8; n_bytes as usize];
    if file.read_exact(&mut pcm).is_err() {
        bad(request, "Read error", 500);
        return;
    }
    let body = match channel {
        Some(ch) => match mono_channel_snippet(&info, &pcm, ch) {
            Some(b) => b,
            None => {
                bad(request, "Bad channel", 400);
                return;
            }
        },
        None => {
            let mut b = wav_header_bytes(&info, n_bytes as u32);
            b.extend_from_slice(&pcm);
            b
        }
    };
    let resp = tiny_http::Response::from_data(body)
        .with_header(hdr("Content-Type: audio/wav"))
        .with_header(hdr("Cache-Control: no-store"));
    let _ = request.respond(resp);
}

/// Моно-фрагмент выбранного канала: для split-записей фрагмент спикера не
/// должен звучать вместе с эхом другого канала. Поддерживаются PCM и float
/// WAV; размер сэмпла берётся из заголовка.
fn mono_channel_snippet(info: &WavInfo, pcm: &[u8], channel: usize) -> Option<Vec<u8>> {
    let sample_bytes = info.bits as usize / 8;
    let frame_bytes = info.channels as usize * sample_bytes;
    if sample_bytes == 0 || frame_bytes == 0 || channel >= info.channels as usize {
        return None;
    }
    let frames = pcm.len() / frame_bytes;
    if frames == 0 {
        return None;
    }
    let mut out_pcm = Vec::with_capacity(frames * sample_bytes);
    for f in 0..frames {
        let off = f * frame_bytes + channel * sample_bytes;
        out_pcm.extend_from_slice(&pcm[off..off + sample_bytes]);
    }
    let mono = WavInfo {
        audio_format: info.audio_format,
        channels: 1,
        rate: info.rate,
        bits: info.bits,
        data_offset: 44,
        data_len: out_pcm.len() as u64,
    };
    let mut body = wav_header_bytes(&mono, out_pcm.len() as u32);
    body.extend_from_slice(&out_pcm);
    Some(body)
}

/// Папка записей канонически (для проверок принадлежности путей).
fn save_dir_canon() -> PathBuf {
    save_dir().canonicalize().unwrap_or_else(|_| save_dir())
}

/// Путь обязан лежать внутри папки записей: иначе 404.
/// Файл может ещё не существовать (транскрипта нет) — тогда проверяем родителя.
pub(super) fn inside_save_dir(p: &Path) -> bool {
    let dir = save_dir_canon();
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    };
    if let Ok(real) = abs.canonicalize() {
        return real == dir || real.starts_with(&dir);
    }
    match abs.parent().and_then(|par| par.canonicalize().ok()) {
        Some(rp) => rp == dir || rp.starts_with(&dir),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> WavInfo {
        WavInfo {
            audio_format: 1,
            channels: 2,
            rate: 16000,
            bits: 16,
            data_offset: 44,
            data_len: 64000, // 1 секунда
        }
    }

    #[test]
    fn wav_header_roundtrip() {
        let i = info();
        let h = wav_header_bytes(&i, 64000);
        assert_eq!(h.len(), 44);
        let parsed = parse_wav_header(&h).expect("синтезированный заголовок обязан парситься");
        assert_eq!((parsed.channels, parsed.rate, parsed.bits), (2, 16000, 16));
        assert_eq!(parsed.data_offset, 44);
        assert_eq!(parsed.data_len, 64000);
    }

    #[test]
    fn wav_header_rejects_garbage() {
        assert!(parse_wav_header(b"").is_none());
        assert!(parse_wav_header(b"RIFX....WAVEfmt ").is_none());
    }

    #[test]
    fn snippet_channel_is_isolated_and_mono() {
        let info = info();
        // Два стерео-сэмпла: L=100, R=200, L=300, R=400 (i16 LE).
        let pcm: Vec<u8> = [100i16, 200, 300, 400]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let body = mono_channel_snippet(&info, &pcm, 0).expect("канал 0");
        let parsed = parse_wav_header(&body[..44]).expect("заголовок");
        assert_eq!(parsed.channels, 1);
        assert_eq!(parsed.data_len, 4);
        let expected = [pcm[0], pcm[1], pcm[4], pcm[5]];
        assert_eq!(&body[44..], &expected[..]);
        assert!(mono_channel_snippet(&info, &pcm, 2).is_none());
    }
}
