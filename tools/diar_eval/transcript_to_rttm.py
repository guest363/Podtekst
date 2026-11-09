#!/usr/bin/env python3
"""Convert Podtekst .transcript.json to RTTM for offline DER evaluation.

Only the standard library is used. Speaker names are remapped to SPEAKER_NN
by total speech time, because RTTM labels must be space-free and the evaluation
only cares about who is who, not the UI name.
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path


def _utterances(path: Path) -> list[dict]:
    doc = json.loads(path.read_text(encoding="utf-8"))
    utterances = doc.get("utterances")
    if not isinstance(utterances, list):
        raise SystemExit(f"{path}: нет массива utterances")
    return [u for u in utterances if isinstance(u, dict)]


def speaker_labels(utterances: list[dict]) -> dict[str, str]:
    totals: dict[str, float] = {}
    first: dict[str, float] = {}
    for u in utterances:
        name = str(u.get("speaker", ""))
        start = float(u.get("start", 0.0))
        end = float(u.get("end", 0.0))
        totals[name] = totals.get(name, 0.0) + max(0.0, end - start)
        first[name] = min(first.get(name, start), start)
    ordered = sorted(totals, key=lambda name: (-totals[name], first[name], name))
    return {name: f"SPEAKER_{index:02d}" for index, name in enumerate(ordered, 1)}


def to_rttm(utterances: list[dict], file_id: str) -> list[str]:
    labels = speaker_labels(utterances)
    lines = []
    for u in sorted(utterances, key=lambda x: (float(x.get("start", 0.0)), float(x.get("end", 0.0)))):
        start = float(u.get("start", 0.0))
        end = float(u.get("end", 0.0))
        if end <= start:
            continue
        speaker = labels[str(u.get("speaker", ""))]
        lines.append(
            f"SPEAKER {file_id} 1 {start:.3f} {end - start:.3f} "
            f"<NA> <NA> {speaker} <NA> <NA>"
        )
    return lines


def _self_test() -> None:
    utterances = [
        {"start": 0.0, "end": 2.5, "speaker": "A"},
        {"start": 2.5, "end": 4.0, "speaker": "B"},
        {"start": 4.0, "end": 5.0, "speaker": "A"},
    ]
    lines = to_rttm(utterances, "fixture")
    assert len(lines) == 3, lines
    first = lines[0].split()
    second = lines[1].split()
    assert first[0] == "SPEAKER" and first[1] == "fixture" and first[2] == "1"
    assert first[3] == "0.000" and first[4] == "2.500"
    assert first[7] == "SPEAKER_01" and second[7] == "SPEAKER_02"
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "t.rttm"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        assert len(path.read_text(encoding="utf-8").splitlines()) == 3
    print("self-test ok", file=sys.stderr)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcript", nargs="?", type=Path)
    parser.add_argument("output", nargs="?", type=Path)
    parser.add_argument("--id", help="RTTM file id (по умолчанию — имя транскрипта)")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        _self_test()
        return
    if args.transcript is None:
        parser.error("нужен путь к .transcript.json")
    utterances = _utterances(args.transcript)
    output = args.output or args.transcript.with_suffix(".rttm")
    file_id = args.id or args.transcript.stem.replace(" ", "_")
    lines = to_rttm(utterances, file_id)
    output.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
    print(f"{output}: {len(lines)} сегментов, {len(set(speaker_labels(utterances).values()))} спикеров")


if __name__ == "__main__":
    main()
