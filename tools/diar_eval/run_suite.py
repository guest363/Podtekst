#!/usr/bin/env python3
"""Запуск small/full suite из tools/diar_eval/suites."""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMPARE = ("language", "dataset", "subset", "speakers_ref", "speakers_hyp",
           "WER", "CER", "DER", "SCA", "cpWER", "attribution")


def _run(entry: dict, root: Path, app: Path, out: Path, timeout: float) -> int:
    command = [
        sys.executable,
        str(HERE / "run_batch.py"),
        "--app", str(app),
        "--manifest", str(root / entry["path"]),
        "--out", str(out / entry["name"]),
        "--timeout", str(timeout),
    ]
    if data_root := entry.get("data_root"):
        command += ["--data-root", str(root / data_root)]
    for dataset in entry.get("datasets", []):
        command += ["--dataset", dataset]
    print("+", " ".join(command), flush=True)
    return subprocess.run(command, check=False).returncode


def _combine(out: Path, names: list[str]) -> Path:
    rows: dict[str, dict] = {}
    fieldnames = None
    for name in names:
        path = out / name / "metrics.csv"
        if not path.is_file():
            continue
        with path.open(newline="", encoding="utf-8") as file:
            reader = csv.DictReader(file)
            if fieldnames is None:
                fieldnames = reader.fieldnames
            for row in reader:
                file_id = row["file_id"]
                if file_id in rows:
                    if any(rows[file_id].get(key) != row.get(key) for key in COMPARE):
                        raise ValueError(f"противоречивый повтор file_id: {file_id}")
                    continue
                rows[file_id] = row
    if fieldnames is None:
        raise ValueError("ни один suite-манифест не дал metrics.csv")
    target = out / "metrics.csv"
    with target.open("w", newline="", encoding="utf-8") as file:
        writer = csv.DictWriter(file, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(sorted(rows.values(), key=lambda row: row["file_id"]))
    return target


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--suite", choices=("small", "full"), required=True)
    parser.add_argument("--root", type=Path, default=Path(os.environ.get("MR_DIAR_BENCH_ROOT", "~/.cache/podtekst-bench")).expanduser())
    parser.add_argument("--app", type=Path, default=Path("src-tauri/target/release/podtekst"))
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=900)
    args = parser.parse_args()

    suite = json.loads((HERE / "suites" / f"{args.suite}.json").read_text(encoding="utf-8"))
    args.out.mkdir(parents=True, exist_ok=True)
    codes = [_run(entry, args.root, args.app.resolve(), args.out.resolve(), args.timeout)
             for entry in suite["manifests"]]
    combined = _combine(args.out, [entry["name"] for entry in suite["manifests"]])
    print(f"suite={args.suite} metrics={combined}")
    return int(any(codes))


if __name__ == "__main__":
    raise SystemExit(main())
