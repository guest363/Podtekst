#!/usr/bin/env python3
"""Детерминированная сверка metrics.csv с baseline.json."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from pathlib import Path

FIELDS = ("speakers_ref", "speakers_hyp", "WER", "CER", "DER", "SCA", "cpWER", "attribution")
EXACT = {"speakers_ref", "speakers_hyp"}
LOWER = {"WER", "CER", "DER", "cpWER"}
HIGHER = {"SCA", "attribution"}
KEEP = ("language", "dataset", "subset", *FIELDS)
TOLERANCE = 1e-9


def _number(value: object) -> float | None:
    text = str(value).strip() if value is not None else ""
    if not text or text.lower() == "nan":
        return None
    return float(text)


def _load_rows(paths: list[Path]) -> dict[str, dict]:
    rows: dict[str, dict] = {}
    for path in paths:
        metrics = path / "metrics.csv" if path.is_dir() else path
        with metrics.open(newline="", encoding="utf-8") as file:
            for row in csv.DictReader(file):
                file_id = row["file_id"]
                if file_id in rows:
                    if _record(row) != _record(rows[file_id]):
                        raise ValueError(f"противоречивый повтор file_id: {file_id}")
                    continue
                if row.get("exit_code") not in ("0", 0, None, "") or row.get("error"):
                    raise ValueError(f"{file_id}: прогон завершился ошибкой")
                rows[file_id] = row
    return rows


def _record(row: dict) -> dict:
    record = {}
    for key in KEEP:
        value = row.get(key, "")
        if key in EXACT:
            record[key] = int(value) if str(value).strip() else None
        elif key in LOWER | HIGHER:
            record[key] = _number(value)
        else:
            record[key] = value
    return record


def _compare(
    expected: dict[str, dict], actual: dict[str, dict], allow_extra: bool = False
) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    changes: list[str] = []
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    errors.extend(f"нет файла в прогоне: {file_id}" for file_id in missing)
    if not allow_extra:
        errors.extend(f"лишний файл в прогоне: {file_id}" for file_id in extra)
    for file_id in sorted(set(expected) & set(actual)):
        base = expected[file_id]
        row = actual[file_id]
        language = base.get("language") or row.get("language") or ""
        for field in FIELDS:
            if field not in base:
                continue
            left = _number(base[field])
            right = _number(row.get(field))
            if left is None and right is None:
                continue
            if left is None or right is None:
                errors.append(f"{file_id}: {field} отсутствует")
                continue
            if field in EXACT:
                if int(left) != int(right):
                    message = f"{file_id}: {field} {int(left)} -> {int(right)}"
                    changes.append(message)
                    if language == "ru":
                        errors.append(message)
                continue
            if math.isclose(left, right, abs_tol=TOLERANCE, rel_tol=0.0):
                continue
            worse = right > left if field in LOWER else right < left
            label = "ухудшение" if worse else "улучшение"
            message = f"{file_id}: {field} {left:.6f} -> {right:.6f} ({label})"
            changes.append(message)
            if language == "ru" and worse:
                errors.append(message)
    return errors, changes


def _json_safe(value):
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, dict):
        return {key: _json_safe(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_json_safe(item) for item in value]
    return value


def _update(path: Path, suite: str, actual: dict[str, dict], baseline: dict) -> None:
    records = {file_id: _record(row) for file_id, row in actual.items()}
    baseline.setdefault("suites", {})[suite] = {"files": len(records), "records": records}
    if suite == "small":
        baseline["metrics"] = {
            file_id: {"DER": float(record["DER"]), "speakers_hyp": int(record["speakers_hyp"])}
            for file_id, record in records.items()
        }
    else:
        from report import aggregate_groups

        baseline["groups"] = aggregate_groups(list(actual.values()))
    path.write_text(
        json.dumps(_json_safe(baseline), indent=2, ensure_ascii=False, allow_nan=False) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", type=Path, action="append", required=True)
    parser.add_argument("--suite", choices=("small", "full"), default="small")
    parser.add_argument("--baseline", type=Path, default=Path(__file__).with_name("baseline.json"))
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args()

    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
    expected = baseline["suites"][args.suite]["records"]
    actual = _load_rows(args.run)
    errors, changes = _compare(expected, actual, allow_extra=args.update)
    for change in changes:
        print(change)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    if changes and not args.update:
        print("baseline отличается; обновите его явно: --update", file=sys.stderr)
        return 1
    if args.update:
        _update(args.baseline, args.suite, actual, baseline)
        print(f"baseline обновлён: {args.baseline} ({args.suite})")
    else:
        print(f"OK: {len(actual)} файлов совпали с baseline")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
