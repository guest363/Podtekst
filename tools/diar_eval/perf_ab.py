#!/usr/bin/env python3
"""Парный A/B двух релизных бинарников на одном наборе и одной машине."""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from perf import (
    HERE,
    REPO_ROOT,
    STAGE_ABS_MS,
    STAGE_COLUMNS,
    STAGE_REGRESSION,
    TOTAL_REGRESSION,
    bootstrap_ci,
    median,
    perf_path_for_transcript,
    read_metrics,
)


def _sidecar_meta(row: dict) -> dict:
    transcript = row.get("transcript")
    if not transcript:
        return {"commit": None, "engine": None}
    path = perf_path_for_transcript(transcript)
    if path is None or not path.is_file():
        return {"commit": None, "engine": None}
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {"commit": None, "engine": None}
    if not isinstance(doc, dict):
        return {"commit": None, "engine": None}
    return {"commit": doc.get("commit"), "engine": doc.get("engine")}


def _run_one(
    app: Path,
    manifest: Path,
    data_root: Path | None,
    file_id: str,
    out: Path,
    timeout: float,
) -> tuple[dict, dict]:
    command = [
        sys.executable,
        str(HERE / "run_batch.py"),
        "--app", str(app),
        "--manifest", str(manifest),
        "--out", str(out),
        "--ids", file_id,
        "--perf",
        "--timeout", str(timeout),
    ]
    if data_root is not None:
        command += ["--data-root", str(data_root)]
    result = subprocess.run(
        command, cwd=REPO_ROOT, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-5:]
        raise RuntimeError(f"run_batch вернул {result.returncode}: {' | '.join(tail)}")
    rows = read_metrics(out / "metrics.csv")
    row = rows.get(file_id) or next(iter(rows.values()), None)
    if row is None:
        raise RuntimeError(f"{file_id}: строка не найдена в metrics.csv")
    if row.get("error"):
        raise RuntimeError(f"{file_id}: {row['error']}")
    if not row.get("perf_ms"):
        raise RuntimeError(f"{file_id}: нет perf sidecar")
    return row, _sidecar_meta(row)


def _ratio(a: dict, b: dict, column: str) -> float | None:
    try:
        left = float(a[column])
        right = float(b[column])
    except (KeyError, TypeError, ValueError):
        return None
    if left <= 0 or right <= 0:
        return None
    ratio = right / left
    return ratio if math.isfinite(ratio) else None


def _abs_diff(a: dict, b: dict, column: str) -> float | None:
    try:
        diff = float(b[column]) - float(a[column])
    except (KeyError, TypeError, ValueError):
        return None
    return diff if math.isfinite(diff) else None


def _summary(values: list[float]) -> dict:
    clean = [value for value in values if value is not None and math.isfinite(value)]
    med = median(clean)
    low, high = bootstrap_ci(clean)
    return {
        "n": len(clean),
        "median": med,
        "ci95": [low, high],
    }


def _fmt(value: float | None, suffix: str = "") -> str:
    return "—" if value is None else f"{value:.4f}{suffix}"


def order_for(rep: int) -> tuple[str, str]:
    return ("a", "b") if rep % 2 == 0 else ("b", "a")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--a", type=Path, required=True, help="старый бинарник")
    parser.add_argument("--b", type=Path, required=True, help="новый бинарник")
    parser.add_argument("--suite", default="perf")
    parser.add_argument("--reps", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(os.environ.get("MR_DIAR_BENCH_ROOT", "~/.cache/podtekst-bench")).expanduser(),
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=3600)
    args = parser.parse_args(argv)
    if args.reps <= 0 or args.warmup < 0:
        print("ошибка: --reps > 0, --warmup >= 0", file=sys.stderr)
        return 2

    root = args.root.expanduser().resolve()
    suite = json.loads((HERE / "suites" / f"{args.suite}.json").read_text(encoding="utf-8"))
    cases: list[tuple[str, Path, Path | None]] = []
    for entry in suite["manifests"]:
        manifest = root / entry["path"]
        data_root = root / entry["data_root"] if entry.get("data_root") else None
        ids = entry.get("ids") or []
        if not ids:
            raise SystemExit(f"suite {args.suite}: у {entry['name']} нет ids")
        for file_id in ids:
            cases.append((file_id, manifest, data_root))

    app_a = args.a.expanduser().resolve()
    app_b = args.b.expanduser().resolve()
    for app in (app_a, app_b):
        if not app.is_file():
            print(f"ошибка: бинарник не найден: {app}", file=sys.stderr)
            return 2

    out = args.out
    runs_dir = out / "runs"
    runs_dir.mkdir(parents=True, exist_ok=True)
    session = Path(tempfile.mkdtemp(prefix="session-", dir=runs_dir))
    total_runs = args.warmup + args.reps
    samples: dict[str, dict[str, list[dict]]] = {
        file_id: {"a": [], "b": []} for file_id, _, _ in cases
    }
    binaries: dict[str, dict] = {"a": {}, "b": {}}
    for rep in range(total_runs):
        for file_id, manifest, data_root in cases:
            safe = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in file_id)[:120]
            for variant in order_for(rep):
                app = app_a if variant == "a" else app_b
                run_out = session / f"rep-{rep:02d}" / safe / variant
                print(f"[{rep + 1}/{total_runs}] {file_id} {variant}", flush=True)
                row, meta = _run_one(app, manifest, data_root, file_id, run_out, args.timeout)
                for key in ("commit", "engine"):
                    if meta.get(key):
                        binaries[variant].setdefault(key, meta[key])
                if rep >= args.warmup:
                    samples[file_id][variant].append(row)

    files: dict[str, dict] = {}
    all_total: list[float] = []
    stage_pairs: dict[str, list[tuple[float, float]]] = {
        column: [] for column in STAGE_COLUMNS
    }
    for file_id, variants in samples.items():
        a_rows = variants["a"]
        b_rows = variants["b"]
        total = [
            ratio
            for ratio in (_ratio(a, b, "rtf_norm") for a, b in zip(a_rows, b_rows))
            if ratio is not None
        ]
        all_total.extend(total)
        stages: dict[str, dict] = {}
        for column in STAGE_COLUMNS:
            pairs = []
            for a, b in zip(a_rows, b_rows):
                ratio = _ratio(a, b, column)
                absolute = _abs_diff(a, b, column)
                if ratio is not None and absolute is not None:
                    pairs.append((ratio, absolute))
            stage_pairs[column].extend(pairs)
            summary = _summary([ratio for ratio, _ in pairs])
            summary["abs_ms"] = median([absolute for _, absolute in pairs])
            stages[column] = summary
        files[file_id] = {
            "reps": len(a_rows),
            "total": _summary(total),
            "stages": stages,
        }

    stage_summary: dict[str, dict] = {}
    for column in STAGE_COLUMNS:
        summary = _summary([ratio for ratio, _ in stage_pairs[column]])
        summary["abs_ms"] = median([absolute for _, absolute in stage_pairs[column]])
        stage_summary[column] = summary
    total_summary = _summary(all_total)
    if total_summary["median"] is None:
        print("ошибка: нет парных данных rtf_norm", file=sys.stderr)
        return 2
    warnings = [
        f"{column}: +{(summary['median'] - 1) * 100:.1f}% и {summary['abs_ms']:.1f} мс"
        for column, summary in stage_summary.items()
        if summary["median"] is not None
        and summary["median"] > 1 + STAGE_REGRESSION
        and (summary["abs_ms"] or 0) > STAGE_ABS_MS
    ]
    regressed = total_summary["median"] is not None and total_summary["median"] > 1 + TOTAL_REGRESSION

    report = {
        "schema": 1,
        "suite": args.suite,
        "reps": args.reps,
        "warmup": args.warmup,
        "a": str(app_a),
        "b": str(app_b),
        "binaries": binaries,
        "total": total_summary,
        "stages": stage_summary,
        "files": files,
        "warnings": warnings,
        "regressed": regressed,
    }
    (out / "ab.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    lines = [
        "# A/B скорости",
        "",
        f"Набор: `{args.suite}`, повторов: {args.reps}, прогрев: {args.warmup}.",
        f"A: `{app_a}`",
        f"B: `{app_b}`",
        "",
        "| файл | B/A total | 95% CI |",
        "|---|---|---|",
    ]
    for file_id, item in sorted(files.items()):
        low, high = item["total"]["ci95"]
        lines.append(
            f"| {file_id} | {_fmt(item['total']['median'])} | "
            f"{_fmt(low)}–{_fmt(high)} |"
        )
    low, high = total_summary["ci95"]
    lines += [
        f"| **aggregate** | **{_fmt(total_summary['median'])}** | "
        f"{_fmt(low)}–{_fmt(high)} |",
        "",
        "| стадия | B/A | абсолют, мс |",
        "|---|---|---|",
    ]
    for column, summary in stage_summary.items():
        lines.append(
            f"| {column} | {_fmt(summary['median'])} | {_fmt(summary['abs_ms'])} |"
        )
    lines.append("")
    if warnings:
        lines.append("Предупреждения по стадиям:")
        lines.extend(f"- {warning}" for warning in warnings)
    lines.append(
        "Итог: регресс total > +5%" if regressed else "Итог: регресса total нет"
    )
    (out / "ab-report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")

    print("\n".join(lines))
    print(f"отчёт: {out / 'ab-report.md'}")
    return 1 if regressed else 0


if __name__ == "__main__":
    try:
        code = main()
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        code = 2
    raise SystemExit(code)
