use std::collections::{BTreeMap, HashMap};

use super::{
    median, percentile, vowels, Monologue, Navigation, Span, SpeakerMetrics, Totals, Turn,
};

const MONOLOGUE_GAP_SEC: f64 = 1.5;
const INTERRUPTION_MIN_SEC: f64 = 0.3;
const INTERRUPTION_YIELD_SEC: f64 = 1.0;
const LATENCY_MAX_SEC: f64 = 3.0;
const LONG_PAUSE_SEC: f64 = 5.0;
const MONOLOGUE_LONG_SEC: f64 = 30.0;
const SCAN_BACK: usize = 64;
const PAUSE_LIST_CAP: usize = 50;

pub struct Interaction {
    pub speakers: Vec<SpeakerMetrics>,
    pub totals: Totals,
    pub navigation: Navigation,
    pub speech_sec: f64,
    pub duration_sec: f64,
    pub warnings: Vec<String>,
}

#[derive(Default)]
struct Acc {
    turns: Vec<f64>,
    words: usize,
    vowels: usize,
    fillers: usize,
    latencies: Vec<f64>,
    channels: BTreeMap<usize, usize>,
    made: usize,
    successful: usize,
    suffered: usize,
}

pub fn compute(turns: &[Turn], duration_sec: f64, names: &HashMap<String, String>) -> Interaction {
    let mut sorted: Vec<&Turn> = turns.iter().filter(|t| t.end > t.start).collect();
    sorted.sort_by(|a, b| a.start.total_cmp(&b.start));

    let merged = merge_intervals(&sorted);
    let speech_sec: f64 = merged.iter().map(|(a, b)| b - a).sum();
    let duration_sec = sorted
        .iter()
        .map(|turn| turn.end)
        .fold(duration_sec.max(0.0), f64::max);
    let (overlap_sec, overlap_by_speaker) = overlap_stats(&sorted);

    let mut acc: HashMap<&str, Acc> = HashMap::new();
    let mut intervals: HashMap<&str, Vec<(f64, f64)>> = HashMap::new();
    for turn in &sorted {
        let entry = acc.entry(turn.speaker.as_str()).or_default();
        entry.turns.push(turn.end - turn.start);
        entry.words += turn
            .text
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count();
        entry.vowels += vowels(&turn.text);
        entry.fillers += count_fillers(&turn.text);
        if let Some(channel) = turn.channel {
            *entry.channels.entry(channel).or_default() += 1;
        }
        intervals
            .entry(turn.speaker.as_str())
            .or_default()
            .push((turn.start, turn.end));
    }
    let speech_by_speaker: HashMap<&str, f64> = intervals
        .into_iter()
        .map(|(speaker, spans)| (speaker, union_len(spans)))
        .collect();
    let sum_speech: f64 = speech_by_speaker.values().sum();

    let mut runs: Vec<(String, f64, f64)> = Vec::new();
    for turn in &sorted {
        match runs.last_mut() {
            Some((speaker, _, end))
                if *speaker == turn.speaker && turn.start - *end < MONOLOGUE_GAP_SEC =>
            {
                *end = (*end).max(turn.end);
            }
            _ => runs.push((turn.speaker.clone(), turn.start, turn.end)),
        }
    }

    let mut first_interruption: Option<Span> = None;
    for (idx, current) in sorted.iter().enumerate() {
        let Some(channel) = current.channel else {
            continue;
        };
        let previous = sorted[..idx]
            .iter()
            .rev()
            .take(SCAN_BACK)
            .filter(|candidate| {
                candidate.speaker != current.speaker
                    && candidate.channel.is_some_and(|c| c != channel)
                    && candidate.end > current.start + INTERRUPTION_MIN_SEC
            })
            .max_by(|a, b| a.end.total_cmp(&b.end));
        let Some(previous) = previous else {
            continue;
        };
        let entry = acc.entry(current.speaker.as_str()).or_default();
        entry.made += 1;
        if previous.end - current.start <= INTERRUPTION_YIELD_SEC
            && current.end - current.start >= INTERRUPTION_YIELD_SEC
        {
            entry.successful += 1;
        }
        acc.entry(previous.speaker.as_str()).or_default().suffered += 1;
        let span = Span {
            start: current.start,
            end: previous.end.min(current.end),
        };
        if first_interruption.is_none_or(|first| span.start < first.start) {
            first_interruption = Some(span);
        }
    }

    for (idx, current) in sorted.iter().enumerate() {
        let previous = sorted[..idx]
            .iter()
            .rev()
            .take(SCAN_BACK)
            .find(|candidate| {
                candidate.speaker != current.speaker && candidate.end <= current.start
            });
        let Some(previous) = previous else {
            continue;
        };
        let gap = current.start - previous.end;
        if (0.0..=LATENCY_MAX_SEC).contains(&gap) {
            acc.entry(current.speaker.as_str())
                .or_default()
                .latencies
                .push(gap * 1000.0);
        }
    }

    let pauses: Vec<Span> = merged
        .windows(2)
        .filter_map(|pair| {
            let gap = pair[1].0 - pair[0].1;
            (gap >= LONG_PAUSE_SEC).then_some(Span {
                start: pair[0].1,
                end: pair[1].0,
            })
        })
        .collect();

    let longest_monologue = runs
        .iter()
        .max_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1)))
        .map(|run| Monologue {
            speaker: run.0.clone(),
            start: run.1,
            end: run.2,
        });

    let navigation = Navigation {
        longest_monologue,
        first_long_pause: pauses.first().copied(),
        first_interruption,
        pauses_over_5s: pauses.iter().copied().take(PAUSE_LIST_CAP).collect(),
    };

    let mut ids: Vec<&str> = acc.keys().copied().collect();
    ids.sort_by(|a, b| {
        speech_by_speaker[b]
            .total_cmp(&speech_by_speaker[a])
            .then_with(|| a.cmp(b))
    });

    let speakers = ids
        .into_iter()
        .map(|id| {
            let entry = &acc[id];
            let (monologue_max_sec, monologues_over_30s) = runs
                .iter()
                .filter(|run| run.0 == id)
                .fold((0.0_f64, 0_usize), |(max, count), run| {
                    let dur = run.2 - run.1;
                    (max.max(dur), count + usize::from(dur >= MONOLOGUE_LONG_SEC))
                });
            let speech_sec = speech_by_speaker[id];
            SpeakerMetrics {
                id: id.to_string(),
                name: names.get(id).cloned().unwrap_or_else(|| id.to_string()),
                channel: entry
                    .channels
                    .iter()
                    .max_by_key(|(_, count)| *count)
                    .map(|(channel, _)| *channel),
                speech_sec,
                share: if sum_speech > 0.0 {
                    speech_sec / sum_speech
                } else {
                    0.0
                },
                turns: entry.turns.len(),
                words: entry.words,
                vowels: entry.vowels,
                fillers: entry.fillers,
                fillers_per_100_words: (entry.words > 0)
                    .then(|| entry.fillers as f64 / entry.words as f64 * 100.0),
                turn_sec_median: median(&entry.turns),
                turn_sec_p90: percentile(&entry.turns, 0.9),
                monologue_max_sec: (monologue_max_sec > 0.0).then_some(monologue_max_sec),
                monologues_over_30s,
                interruptions_made: entry.made,
                interruptions_successful: entry.successful,
                interruptions_suffered: entry.suffered,
                overlap_sec: overlap_by_speaker.get(id).copied().unwrap_or(0.0),
                response_latency_ms_median: median(&entry.latencies),
                response_latency_n: entry.latencies.len(),
                wpm: (speech_sec > 0.0).then(|| entry.words as f64 / speech_sec * 60.0),
                ..SpeakerMetrics::default()
            }
        })
        .collect();

    let turn_durations: Vec<f64> = sorted.iter().map(|t| t.end - t.start).collect();
    let monologue_secs: Vec<f64> = runs.iter().map(|run| run.2 - run.1).collect();
    let switches = sorted
        .windows(2)
        .filter(|pair| pair[0].speaker != pair[1].speaker)
        .count();
    let dead_air_sec: f64 = pauses.iter().map(|pause| pause.end - pause.start).sum();
    let dead_air_max_sec = pauses
        .iter()
        .map(|pause| pause.end - pause.start)
        .reduce(f64::max);
    let global_latencies: Vec<f64> = acc
        .values()
        .flat_map(|entry| entry.latencies.iter().copied())
        .collect();
    let totals = Totals {
        turns: sorted.len(),
        turn_sec_median: median(&turn_durations),
        turn_sec_p90: percentile(&turn_durations, 0.9),
        monologue_sec_median: median(&monologue_secs),
        monologue_sec_p90: percentile(&monologue_secs, 0.9),
        monologues_over_30s: runs
            .iter()
            .filter(|run| run.2 - run.1 >= MONOLOGUE_LONG_SEC)
            .count(),
        interruptions: acc.values().map(|entry| entry.made).sum(),
        interruptions_successful: acc.values().map(|entry| entry.successful).sum(),
        response_latency_ms_median: median(&global_latencies),
        switches_per_min: (speech_sec > 0.0).then(|| switches as f64 / (speech_sec / 60.0)),
        overlap_sec,
        overlap_share: (speech_sec > 0.0).then(|| overlap_sec / speech_sec),
        dead_air_n: pauses.len(),
        dead_air_sec,
        dead_air_max_sec,
        dead_air_share: (duration_sec > 0.0).then(|| dead_air_sec / duration_sec),
        density: (duration_sec > 0.0).then(|| speech_sec / duration_sec),
    };

    let mut warnings = Vec::new();
    let mut channels: Vec<usize> = sorted.iter().filter_map(|turn| turn.channel).collect();
    channels.sort_unstable();
    channels.dedup();
    if channels.len() < 2 {
        warnings.push("mono_no_overlap".to_string());
    }
    if sorted.len() < 10 {
        warnings.push("few_turns".to_string());
    }

    Interaction {
        speakers,
        totals,
        navigation,
        speech_sec,
        duration_sec,
        warnings,
    }
}

fn overlap_stats(turns: &[&Turn]) -> (f64, HashMap<String, f64>) {
    let mut events: Vec<(f64, i8, &str)> = Vec::with_capacity(turns.len() * 2);
    for turn in turns {
        events.push((turn.start, 1, turn.speaker.as_str()));
        events.push((turn.end, -1, turn.speaker.as_str()));
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let mut active: HashMap<&str, usize> = HashMap::new();
    let mut per_speaker: HashMap<&str, f64> = HashMap::new();
    let mut distinct = 0usize;
    let mut total = 0.0;
    let mut last = events.first().map(|event| event.0).unwrap_or(0.0);
    for (time, kind, speaker) in events {
        let dt = (time - last).max(0.0);
        if dt > 0.0 && distinct >= 2 {
            total += dt;
            for (name, count) in &active {
                if *count > 0 {
                    *per_speaker.entry(name).or_default() += dt;
                }
            }
        }
        let count = active.entry(speaker).or_default();
        if kind > 0 {
            if *count == 0 {
                distinct += 1;
            }
            *count += 1;
        } else if *count > 0 {
            *count -= 1;
            if *count == 0 {
                distinct -= 1;
            }
        }
        last = time;
    }
    (
        total,
        per_speaker
            .into_iter()
            .map(|(name, seconds)| (name.to_string(), seconds))
            .collect(),
    )
}

fn count_fillers(text: &str) -> usize {
    const FILLERS: [&str; 4] = ["э", "ээ", "мм", "ну"];
    let tokens: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect();
    let single = tokens
        .iter()
        .filter(|token| FILLERS.contains(&token.as_str()))
        .count();
    let phrases = tokens
        .windows(2)
        .filter(|pair| pair == &["как", "бы"])
        .count();
    single + phrases
}

fn union_len(mut spans: Vec<(f64, f64)>) -> f64 {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for (start, end) in spans {
        match current {
            Some((left, right)) if start <= right => current = Some((left, right.max(end))),
            Some((left, right)) => {
                total += right - left;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((left, right)) = current {
        total += right - left;
    }
    total
}

fn merge_intervals(turns: &[&Turn]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for turn in turns {
        match out.last_mut() {
            Some(last) if turn.start <= last.1 => last.1 = last.1.max(turn.end),
            _ => out.push((turn.start, turn.end)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speaker: &str) -> Turn {
        Turn {
            start,
            end,
            speaker: speaker.to_string(),
            channel: None,
            text: "да".to_string(),
        }
    }

    fn names() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn share_sums_to_one() {
        let turns = [
            turn(0.0, 10.0, "A"),
            turn(10.0, 14.0, "B"),
            turn(14.0, 16.0, "A"),
        ];
        let result = compute(&turns, 16.0, &names());
        let total: f64 = result.speakers.iter().map(|s| s.share).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert_eq!(result.speech_sec, 16.0);
    }

    #[test]
    fn monologue_merges_short_gaps() {
        let turns = [
            turn(0.0, 5.0, "A"),
            turn(5.5, 35.0, "A"),
            turn(35.0, 37.0, "B"),
        ];
        let result = compute(&turns, 37.0, &names());
        assert_eq!(result.speakers[0].monologues_over_30s, 1);
        assert!(result.speakers[0].monologue_max_sec.unwrap() > 34.0);
    }

    #[test]
    fn pause_over_five_seconds_found() {
        let turns = [turn(0.0, 3.0, "A"), turn(9.0, 12.0, "B")];
        let result = compute(&turns, 12.0, &names());
        let pause = result.navigation.first_long_pause.unwrap();
        assert!((pause.start - 3.0).abs() < 1e-9);
        assert!((pause.end - 9.0).abs() < 1e-9);
        assert_eq!(result.totals.dead_air_n, 1);
        assert!((result.totals.dead_air_max_sec.unwrap() - 6.0).abs() < 1e-9);
    }

    #[test]
    fn fillers_counted_per_100_words() {
        let mut turn = turn(0.0, 1.0, "A");
        turn.text = "ну как бы э да".to_string();
        let result = compute(&[turn], 1.0, &names());
        let speaker = &result.speakers[0];
        assert_eq!(speaker.fillers, 3);
        assert!((speaker.fillers_per_100_words.unwrap() - 60.0).abs() < 0.01);
    }

    #[test]
    fn mono_has_no_interruptions() {
        let turns = [
            turn(0.0, 5.0, "A"),
            turn(4.0, 8.0, "B"),
            turn(8.0, 10.0, "A"),
        ];
        let result = compute(&turns, 10.0, &names());
        assert!(result.navigation.first_interruption.is_none());
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning == "mono_no_overlap"));
    }

    #[test]
    fn stereo_interruption_detected() {
        let mut first = turn(0.0, 5.0, "A");
        first.channel = Some(0);
        let mut second = turn(4.0, 8.0, "B");
        second.channel = Some(1);
        let result = compute(&[first, second], 8.0, &names());
        assert!(result.navigation.first_interruption.is_some());
        assert_eq!(result.speakers[1].interruptions_made, 1);
        assert!((result.totals.overlap_sec - 1.0).abs() < 1e-9);
        assert!((result.totals.overlap_share.unwrap() - 0.125).abs() < 1e-9);
    }

    #[test]
    fn single_channel_warns_about_missing_overlap() {
        let mut first = turn(0.0, 5.0, "A");
        first.channel = Some(0);
        let mut second = turn(4.0, 8.0, "B");
        second.channel = Some(0);
        let result = compute(&[first, second], 8.0, &names());
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning == "mono_no_overlap"));
        assert!(result.totals.overlap_sec > 0.0);
    }
}
