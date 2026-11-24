//! Бонус hotwords: внутрипоисковый префиксный trie на шагах бима и финальный рескоринг
//! целых слов с проверкой границ токенов `▁`.

use std::collections::HashMap;
use std::path::Path;

use super::accel::{ExecutionProviderKind, SessionProfile};
use super::features;
use super::session::{outputs_are_finite, OrtSession};
use crate::hotwords::{normalize_term, normalize_word, parse_hotwords};

pub use crate::hotwords::{DEFAULT_CONFIDENCE_THRESHOLD, MAX_HOTWORDS, MAX_HOTWORD_CHARS};

pub const HOTWORD_BONUS: f32 = 3.0;

pub fn hotword_bonus() -> f32 {
    static VALUE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        if std::env::var("MR_HOTWORD_BENCH").as_deref() != Ok("1") {
            return HOTWORD_BONUS;
        }
        std::env::var("MR_HOTWORD_BONUS")
            .ok()
            .and_then(|raw| parse_bonus(&raw))
            .unwrap_or(HOTWORD_BONUS)
    })
}

fn parse_bonus(raw: &str) -> Option<f32> {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=10.0).contains(value))
}

pub const BEAM_WIDTH: usize = 8;
const TOP_K: usize = 16;

#[derive(Debug, Clone)]
pub struct CtcWord {
    pub text: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct CtcHypothesis {
    pub text: String,
    pub words: Vec<CtcWord>,
}

/// Времена слов относительно начала сегмента; `None` — CTC не дал кадр.
pub type CtcWordTimes = Vec<Option<(f64, f64)>>;
pub type TimedCtcHypothesis = (CtcHypothesis, CtcWordTimes);

#[derive(Debug, Clone)]
pub struct AlignedWord {
    pub text: String,
    pub confidence: f64,
    pub needs_review: bool,
    pub start_sec: f64,
    pub end_sec: f64,
}

fn tokenize_greedy(term: &str, vocab: &[String]) -> Option<Vec<i64>> {
    let sep = vocab.iter().position(|p| p == "▁")? as i64;
    let mut ids = Vec::new();
    for word in normalize_term(term).split(' ').filter(|w| !w.is_empty()) {
        let chars: Vec<char> = word.chars().collect();
        let mut pos = 0;
        let mut first = true;
        while pos < chars.len() {
            let mut best: Option<(usize, i64)> = None;
            for (id, piece) in vocab.iter().enumerate() {
                let bare = piece.strip_prefix('▁').unwrap_or(piece);
                if bare.is_empty() {
                    continue;
                }
                if first && !piece.starts_with('▁') {
                    continue;
                }
                if !first && piece.starts_with('▁') {
                    continue;
                }
                let blen = bare.chars().count();
                if pos + blen > chars.len() {
                    continue;
                }
                if chars[pos..pos + blen].iter().collect::<String>() == *bare
                    && best.map(|(l, _)| blen > l).unwrap_or(true)
                {
                    best = Some((blen, id as i64));
                }
            }
            match best {
                Some((l, id)) => {
                    ids.push(id);
                    pos += l;
                }
                None if first => ids.push(sep),
                None => return None,
            }
            first = false;
        }
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn tokenize_chars(term: &str, vocab: &[String]) -> Option<Vec<i64>> {
    let sep = vocab.iter().position(|p| p == "▁")? as i64;
    let mut ids = Vec::new();
    for word in normalize_term(term).split(' ').filter(|w| !w.is_empty()) {
        ids.push(sep);
        for ch in word.chars() {
            let s = ch.to_string();
            ids.push(vocab.iter().position(|p| *p == s)? as i64);
        }
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

fn tokenize_term(term: &str, vocab: &[String]) -> Vec<Vec<i64>> {
    let mut out = Vec::new();
    for v in [tokenize_greedy(term, vocab), tokenize_chars(term, vocab)]
        .into_iter()
        .flatten()
    {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// Контракт входа не фиксирован экспортом модели: часть сборок отдаёт
/// softmax-вероятности, часть — лог-вероятности. Вероятности неотрицательны и
/// суммируются в 1; лог-вероятности всегда содержат отрицательные значения.
fn to_log_row(row: &[f32]) -> Vec<f32> {
    let sum: f64 = row.iter().map(|&x| x as f64).sum();
    let non_negative = row.iter().all(|&x| x >= 0.0);
    if non_negative && (sum - 1.0).abs() <= 1e-3 {
        return row.iter().map(|&x| x.max(1e-12).ln()).collect();
    }
    // Для уже нормированных лог-вероятностей LSE = 0, поэтому повторный
    // log_softmax идемпотентен. Ветка «все ≤ 0 — значит уже лог» ошибалась
    // на сырых логитах с отрицательным максимумом.
    let lse = row.iter().fold(f32::NEG_INFINITY, |a, &x| log_add(a, x));
    row.iter().map(|x| x - lse).collect()
}

fn log_add(a: f32, b: f32) -> f32 {
    if a == f32::NEG_INFINITY {
        return b;
    }
    if b == f32::NEG_INFINITY {
        return a;
    }
    let (m, d) = if a > b { (a, b - a) } else { (b, a - b) };
    m + (1.0 + d.exp()).ln()
}

fn render_words(ids: &[i64], probs: &[f64], vocab: &[String], blank: usize) -> Vec<CtcWord> {
    render_words_timed(ids, probs, &[], vocab, blank).0
}

type FrameSpan = Option<(usize, usize)>;

#[allow(clippy::too_many_arguments)]
fn flush_word(
    cur: &mut String,
    acc: &mut f64,
    n: &mut usize,
    start: &mut Option<usize>,
    end: &mut Option<usize>,
    words: &mut Vec<CtcWord>,
    spans: &mut Vec<FrameSpan>,
) {
    if cur.is_empty() {
        return;
    }
    words.push(CtcWord {
        text: cur.clone(),
        confidence: if *n > 0 { *acc / *n as f64 } else { 0.0 },
    });
    spans.push(match (*start, *end) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    });
    cur.clear();
    *acc = 0.0;
    *n = 0;
    *start = None;
    *end = None;
}

/// `id_frames[k]` — номер кадра CTC для `ids[k]`; пустой срез допустим
/// (бонусный beam не хранит кадры, времена тогда неизвестны).
fn render_words_timed(
    ids: &[i64],
    probs: &[f64],
    id_frames: &[usize],
    vocab: &[String],
    blank: usize,
) -> (Vec<CtcWord>, Vec<FrameSpan>) {
    let mut words = Vec::new();
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut acc = 0.0;
    let mut n = 0usize;
    let mut start = None;
    let mut end = None;
    let mut skipped = 0usize;
    for (k, &id) in ids.iter().enumerate() {
        if id as usize == blank {
            skipped += 1;
            continue;
        }
        if id == 0 {
            skipped += 1;
            flush_word(
                &mut cur, &mut acc, &mut n, &mut start, &mut end, &mut words, &mut spans,
            );
            continue;
        }
        let Some(piece) = vocab.get(id as usize) else {
            skipped += 1;
            continue;
        };
        let frame = id_frames.get(k).copied();
        if piece.starts_with('▁') {
            flush_word(
                &mut cur, &mut acc, &mut n, &mut start, &mut end, &mut words, &mut spans,
            );
            start = frame;
            cur.push_str(piece.strip_prefix('▁').unwrap_or(piece));
        } else {
            cur.push_str(piece);
            start = start.or(frame);
        }
        end = frame.or(end);
        // probs параллелен ids без blank/<unk> — их пропускает жадный декодер.
        acc += probs.get(k - skipped).copied().unwrap_or(0.0);
        n += 1;
    }
    flush_word(
        &mut cur, &mut acc, &mut n, &mut start, &mut end, &mut words, &mut spans,
    );
    (words, spans)
}

pub fn decode_ctc_greedy(logprobs: &[Vec<f32>], vocab: &[String], blank: usize) -> CtcHypothesis {
    decode_ctc_greedy_timed(logprobs, vocab, blank).0
}

pub fn decode_ctc_greedy_timed(
    logprobs: &[Vec<f32>],
    vocab: &[String],
    blank: usize,
) -> (CtcHypothesis, Vec<FrameSpan>) {
    let rows: Vec<Vec<f32>> = logprobs.iter().map(|row| to_log_row(row)).collect();
    decode_ctc_greedy_rows(&rows, vocab, blank)
}

/// Жадный декодер по уже посчитанным лог-строкам: beam-ветка не должна
/// считать log-softmax по всем кадрам второй раз.
fn decode_ctc_greedy_rows(
    rows: &[Vec<f32>],
    vocab: &[String],
    blank: usize,
) -> (CtcHypothesis, Vec<FrameSpan>) {
    let mut ids = Vec::new();
    let mut probs = Vec::new();
    let mut id_frames = Vec::new();
    let mut prev: i64 = -1;
    for (frame, lp) in rows.iter().enumerate() {
        let (mut best, mut bl) = (0usize, f32::NEG_INFINITY);
        for (i, &x) in lp.iter().enumerate() {
            if x > bl {
                bl = x;
                best = i;
            }
        }
        let id = best as i64;
        if best == blank {
            prev = id;
            continue;
        }
        if best == 0 {
            ids.push(0);
            id_frames.push(frame);
            prev = id;
            continue;
        }
        if id == prev {
            continue;
        }
        prev = id;
        ids.push(id);
        id_frames.push(frame);
        probs.push(bl.exp() as f64);
    }
    let (words, spans) = render_words_timed(&ids, &probs, &id_frames, vocab, blank);
    (
        CtcHypothesis {
            text: words
                .iter()
                .map(|w| w.text.clone())
                .collect::<Vec<_>>()
                .join(" "),
            words,
        },
        spans,
    )
}

fn is_word_boundary(id: i64, vocab: &[String]) -> bool {
    id == 0
        || vocab
            .get(id as usize)
            .is_some_and(|piece| piece.starts_with('▁'))
}

fn contains_seq(hay: &[i64], needle: &[i64], vocab: &[String]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).enumerate().any(|(i, w)| {
        w == needle
            && hay
                .get(i + needle.len())
                .map(|&id| is_word_boundary(id, vocab))
                .unwrap_or(true)
    })
}

struct TrieNode {
    children: Vec<(i64, usize)>,
    terminal_id: Option<usize>,
    min_end_depth: usize,
}

struct HotwordTrie {
    nodes: Vec<TrieNode>,
}

impl HotwordTrie {
    fn build(hotword_ids: &[Vec<i64>]) -> Self {
        let mut nodes = vec![TrieNode {
            children: Vec::new(),
            terminal_id: None,
            min_end_depth: usize::MAX,
        }];
        for (idx, hw) in hotword_ids.iter().enumerate() {
            if hw.is_empty() {
                continue;
            }
            let len = hw.len();
            let mut cur = 0;
            nodes[cur].min_end_depth = nodes[cur].min_end_depth.min(len);
            for &tok in hw {
                let next = match nodes[cur].children.binary_search_by_key(&tok, |(k, _)| *k) {
                    Ok(pos) => nodes[cur].children[pos].1,
                    Err(pos) => {
                        let id = nodes.len();
                        nodes.push(TrieNode {
                            children: Vec::new(),
                            terminal_id: None,
                            min_end_depth: usize::MAX,
                        });
                        nodes[cur].children.insert(pos, (tok, id));
                        id
                    }
                };
                cur = next;
                nodes[cur].min_end_depth = nodes[cur].min_end_depth.min(len);
            }
            if nodes[cur].terminal_id.is_none() {
                nodes[cur].terminal_id = Some(idx);
            }
        }
        Self { nodes }
    }

    fn score(
        &self,
        prefix: &[i64],
        vocab: &[String],
        bonus: f32,
        seen_epoch: &mut [u32],
        epoch: u32,
    ) -> f32 {
        let mut completed = 0usize;
        let mut best_lookahead = 0.0f32;
        for start in 0..prefix.len() {
            let mut cur = 0usize;
            let mut step = 0usize;
            for &tok in &prefix[start..] {
                let Ok(pos) = self.nodes[cur]
                    .children
                    .binary_search_by_key(&tok, |(k, _)| *k)
                else {
                    break;
                };
                cur = self.nodes[cur].children[pos].1;
                step += 1;
                let end_pos = start + step;
                if let Some(tid) = self.nodes[cur].terminal_id {
                    if end_pos < prefix.len() {
                        if is_word_boundary(prefix[end_pos], vocab) && seen_epoch[tid] != epoch {
                            seen_epoch[tid] = epoch;
                            completed += 1;
                        }
                    } else if seen_epoch[tid] != epoch {
                        best_lookahead = best_lookahead.max(bonus);
                    }
                } else if end_pos == prefix.len() {
                    let frac = step as f32 / self.nodes[cur].min_end_depth.max(1) as f32;
                    best_lookahead = best_lookahead.max(bonus * frac);
                }
            }
        }
        completed as f32 * bonus + best_lookahead
    }
}

pub fn decode_ctc_beam(
    logprobs: &[Vec<f32>],
    vocab: &[String],
    blank: usize,
    beam_width: usize,
    hotword_ids: &[Vec<i64>],
    bonus: f32,
) -> CtcHypothesis {
    decode_ctc_beam_timed(logprobs, vocab, blank, beam_width, hotword_ids, bonus).0
}

pub fn decode_ctc_beam_timed(
    logprobs: &[Vec<f32>],
    vocab: &[String],
    blank: usize,
    beam_width: usize,
    hotword_ids: &[Vec<i64>],
    bonus: f32,
) -> (CtcHypothesis, Vec<FrameSpan>) {
    let bw = beam_width.max(1);
    let trie = (!hotword_ids.is_empty() && bonus > 0.0).then(|| HotwordTrie::build(hotword_ids));
    let mut seen_epoch = if trie.is_some() {
        vec![0u32; hotword_ids.len()]
    } else {
        Vec::new()
    };
    let mut epoch = 0u32;
    let mut beams: Vec<(Vec<i64>, f32, f32)> = vec![(Vec::new(), 0.0, f32::NEG_INFINITY)];
    let mut log_rows: Vec<Vec<f32>> = Vec::with_capacity(logprobs.len());
    for row in logprobs {
        let lp = to_log_row(row);
        let mut order: Vec<usize> = (0..lp.len()).collect();
        if order.len() > TOP_K {
            order.select_nth_unstable_by(TOP_K, |&a, &b| lp[b].total_cmp(&lp[a]));
            order.truncate(TOP_K);
        }
        if !order.contains(&blank) {
            order.push(blank);
        }
        let mut next: HashMap<Vec<i64>, (f32, f32)> = HashMap::new();
        for (prefix, pb, pnb) in &beams {
            let total = log_add(*pb, *pnb);
            let e = next
                .entry(prefix.clone())
                .or_insert((f32::NEG_INFINITY, f32::NEG_INFINITY));
            e.0 = log_add(e.0, total + lp[blank]);
            let last = prefix.last().copied();
            for &c in &order {
                if c == blank {
                    continue;
                }
                if Some(c as i64) == last {
                    let e = next
                        .entry(prefix.clone())
                        .or_insert((f32::NEG_INFINITY, f32::NEG_INFINITY));
                    e.1 = log_add(e.1, *pnb + lp[c]);
                    let mut np = prefix.clone();
                    np.push(c as i64);
                    let e2 = next
                        .entry(np)
                        .or_insert((f32::NEG_INFINITY, f32::NEG_INFINITY));
                    e2.1 = log_add(e2.1, *pb + lp[c]);
                } else {
                    let mut np = prefix.clone();
                    np.push(c as i64);
                    let e = next
                        .entry(np)
                        .or_insert((f32::NEG_INFINITY, f32::NEG_INFINITY));
                    e.1 = log_add(e.1, total + lp[c]);
                }
            }
        }
        // ЗАЧЕМ: при узком beam многотокеновые термины выбиваются до конца слова,
        // поэтому учитываем префиксный trie в ранжировании pruning без искажения pb/pnb.
        let mut scored: Vec<_> = if let Some(trie) = trie.as_ref() {
            next.into_iter()
                .map(|(p, (a, b))| {
                    epoch = epoch.wrapping_add(1);
                    if epoch == 0 {
                        seen_epoch.fill(0);
                        epoch = 1;
                    }
                    let score =
                        log_add(a, b) + trie.score(&p, vocab, bonus, &mut seen_epoch, epoch);
                    (score, (p, (a, b)))
                })
                .collect()
        } else {
            next.into_iter()
                .map(|(p, (a, b))| (log_add(a, b), (p, (a, b))))
                .collect()
        };
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(bw);
        beams = scored
            .into_iter()
            .map(|(_, (p, (a, b)))| (p, a, b))
            .collect();
        log_rows.push(lp);
    }
    let mut best_prefix: Vec<i64> = Vec::new();
    let mut best_score = f32::NEG_INFINITY;
    for (prefix, pb, pnb) in &beams {
        let mut s = log_add(*pb, *pnb);
        for hw in hotword_ids {
            if contains_seq(prefix, hw, vocab) {
                s += bonus;
            }
        }
        if s > best_score {
            best_score = s;
            best_prefix = prefix.clone();
        }
    }
    let (greedy, greedy_spans) = decode_ctc_greedy_rows(&log_rows, vocab, blank);
    let beam_texts: Vec<String> = {
        let w = render_words(&best_prefix, &vec![1.0; best_prefix.len()], vocab, blank);
        w.into_iter().map(|x| x.text).collect()
    };
    let (confs, spans) = align_conf_timed(&beam_texts, &greedy.words, &greedy_spans);
    let words: Vec<CtcWord> = beam_texts
        .into_iter()
        .zip(confs)
        .map(|(text, confidence)| CtcWord { text, confidence })
        .collect();
    (
        CtcHypothesis {
            text: words
                .iter()
                .map(|w| w.text.clone())
                .collect::<Vec<_>>()
                .join(" "),
            words,
        },
        spans,
    )
}

fn align_map(a: &[String], b: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate().skip(1) {
        row[0] = i;
    }
    for (j, v) in dp[0].iter_mut().enumerate().skip(1) {
        *v = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = dp[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]);
            dp[i][j] = sub.min(dp[i - 1][j] + 1).min(dp[i][j - 1] + 1);
        }
    }
    let mut map: Vec<Option<usize>> = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if dp[i][j] == dp[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]) {
            map[i - 1] = Some(j - 1);
            i -= 1;
            j -= 1;
        } else if dp[i][j] == dp[i - 1][j] + 1 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    map
}

fn align_conf_timed(
    target: &[String],
    source: &[CtcWord],
    spans: &[FrameSpan],
) -> (Vec<f64>, Vec<FrameSpan>) {
    let a: Vec<String> = target.iter().map(|w| normalize_word(w)).collect();
    let b: Vec<String> = source.iter().map(|w| normalize_word(&w.text)).collect();
    align_map(&a, &b)
        .into_iter()
        .map(|o| {
            o.map(|k| (source[k].confidence, spans.get(k).copied().unwrap_or(None)))
                .unwrap_or((0.0, None))
        })
        .unzip()
}

// Без гейта близости бонус CTC затирал бы несвязанные слова («проект» → «объектом»).
fn edit_dist(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

// shortcut: лишние CTC-слова не вставляем; вставлять, когда появятся word timestamps.
pub fn align_to_reference(
    rnnt_text: &str,
    ctc: &CtcHypothesis,
    hotwords: &[String],
    threshold: f64,
) -> Vec<AlignedWord> {
    align_to_reference_timed(rnnt_text, ctc, &[], hotwords, threshold)
}

pub fn align_to_reference_timed(
    rnnt_text: &str,
    ctc: &CtcHypothesis,
    times: &[Option<(f64, f64)>],
    hotwords: &[String],
    threshold: f64,
) -> Vec<AlignedWord> {
    let entries = parse_hotwords(hotwords);
    let mut full: HashMap<String, &str> = entries
        .iter()
        .map(|e| (normalize_word(&e.boost), e.to_display.as_str()))
        .collect();
    for entry in &entries {
        if entry.to_display.contains(' ') {
            for word in entry.to_display.split_whitespace() {
                let key = normalize_word(word);
                if !key.is_empty() {
                    full.entry(key).or_insert(word);
                }
            }
        }
    }
    let in_dict = |w: &str| -> bool {
        let n = normalize_word(w);
        full.contains_key(&n)
            || normalize_term(w)
                .split(' ')
                .any(|p| full.contains_key(&normalize_word(p)))
    };
    let rwords: Vec<String> = rnnt_text.split_whitespace().map(str::to_string).collect();
    let a: Vec<String> = rwords.iter().map(|w| normalize_word(w)).collect();
    let b: Vec<String> = ctc.words.iter().map(|w| normalize_word(&w.text)).collect();
    let src_of = align_map(&a, &b);
    let out: Vec<AlignedWord> = rwords
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let mut text = w.clone();
            let mut confidence = 0.0;
            let (mut start_sec, mut end_sec) = (0.0, 0.0);
            if let Some(Some(j)) = src_of.get(i).copied() {
                if let Some(cw) = ctc.words.get(j) {
                    confidence = cw.confidence;
                    if let Some((a, b)) = times.get(j).copied().flatten() {
                        start_sec = a;
                        end_sec = b;
                    }
                    let nw = normalize_word(&w);
                    let nc = normalize_word(&cw.text);
                    // SAFETY: бонусное слово — только при близком написании и уверенности ≥ порога;
                    // `longest >= 2` открывает замену 1 символа в двухбуквенных аббревиатурах.
                    let close = {
                        let longest = nw.chars().count().max(nc.chars().count());
                        !nc.is_empty()
                            && longest >= 2
                            && edit_dist(&nw, &nc) <= (longest / 3).max(1)
                    };
                    if nc != nw && confidence >= threshold && close && in_dict(&cw.text) {
                        text = full
                            .get(&nc)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| cw.text.clone());
                    }
                }
            }
            let needs_review = confidence < threshold;
            AlignedWord {
                text,
                confidence,
                needs_review,
                start_sec,
                end_sec,
            }
        })
        .collect();
    let mut pairs: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for e in &entries {
        if let Some(from) = &e.from {
            let to: Vec<String> = e
                .to_display
                .split_whitespace()
                .map(str::to_string)
                .collect();
            if !to.is_empty() {
                pairs.push((from.clone(), to));
            }
        }
    }
    pairs.sort_by_key(|x| std::cmp::Reverse(x.0.len()));
    if pairs.is_empty() {
        return out;
    }
    let mut protected = vec![false; out.len()];
    for entry in &entries {
        let term: Vec<String> = entry
            .to_display
            .split_whitespace()
            .map(normalize_word)
            .filter(|word| !word.is_empty())
            .collect();
        if term.is_empty() || term.len() > out.len() {
            continue;
        }
        for start in 0..=out.len() - term.len() {
            if out[start..start + term.len()]
                .iter()
                .zip(&term)
                .all(|(word, expected)| normalize_word(&word.text) == *expected)
            {
                protected[start..start + term.len()].fill(true);
            }
        }
    }
    let mut res = Vec::with_capacity(out.len());
    let mut i = 0;
    while i < out.len() {
        let mut hit: Option<(&Vec<String>, usize)> = None;
        for (from, to) in &pairs {
            if out.len() - i >= from.len()
                && !protected[i..i + from.len()].iter().any(|flag| *flag)
                && out[i..i + from.len()]
                    .iter()
                    .zip(from)
                    .all(|(w, f)| normalize_word(&w.text) == *f)
            {
                hit = Some((to, from.len()));
                break;
            }
        }
        if let Some((to, n)) = hit {
            let conf = out[i..i + n].iter().map(|w| w.confidence).sum::<f64>() / n as f64;
            let start_sec = out[i..i + n]
                .iter()
                .map(|w| w.start_sec)
                .filter(|&x| x > 0.0)
                .fold(f64::INFINITY, f64::min);
            let end_sec = out[i..i + n]
                .iter()
                .map(|w| w.end_sec)
                .fold(0.0f64, f64::max);
            let (start_sec, end_sec) = if start_sec.is_finite() {
                (start_sec, end_sec)
            } else {
                (0.0, 0.0)
            };
            res.extend(to.iter().map(|t| AlignedWord {
                text: t.clone(),
                confidence: conf,
                needs_review: false,
                start_sec,
                end_sec,
            }));
            i += n;
        } else {
            res.push(out[i].clone());
            i += 1;
        }
    }
    res
}

pub struct GigaCTC {
    sess: OrtSession,
    in_feat: String,
    in_len: Option<String>,
    out_lp: String,
    vocab: Vec<String>,
    blank: usize,
    hw_cache_key: Vec<String>,
    hw_cache_ids: Vec<Vec<i64>>,
}

impl GigaCTC {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let mut onnx: Option<std::path::PathBuf> = None;
        for name in ["gigaam_ctc_q.onnx", "gigaam_ctc.onnx"] {
            let p = dir.join(name);
            if p.is_file() {
                onnx = Some(p);
                break;
            }
        }
        if onnx.is_none() {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    let n = p
                        .file_name()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if p.is_file() && n.ends_with(".onnx") && n.contains("ctc") {
                        onnx = Some(p);
                        break;
                    }
                }
            }
        }
        let onnx = onnx
            .ok_or_else(|| "нет CTC-модели: скачайте её в «Настройки» → «Модели ИИ»".to_string())?;
        // ЗАЧЕМ: INT8-граф с динамической длиной на GPU (DirectML и CUDA) медленнее CPU
        // из-за переключения ядра по длине сегмента и даёт численный дрейф на коротких фразах.
        let sess = OrtSession::load_with_probe(&onnx, 0, SessionProfile::CpuOnly, |session| {
            let in_feat = if session.input_names.iter().any(|n| n == "features") {
                "features".to_string()
            } else {
                session
                    .input_names
                    .first()
                    .cloned()
                    .ok_or_else(|| "у CTC нет входов".to_string())?
            };
            let in_len = ["feature_lengths", "length", "feat_lengths"]
                .iter()
                .find_map(|n| session.input_names.iter().find(|x| x == n).cloned());
            let mut f32_in = HashMap::new();
            f32_in.insert(
                in_feat,
                (
                    vec![1i64, features::GIGAAM_MEL_BANDS as i64, 100],
                    vec![0.0f32; features::GIGAAM_MEL_BANDS * 100],
                ),
            );
            let mut i64_in = HashMap::new();
            if let Some(name) = in_len {
                i64_in.insert(name, (vec![1i64], vec![100i64]));
            }
            let out = session.run_mixed(f32_in, i64_in)?;
            if !outputs_are_finite(&out) {
                return Err("gigaam_ctc probe: нечисловой выход".to_string());
            }
            Ok(())
        })?;
        let in_feat = if sess.input_names.iter().any(|n| n == "features") {
            "features".to_string()
        } else {
            sess.input_names
                .first()
                .cloned()
                .ok_or_else(|| "у CTC нет входов".to_string())?
        };
        let in_len = ["feature_lengths", "length", "feat_lengths"]
            .iter()
            .find_map(|n| sess.input_names.iter().find(|x| x == n).cloned());
        let out_lp = if sess.output_names.iter().any(|n| n == "log_probs") {
            "log_probs".to_string()
        } else {
            sess.output_names
                .first()
                .cloned()
                .ok_or_else(|| "у CTC нет выходов".to_string())?
        };
        let vocab_text =
            std::fs::read_to_string(dir.join("gigaam_ctc_vocab.txt")).map_err(|e| e.to_string())?;
        let mut pairs: Vec<(usize, String)> = Vec::new();
        for line in vocab_text.lines() {
            let mut it = line.split_whitespace();
            if let (Some(piece), Some(id_s)) = (it.next(), it.next()) {
                if let Ok(id) = id_s.parse::<usize>() {
                    pairs.push((id, piece.to_string()));
                }
            }
        }
        if pairs.is_empty() {
            return Err("пустой словарь CTC".to_string());
        }
        let size = pairs.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
        let mut vocab = vec![String::new(); size];
        for (id, piece) in pairs {
            vocab[id] = piece;
        }
        let blank = vocab
            .iter()
            .rposition(|p| p.contains("blk"))
            .unwrap_or_else(|| vocab.len().saturating_sub(1));
        Ok(Self {
            sess,
            in_feat,
            in_len,
            out_lp,
            vocab,
            blank,
            hw_cache_key: Vec::new(),
            hw_cache_ids: Vec::new(),
        })
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.sess.provider()
    }

    pub fn transcribe_segment(
        &mut self,
        samples: &[f32],
        hotwords: &[String],
    ) -> Result<CtcHypothesis, String> {
        self.transcribe_segment_timed(samples, hotwords)
            .map(|(hypothesis, _)| hypothesis)
    }

    pub fn transcribe_segment_timed(
        &mut self,
        samples: &[f32],
        hotwords: &[String],
    ) -> Result<TimedCtcHypothesis, String> {
        if samples.len() < 1600 {
            return Ok((
                CtcHypothesis {
                    text: String::new(),
                    words: Vec::new(),
                },
                Vec::new(),
            ));
        }
        let mel = super::features::nemo_mel(samples);
        self.transcribe_mel_timed(&mel, hotwords)
    }

    pub fn transcribe_mel_timed(
        &mut self,
        mel: &[Vec<f32>],
        hotwords: &[String],
    ) -> Result<TimedCtcHypothesis, String> {
        if mel.is_empty() {
            return Ok((
                CtcHypothesis {
                    text: String::new(),
                    words: Vec::new(),
                },
                Vec::new(),
            ));
        }
        features::ensure_mel_width(mel, features::GIGAAM_MEL_BANDS)?;
        let tf = mel.len();
        let mut flat = Vec::with_capacity(features::GIGAAM_MEL_BANDS * tf);
        for c in 0..features::GIGAAM_MEL_BANDS {
            for row in mel.iter() {
                flat.push(row[c]);
            }
        }
        let mut f32_in = HashMap::new();
        f32_in.insert(self.in_feat.clone(), (vec![1i64, 64, tf as i64], flat));
        let mut i64_in = HashMap::new();
        if let Some(name) = &self.in_len {
            i64_in.insert(name.clone(), (vec![1i64], vec![tf as i64]));
        }
        let out = self.sess.run_mixed(f32_in, i64_in)?;
        let (shape, data) = out
            .get(&self.out_lp)
            .ok_or_else(|| "нет выхода CTC".to_string())?;
        let v = self.vocab.len();
        let frames: Vec<Vec<f32>> = if shape.len() == 3 && shape[0] == 1 && shape[2] as usize == v {
            let te = shape[1] as usize;
            (0..te).map(|t| data[t * v..(t + 1) * v].to_vec()).collect()
        } else if shape.len() == 2 && shape[1] as usize == v {
            let te = shape[0] as usize;
            (0..te).map(|t| data[t * v..(t + 1) * v].to_vec()).collect()
        } else if shape.len() == 3 && shape[0] == 1 && shape[1] as usize == v {
            let te = shape[2] as usize;
            (0..te)
                .map(|t| (0..v).map(|k| data[k * te + t]).collect())
                .collect()
        } else {
            return Err(format!("странный выход CTC: {shape:?}"));
        };
        if frames.is_empty() {
            return Ok((
                CtcHypothesis {
                    text: String::new(),
                    words: Vec::new(),
                },
                Vec::new(),
            ));
        }
        if hotwords != self.hw_cache_key.as_slice() {
            let mut hw_ids = Vec::new();
            let mut hw_skipped = Vec::new();
            for e in parse_hotwords(hotwords) {
                let mut variants = tokenize_term(&e.boost, &self.vocab);
                if variants.is_empty() {
                    hw_skipped.push(e.boost.clone());
                } else {
                    hw_ids.append(&mut variants);
                }
            }
            if !hw_skipped.is_empty() {
                // Сами термины — пользовательские данные: логируем только счётчик.
                log::warn!(
                    target: "asr",
                    skipped = hw_skipped.len(),
                    applied = hw_ids.len();
                    "ev=asr.ctc.hotwords_skipped"
                );
            }
            self.hw_cache_key = hotwords.to_vec();
            self.hw_cache_ids = hw_ids;
        }
        let (hypothesis, spans) = decode_ctc_beam_timed(
            &frames,
            &self.vocab,
            self.blank,
            BEAM_WIDTH,
            &self.hw_cache_ids,
            hotword_bonus(),
        );
        let duration_sec = mel.len() as f64 * 160.0 / 16000.0;
        let frame_sec = duration_sec / frames.len() as f64;
        let times = spans
            .into_iter()
            .map(|span| {
                span.map(|(start, end)| {
                    (
                        (start as f64 * frame_sec).min(duration_sec),
                        ((end + 1) as f64 * frame_sec).min(duration_sec),
                    )
                })
            })
            .collect();
        Ok((hypothesis, times))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonus_override_accepts_only_bounded_values() {
        assert_eq!(parse_bonus("2.5"), Some(2.5));
        assert_eq!(parse_bonus(" 0 "), Some(0.0));
        assert_eq!(parse_bonus("10"), Some(10.0));
        assert_eq!(parse_bonus("10.1"), None);
        assert_eq!(parse_bonus("-1"), None);
        assert_eq!(parse_bonus("нет"), None);
    }

    fn vocab4() -> Vec<String> {
        vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "▁а".to_string(),
            "▁б".to_string(),
            "<blk>".to_string(),
        ]
    }

    fn lp(row: &[f32]) -> Vec<f32> {
        row.to_vec()
    }

    #[test]
    fn greedy_collapses_repeats_and_blank() {
        let v = vocab4();
        let frames = vec![
            lp(&[0.0, 0.0, 5.0, 0.0, 0.0]),
            lp(&[0.0, 0.0, 5.0, 0.0, 0.0]),
            lp(&[0.0, 0.0, 0.0, 0.0, 5.0]),
            lp(&[0.0, 0.0, 0.0, 5.0, 0.0]),
        ];
        let h = decode_ctc_greedy(&frames, &v, 4);
        assert_eq!(h.text, "а б");
        assert_eq!(h.words.len(), 2);
        assert!(h.words.iter().all(|w| (0.0..=1.0).contains(&w.confidence)));
    }

    #[test]
    fn probability_rows_are_converted_to_log_probs() {
        let v = vocab4();
        // Вход — softmax-вероятности, а не лог-вероятности: без ln уверенность
        // вышла бы exp(0.9) ≈ 2.46 и выпала бы из диапазона [0, 1].
        let frames = vec![
            vec![0.02, 0.03, 0.9, 0.03, 0.02],
            vec![0.02, 0.03, 0.03, 0.02, 0.9],
        ];
        let h = decode_ctc_greedy(&frames, &v, 4);
        assert_eq!(h.text, "а");
        assert!((h.words[0].confidence - 0.9).abs() < 1e-5);
    }

    #[test]
    fn log_probability_rows_keep_previous_decoding() {
        let row = [
            0.1f32.ln(),
            0.2f32.ln(),
            0.6f32.ln(),
            0.05f32.ln(),
            0.05f32.ln(),
        ];
        for (actual, expected) in to_log_row(&row).into_iter().zip(row) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn negative_raw_logits_are_normalized() {
        // Все логиты < 0 — это ещё не лог-вероятности: сумма exp должна быть 1.
        let out = to_log_row(&[-3.0, -1.0, -2.0]);
        let sum: f32 = out.iter().map(|x| x.exp()).sum();
        assert!((sum - 1.0).abs() < 1e-6, "сумма {sum}");
        assert!(out[1] > out[2] && out[2] > out[0]);
    }

    #[test]
    fn hotword_does_not_match_prefix_of_longer_word() {
        let v = vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "▁ит".to_string(),
            "о".to_string(),
            "г".to_string(),
            "<blk>".to_string(),
        ];
        assert!(contains_seq(&[2], &[2], &v));
        assert!(contains_seq(&[2, 0, 3], &[2], &v));
        assert!(!contains_seq(&[2, 3, 4], &[2], &v));
        assert!(!contains_seq(&[1, 2, 3, 4], &[1, 2, 3], &v));
    }

    #[test]
    fn greedy_words_carry_frame_spans() {
        let v = vocab4();
        let frames = vec![
            lp(&[0.0, 0.0, 5.0, 0.0, 0.0]),
            lp(&[0.0, 0.0, 0.0, 0.0, 5.0]),
            lp(&[0.0, 0.0, 0.0, 5.0, 0.0]),
        ];
        let (hypothesis, spans) = decode_ctc_greedy_timed(&frames, &v, 4);
        assert_eq!(hypothesis.text, "а б");
        assert_eq!(spans, vec![Some((0, 0)), Some((2, 2))]);
    }

    #[test]
    fn timed_alignment_propagates_word_spans() {
        let ctc = CtcHypothesis {
            text: "сбер банк".to_string(),
            words: vec![
                CtcWord {
                    text: "сбер".to_string(),
                    confidence: 0.9,
                },
                CtcWord {
                    text: "банк".to_string(),
                    confidence: 0.8,
                },
            ],
        };
        let times = [Some((0.5, 0.8)), Some((0.9, 1.2))];
        let aligned = align_to_reference_timed(
            "сбир банк",
            &ctc,
            &times,
            &["Сбер".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned[0].start_sec, 0.5);
        assert_eq!(aligned[0].end_sec, 0.8);
        assert_eq!(aligned[1].start_sec, 0.9);
        assert_eq!(aligned[1].end_sec, 1.2);
    }

    #[test]
    fn beam_bonus_prefers_hotword() {
        let v = vocab4();
        // Кадр 2: «а» и «б» почти равны — без бонуса побеждает «а».
        let frames = vec![
            lp(&[0.0, 0.0, 3.0, 2.9, 0.0]),
            lp(&[0.0, 0.0, 0.0, 0.0, 5.0]),
        ];
        let plain = decode_ctc_beam(&frames, &v, 4, 4, &[], 0.0);
        assert_eq!(plain.text, "а");
        let hw = tokenize_term("б", &v);
        assert!(!hw.is_empty());
        let boosted = decode_ctc_beam(&frames, &v, 4, 4, &hw, 5.0);
        assert_eq!(boosted.text, "б");
    }

    #[test]
    fn tokenize_falls_back_to_separator_and_chars() {
        // Вокаб без ▁-куска на старте слова: оба варианта дают разделитель + буквы.
        let v = vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "м".to_string(),
            "е".to_string(),
            "<blk>".to_string(),
        ];
        let vars = tokenize_term("ме", &v);
        assert_eq!(vars, vec![vec![1, 2, 3]]);
        // Нет ни кусков, ни букв — термин без вариантов, бонуса не будет.
        let vars = tokenize_term("ме", &["<unk>".to_string(), "▁".to_string()]);
        assert!(vars.is_empty());
    }

    #[test]
    fn tokenize_prefers_longest_piece_but_keeps_char_variant() {
        let v = vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "▁на".to_string(),
            "н".to_string(),
            "а".to_string(),
            "д".to_string(),
            "о".to_string(),
            "до".to_string(),
            "<blk>".to_string(),
        ];
        let vars = tokenize_term("надо", &v);
        assert!(vars.contains(&vec![2, 7]));
        assert!(vars.contains(&vec![1, 3, 4, 5, 6]));
    }

    #[test]
    fn align_corrects_and_flags() {
        let ctc = CtcHypothesis {
            text: "сбер банк".to_string(),
            words: vec![
                CtcWord {
                    text: "сбер".to_string(),
                    confidence: 0.9,
                },
                CtcWord {
                    text: "банк".to_string(),
                    confidence: 0.2,
                },
            ],
        };
        let aligned = align_to_reference(
            "сбир банк",
            &ctc,
            &["Сбер".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned.len(), 2);
        assert_eq!(aligned[0].text, "Сбер");
        assert!(!aligned[0].needs_review);
        assert!(aligned[1].needs_review);
    }

    #[test]
    fn align_rejects_unrelated_dict_word_and_low_confidence() {
        let ctc = CtcHypothesis {
            text: "объект".to_string(),
            words: vec![CtcWord {
                text: "объект".to_string(),
                confidence: 0.9,
            }],
        };
        let aligned = align_to_reference(
            "проект готов",
            &ctc,
            &["объект".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned[0].text, "проект");
        let ctc = CtcHypothesis {
            text: "сбер".to_string(),
            words: vec![CtcWord {
                text: "сбер".to_string(),
                confidence: 0.2,
            }],
        };
        let aligned = align_to_reference(
            "сбир",
            &ctc,
            &["Сбер".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned[0].text, "сбир");
        assert!(aligned[0].needs_review);
    }

    #[test]
    fn align_applies_heard_to_correct_pairs() {
        let ctc = CtcHypothesis {
            text: "секс баг".to_string(),
            words: vec![
                CtcWord {
                    text: "секс".to_string(),
                    confidence: 0.4,
                },
                CtcWord {
                    text: "баг".to_string(),
                    confidence: 0.3,
                },
            ],
        };
        let aligned = align_to_reference(
            "секс-баг вылез",
            &ctc,
            &["секс-баг => фикс багов".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned.len(), 3);
        assert_eq!(aligned[0].text, "фикс");
        assert_eq!(aligned[1].text, "багов");
        assert!(!aligned[0].needs_review);
        assert!(!aligned[1].needs_review);
    }

    #[test]
    fn pair_does_not_rewrite_canonical_term() {
        let ctc = CtcHypothesis {
            text: String::new(),
            words: Vec::new(),
        };
        let aligned = align_to_reference(
            "опен код хаб",
            &ctc,
            &[
                "опен код => OpenCode".to_string(),
                "опен код хаб".to_string(),
            ],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(
            aligned
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            ["опен", "код", "хаб"]
        );
    }

    #[test]
    fn beam_trie_keeps_multitoken_hotword_alive_before_completion() {
        let v = vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "▁у".to_string(),
            "г".to_string(),
            "л".to_string(),
            "▁л".to_string(),
            "<blk>".to_string(),
        ];
        // На кадре 0 дистрактор `▁л` (3.6) сильнее `▁у` (3.0): при beam_width=1 без trie
        // префикс `▁у` отбрасывается сразу и до окончания `гл` не доживает.
        let frames = vec![
            lp(&[0.0, 0.0, 3.0, 0.0, 0.0, 3.6, 0.0]),
            lp(&[0.0, 0.0, 0.0, 5.0, 0.0, 0.0, 4.9]),
            lp(&[0.0, 0.0, 0.0, 0.0, 5.0, 0.0, 4.9]),
        ];
        let plain = decode_ctc_beam(&frames, &v, 6, 1, &[], 0.0);
        assert_eq!(plain.text, "лгл");
        let hw = tokenize_term("угл", &v);
        assert!(hw.contains(&vec![2, 3, 4]));
        let boosted = decode_ctc_beam(&frames, &v, 6, 1, &hw, 3.0);
        assert_eq!(boosted.text, "угл");
    }

    #[test]
    fn beam_trie_does_not_force_prefix_of_longer_word() {
        let v = vec![
            "<unk>".to_string(),
            "▁".to_string(),
            "▁ит".to_string(),
            "о".to_string(),
            "г".to_string(),
            "<blk>".to_string(),
        ];
        let frames = vec![
            lp(&[0.0, 0.0, 5.0, 0.0, 0.0, 0.0]),
            lp(&[0.0, 0.0, 0.0, 5.0, 0.0, 0.0]),
            lp(&[0.0, 0.0, 0.0, 0.0, 5.0, 0.0]),
        ];
        let hw = tokenize_term("ит", &v);
        let res = decode_ctc_beam(&frames, &v, 5, 4, &hw, 3.0);
        assert_eq!(res.text, "итог");
    }

    #[test]
    fn align_allows_single_edit_for_two_char_abbreviation() {
        let ctc = CtcHypothesis {
            text: "ск рф".to_string(),
            words: vec![
                CtcWord {
                    text: "ск".to_string(),
                    confidence: 0.9,
                },
                CtcWord {
                    text: "рф".to_string(),
                    confidence: 0.9,
                },
            ],
        };
        let aligned = align_to_reference(
            "ук рф",
            &ctc,
            &["СК РФ".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(aligned[0].text, "СК");
        assert_eq!(aligned[1].text, "рф");
        let rejected = align_to_reference(
            "на рф",
            &ctc,
            &["СК РФ".to_string()],
            DEFAULT_CONFIDENCE_THRESHOLD,
        );
        assert_eq!(rejected[0].text, "на");
    }
}
