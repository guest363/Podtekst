#!/usr/bin/env python3
"""Пересчитать только cpWER сохранённого прогона без запуска приложения."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path

if __package__:
    from .metrics import CPWER_VERSION, cpwer, load_segments
    from .run_batch import _resolve, _safe_id, load_manifest
    from .transcript_to_rttm import convert_transcript
else:
    from metrics import CPWER_VERSION, cpwer, load_segments
    from run_batch import _resolve, _safe_id, load_manifest
    from transcript_to_rttm import convert_transcript

CP_COLUMNS = ("cpWER", "cpwer_errors", "cpwer_ref_words", "cpwer_version")
PROVENANCE_COLUMNS = (
    "rescore_source_metrics", "rescore_source_manifest", "rescore_source_metrics_sha256",
    "rescore_hypothesis", "rescore_hypothesis_source", "rescore_hypothesis_sha256",
    "rescore_reference", "rescore_reference_source", "rescore_reference_sha256",
    "rescore_geometry_verified", "rescore_utc",
)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _file(base: Path, value: object, label: str) -> Path:
    path = _resolve(base, value)
    if path is None or not path.is_file():
        raise ValueError(f"{label}: файл не найден")
    return path.resolve()


def _reference_words(row: dict[str, str]) -> int:
    try:
        value = float(row["cpwer_ref_words"])
    except (KeyError, ValueError) as exc:
        raise ValueError("нет исходного знаменателя cpwer_ref_words") from exc
    if not math.isfinite(value) or value <= 0 or not value.is_integer():
        raise ValueError("некорректный исходный знаменатель cpwer_ref_words")
    return int(value)


def _verify_geometry(doc: object, rttm: Path) -> None:
    lines = [" ".join(line.split()) for line in rttm.read_text(encoding="utf-8").splitlines() if line.strip()]
    fields = [line.split() for line in lines]
    if not fields or any(len(parts) != 10 or parts[0] != "SPEAKER" for parts in fields):
        raise ValueError("сохранённый RTTM пуст или некорректен")
    uris = {parts[1] for parts in fields}
    if len(uris) != 1:
        raise ValueError("сохранённый RTTM содержит несколько file id")
    converted = convert_transcript(doc, next(iter(uris)))
    if converted.skipped or sorted(converted.lines) != sorted(lines):
        raise ValueError("геометрия текущего транскрипта не совпадает с сохранённым RTTM")


def _hypothesis(row: dict[str, str], run: Path, allow_live: bool) -> tuple[Path, str, bool]:
    if row.get("transcript_snapshot"):
        return _file(run, row["transcript_snapshot"], "снимок транскрипта"), "transcript_snapshot", False
    if row.get("dev_run"):
        directory = _resolve(run, row["dev_run"])
        if directory is None:
            raise ValueError("не задан путь сохранённого dev-run")
        path = directory / "transcript" / "post.json"
        return _file(run, path, "сохранённый dev-транскрипт"), "dev_run", False
    if not allow_live:
        raise ValueError("нет сохранённого транскрипта; текущий файл требует --allow-live-transcripts")
    path = _file(run, row.get("transcript"), "текущий транскрипт")
    return path, "live_geometry_only", True


def rescore(options: argparse.Namespace) -> dict:
    run = Path(options.run).expanduser().resolve()
    source = run / "metrics.csv"
    output = Path(options.out).expanduser().resolve()
    if output.exists():
        raise FileExistsError("выходной CSV уже существует; задайте новый --out")
    manifest = Path(options.manifest).expanduser().resolve()
    data_root = Path(options.data_root).expanduser().resolve() if options.data_root else manifest.parent
    records: dict[str, dict] = {}
    for record in load_manifest(manifest):
        file_id = str(record.get("id") or "")
        if not file_id or file_id in records:
            raise ValueError("манифест содержит пустой или повторяющийся id")
        records[file_id] = record
    with source.open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        columns = list(reader.fieldnames or [])
        rows = list(reader)
    required = {"file_id", "cpWER", "cpwer_errors", "cpwer_ref_words"}
    if not required.issubset(columns) or len(set(columns)) != len(columns):
        raise ValueError("в исходном CSV отсутствуют обязательные поля или повторяются колонки")
    extra = [f"previous_{column}" for column in CP_COLUMNS] + list(PROVENANCE_COLUMNS)
    if set(extra).intersection(columns):
        raise ValueError("CSV уже содержит историю пересчёта; используйте исходный metrics.csv")
    source_hash = _sha256(source)
    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    seen: set[str] = set()
    safe_ids: set[str] = set()
    rescored: list[dict[str, str]] = []
    for row in rows:
        if None in row or any(value is None for value in row.values()):
            raise ValueError("число полей строки не совпадает с заголовком CSV")
        file_id = row.get("file_id", "")
        if not file_id or file_id in seen:
            raise ValueError("исходный CSV содержит пустой или повторяющийся file_id")
        seen.add(file_id)
        record = records.get(file_id, {})
        language = row.get("language") or str(record.get("language") or "")
        if options.language != "all" and language != options.language:
            continue
        if file_id not in records:
            raise ValueError(f"{file_id}: запись отсутствует в исходном манифесте")
        safe_id = _safe_id(file_id)
        if safe_id in safe_ids:
            raise ValueError("file_id выбранных записей имеют одинаковое имя RTTM")
        safe_ids.add(safe_id)
        try:
            if row.get("error", "").strip() or row.get("exit_code", "") not in ("", "0"):
                raise ValueError("нельзя пересчитать строку с ошибкой прогона")
            old_words = _reference_words(row)
            hypothesis, hypothesis_source, geometry_verified = _hypothesis(
                row, run, options.allow_live_transcripts,
            )
            if row.get("reference_snapshot"):
                reference = _file(run, row["reference_snapshot"], "снимок эталона")
                reference_source = "reference_snapshot"
            else:
                reference = _file(data_root, record.get("ref_text"), "эталон из манифеста")
                reference_source = "manifest_ref_text"
            hypothesis_hash = _sha256(hypothesis)
            reference_hash = _sha256(reference)
            if geometry_verified:
                rttm = _file(run, Path("rttm") / f"{safe_id}.rttm", "сохранённый RTTM")
                _verify_geometry(json.loads(hypothesis.read_text(encoding="utf-8")), rttm)
            details = cpwer(load_segments(reference), load_segments(hypothesis))
            if details["ref_words"] != old_words:
                raise ValueError("знаменатель эталона отличается от исходного cpwer_ref_words")
            if not math.isfinite(details["cpwer"]):
                raise ValueError("пересчитанный cpWER не является конечным числом")
            if hypothesis_hash != _sha256(hypothesis) or reference_hash != _sha256(reference):
                raise ValueError("входные тексты изменились во время пересчёта")
        except (OSError, ValueError, RuntimeError) as exc:
            raise ValueError(f"{file_id}: {exc}") from exc
        updated = dict(row)
        updated.update({f"previous_{column}": row.get(column, "") for column in CP_COLUMNS})
        updated.update({
            "cpWER": f"{details['cpwer']:.6f}", "cpwer_errors": str(details["errors"]),
            "cpwer_ref_words": str(details["ref_words"]), "cpwer_version": CPWER_VERSION,
            "rescore_source_metrics": str(source), "rescore_source_manifest": str(manifest),
            "rescore_source_metrics_sha256": source_hash,
            "rescore_hypothesis": str(hypothesis), "rescore_hypothesis_source": hypothesis_source,
            "rescore_hypothesis_sha256": hypothesis_hash,
            "rescore_reference": str(reference), "rescore_reference_source": reference_source,
            "rescore_reference_sha256": reference_hash,
            "rescore_geometry_verified": str(geometry_verified).lower(), "rescore_utc": timestamp,
        })
        rescored.append(updated)
    if not rescored:
        raise ValueError("нет записей выбранного языка")
    if source_hash != _sha256(source):
        raise ValueError("исходный CSV изменился во время пересчёта")
    if "cpwer_version" not in columns:
        columns.append("cpwer_version")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=columns + extra)
        writer.writeheader()
        writer.writerows(rescored)
    return {"out": output, "rows": len(rescored), "live": sum(row["rescore_geometry_verified"] == "true" for row in rescored)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--data-root", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--language", choices=("ru", "en", "all"), default="ru")
    parser.add_argument(
        "--allow-live-transcripts", action="store_true",
        help="разрешить текущие JSON при совпадении RTTM; совпадение не доказывает неизменность слов",
    )
    options = parser.parse_args(argv)
    try:
        result = rescore(options)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    print(f"{result['out']}: строк={result['rows']}, cpwer_version={CPWER_VERSION}")
    if result["live"]:
        print("Текущие JSON: совпадение RTTM не доказывает неизменность слов и не восстанавливает исходный прогон.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
