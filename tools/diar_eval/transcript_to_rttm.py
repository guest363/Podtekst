#!/usr/bin/env python3
"""Конвертер `.transcript.json` Podtekst в RTTM для оценки диаризации."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path


@dataclass
class RttmConversion:
    lines: list[str] = field(default_factory=list)
    speakers: dict[str, str] = field(default_factory=dict)
    skipped: int = 0


def default_output(transcript: Path) -> Path:
    name = transcript.name
    if name.endswith(".transcript.json"):
        name = name[: -len(".transcript.json")] + ".rttm"
    else:
        name = transcript.stem + ".rttm"
    return transcript.with_name(name)


def default_file_id(transcript: Path) -> str:
    name = transcript.name
    if name.endswith(".transcript.json"):
        return name[: -len(".transcript.json")]
    return transcript.stem


def _safe_file_id(value: str) -> str:
    # RTTM не допускает пробелы в file id; pyannote сопоставляет файлы по этому полю.
    cleaned = re.sub(r"\s+", "_", str(value).strip())
    return cleaned or "audio"


def _read_utterances(path: Path) -> list[dict]:
    doc = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(doc, list):
        utterances = doc
    elif isinstance(doc, dict):
        utterances = doc.get("utterances")
    else:
        raise ValueError(f"{path}: ожидался объект или массив")
    if not isinstance(utterances, list):
        raise ValueError(f"{path}: нет массива utterances")
    return [item for item in utterances if isinstance(item, dict)]


def _number(value: object) -> float | None:
    try:
        parsed = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return parsed if math.isfinite(parsed) else None


def _speaker_labels(rows: list[tuple[float, float, str]]) -> dict[str, str]:
    totals: dict[str, float] = {}
    first_start: dict[str, float] = {}
    for start, end, speaker in rows:
        totals[speaker] = totals.get(speaker, 0.0) + (end - start)
        first_start[speaker] = min(first_start.get(speaker, start), start)
    ordered = sorted(totals, key=lambda name: (-totals[name], first_start[name], name))
    return {name: f"SPEAKER_{index:02d}" for index, name in enumerate(ordered, 1)}


def convert_transcript(doc: object, file_id: str) -> RttmConversion:
    if isinstance(doc, list):
        utterances = doc
    elif isinstance(doc, dict):
        utterances = doc.get("utterances")
    else:
        raise ValueError("ожидался объект или массив utterances")
    if not isinstance(utterances, list):
        raise ValueError("нет массива utterances")

    rows: list[tuple[float, float, str]] = []
    skipped = 0
    for item in utterances:
        if not isinstance(item, dict):
            skipped += 1
            continue
        start = _number(item.get("start"))
        end = _number(item.get("end"))
        if start is None or end is None:
            skipped += 1
            continue
        start = max(0.0, start)
        if end <= start:
            skipped += 1
            continue
        rows.append((start, end, str(item.get("speaker", ""))))

    result = RttmConversion(skipped=skipped)
    result.speakers = _speaker_labels(rows)
    uri = _safe_file_id(file_id)
    for start, end, speaker in sorted(rows, key=lambda row: (row[0], row[1])):
        label = result.speakers[speaker]
        result.lines.append(
            f"SPEAKER {uri} 1 {start:.3f} {end - start:.3f} <NA> <NA> {label} <NA> <NA>"
        )
    return result


def write_rttm(path: Path, lines: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")


def speaker_labels(utterances: list[dict]) -> dict[str, str]:
    rows: list[tuple[float, float, str]] = []
    for item in utterances:
        if not isinstance(item, dict):
            continue
        start = _number(item.get("start"))
        end = _number(item.get("end"))
        if start is None or end is None:
            continue
        start = max(0.0, start)
        if end > start:
            rows.append((start, end, str(item.get("speaker", ""))))
    return _speaker_labels(rows)


def to_rttm(utterances: list[dict], file_id: str) -> list[str]:
    return convert_transcript(utterances, file_id).lines


def _self_test() -> int:
    doc = {
        "utterances": [
            {"start": 0.0, "end": 2.0, "speaker": "B"},
            {"start": 2.0, "end": 7.0, "speaker": "A"},
            {"start": 7.0, "end": 8.0, "speaker": "B"},
            {"start": 8.0, "end": 8.0, "speaker": "A"},
            {"start": "bad", "end": 9.0, "speaker": "A"},
            {"start": 9.0, "end": 10.0, "speaker": ""},
        ]
    }
    result = convert_transcript(doc, "meeting one")
    assert result.skipped == 2, result.skipped
    assert result.speakers["A"] == "SPEAKER_01", result.speakers
    assert result.speakers["B"] == "SPEAKER_02", result.speakers
    assert result.speakers[""] == "SPEAKER_03", result.speakers
    assert len(result.lines) == 4, result.lines
    fields = result.lines[0].split()
    assert len(fields) == 10, fields
    assert fields[0] == "SPEAKER"
    assert fields[1] == "meeting_one", fields[1]
    assert fields[2] == "1"
    assert fields[3] == "0.000"
    assert fields[4] == "2.000"
    assert fields[7] == "SPEAKER_02"
    assert result.lines[1].split()[7] == "SPEAKER_01"

    with tempfile.TemporaryDirectory() as tmp:
        transcript = Path(tmp) / "sample.transcript.json"
        transcript.write_text(json.dumps(doc), encoding="utf-8")
        assert default_output(transcript) == Path(tmp) / "sample.rttm"
        assert default_file_id(transcript) == "sample"
        output = default_output(transcript)
        write_rttm(output, result.lines)
        assert len(output.read_text(encoding="utf-8").splitlines()) == 4

    empty = convert_transcript({"utterances": []}, "empty")
    assert empty.lines == [] and empty.speakers == {}

    print("self-test ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcript", nargs="?", type=Path, help="входной .transcript.json")
    parser.add_argument("output", nargs="?", type=Path, help="выходной RTTM")
    parser.add_argument("--id", help="RTTM file id (по умолчанию имя транскрипта)")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    if args.transcript is None:
        parser.error("нужен путь к .transcript.json")

    try:
        utterances = _read_utterances(args.transcript)
        result = convert_transcript(utterances, args.id or default_file_id(args.transcript))
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2

    output = args.output or default_output(args.transcript)
    if output.resolve() == args.transcript.resolve():
        print("ошибка: выходной RTTM совпадает с входным JSON", file=sys.stderr)
        return 2
    try:
        write_rttm(output, result.lines)
    except OSError as exc:
        print(f"ошибка записи {output}: {exc}", file=sys.stderr)
        return 2

    print(
        f"{output}: {len(result.lines)} сегментов, "
        f"{len(result.speakers)} спикеров, пропущено {result.skipped}"
    )
    if result.skipped:
        print(f"предупреждение: пропущено сегментов {result.skipped}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
