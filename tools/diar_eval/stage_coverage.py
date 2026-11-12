#!/usr/bin/env python3
"""Сравнить покрытие эталона сохранёнными интервалами этапов; это не DER."""

import argparse
import hashlib
import json
import math
from pathlib import Path


def union(intervals):
    result = []
    for start, end in sorted(intervals):
        if not (math.isfinite(start) and math.isfinite(end) and 0 <= start < end):
            raise ValueError(f"некорректный интервал: {start}, {end}")
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(result[-1][1], end))
        else:
            result.append((start, end))
    return result


def intersection(left, right):
    result = []
    i = j = 0
    while i < len(left) and j < len(right):
        start = max(left[i][0], right[j][0])
        end = min(left[i][1], right[j][1])
        if start < end:
            result.append((start, end))
        if left[i][1] < right[j][1]:
            i += 1
        else:
            j += 1
    return result


def duration(intervals):
    return sum(end - start for start, end in intervals)


def difference(left, right):
    result = []
    index = 0
    for start, end in left:
        while index < len(right) and right[index][1] <= start:
            index += 1
        cursor = start
        other = index
        while other < len(right) and right[other][0] < end:
            if cursor < right[other][0]:
                result.append((cursor, min(end, right[other][0])))
            cursor = max(cursor, right[other][1])
            if cursor >= end:
                break
            other += 1
        if cursor < end:
            result.append((cursor, end))
        index = other
    return result


def read_reference(path, uri=None, channel="1"):
    records = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split()
        if len(fields) < 8 or fields[0] != "SPEAKER":
            raise ValueError(f"{path}:{line_number}: некорректная строка RTTM")
        start, length = float(fields[3]), float(fields[4])
        union([(start, start + length)])
        records.append((fields[1], fields[2], start, start + length))
    uris = {item[0] for item in records}
    if uri is None:
        if len(uris) != 1:
            raise ValueError("RTTM должен содержать один URI; выберите URI через --uri")
        uri = next(iter(uris))
    selected = [(start, end) for name, ch, start, end in records if name == uri and ch == channel]
    if not selected:
        raise ValueError(f"в RTTM нет интервалов для URI {uri} и канала {channel}")
    return union(selected), uri


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def pairs(items):
    return union([(float(start), float(end)) for start, end in items])


def stage_report(reference, hypothesis, previous=None):
    ref_covered = intersection(reference, hypothesis)
    covered = duration(ref_covered)
    report = {
        "reference_union_sec": duration(reference),
        "reference_union_covered_sec": covered,
        "hypothesis_union_sec": duration(hypothesis),
        "reference_union_uncovered_sec": duration(reference) - covered,
        "reference_union_uncovered_intervals": difference(reference, hypothesis),
        "extra_union_sec": duration(hypothesis) - covered,
    }
    if previous is not None:
        previous_covered = intersection(reference, previous)
        shared = duration(intersection(previous_covered, ref_covered))
        report["lost_reference_since_previous_sec"] = duration(previous_covered) - shared
        report["gained_reference_since_previous_sec"] = covered - shared
    return report


def analyze(run, reference_path, audio, channel=0, uri=None, rttm_channel="1"):
    expected = read_json(run / "manifest.json").get("input", {}).get("sha256")
    if not isinstance(expected, str):
        raise ValueError("нет manifest input.sha256: нужен новый dev-run с проверяемым входом")
    digest = hashlib.sha256()
    with audio.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    if not isinstance(expected, str) or digest.hexdigest() != expected.lower():
        raise ValueError("SHA-256 аудио не совпадает с manifest input.sha256")
    reference, uri = read_reference(reference_path, uri, rttm_channel)
    prefix = f"ch{channel}"
    vad_dir = run / "vad"
    diar_dir = run / "diar"
    combined = read_json(vad_dir / f"{prefix}.segments.json")
    stages = [("combined", pairs(combined["raw"])), ("grouped", pairs(combined["grouped"]))]
    for name in ("asr", "pre-cluster", "acoustic", "document"):
        path = diar_dir / f"{prefix}.{name}.json"
        if not path.exists() and name == "pre-cluster":
            stages.append((name, None))
            continue
        data = read_json(path)
        stages.append((name, union([(float(s["start"]), float(s["end"])) for s in data["segments"]])))
    ordered = {}
    previous = None
    previous_name = None
    for name, intervals in stages:
        if intervals is None:
            ordered[name] = None
            continue
        ordered[name] = stage_report(reference, intervals, previous)
        ordered[name]["previous_stage"] = previous_name
        previous, previous_name = intervals, name
    branches = {}
    for name in ("raw", "frontend"):
        path = vad_dir / f"{prefix}.{name}.segments.json"
        if path.exists():
            data = read_json(path)
            branches[name] = stage_report(reference, pairs(data["segments"]))
            branches[name]["reused_from"] = data.get("reused_from")
        else:
            branches[name] = None
    return {
        "metric": "speech union coverage; not DER and not a replacement reference",
        "audio_sha256": digest.hexdigest(),
        "reference_uri": uri,
        "reference_channel": rttm_channel,
        "channel": channel,
        "branches": branches,
        "stages": ordered,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--channel", type=int, default=0)
    parser.add_argument("--uri")
    parser.add_argument("--rttm-channel", default="1")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.channel < 0:
        parser.error("--channel должен быть неотрицательным")
    try:
        result = analyze(args.run, args.reference, args.audio, args.channel, args.uri, args.rttm_channel)
        args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    except (OSError, ValueError, KeyError, TypeError) as exc:
        parser.exit(2, f"ошибка: {exc}\n")


if __name__ == "__main__":
    main()
