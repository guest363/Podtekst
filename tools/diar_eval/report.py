#!/usr/bin/env python3
"""Сборка report.md из metrics.csv и сравнение с baseline.json."""

from __future__ import annotations

import argparse
import csv
import json
import math
import random
import sys
import tempfile
from datetime import datetime
from pathlib import Path

RATE_METRICS = ("WER", "CER", "DER", "SCA", "cpWER", "attribution")
BOOTSTRAP_SAMPLES = 1000
BOOTSTRAP_SEED = 20261109
CI_METRICS = {
    "WER": ("wer_errors", "ref_words"),
    "CER": ("cer_errors", "ref_chars"),
    "attribution": ("attribution_correct", "attribution_matched"),
}
DER_PARTS = ("der_miss_sec", "der_false_alarm_sec", "der_confusion_sec")
BASELINE_ALIASES = {
    "WER": ("WER", "wer"),
    "CER": ("CER", "cer"),
    "DER": ("DER", "der"),
    "SCA": ("SCA", "sca"),
    "cpWER": ("cpWER", "cpwer", "cp_wer"),
    "attribution": ("attribution", "attribution_accuracy", "accuracy"),
    "RTF_p50": ("RTF_p50", "rtf_p50", "RTF", "rtf"),
    "peak_rss_kb": ("peak_rss_kb", "rss_kb", "peak_rss"),
}


def _read_rows(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as handle:
        return [dict(row) for row in csv.DictReader(handle)]


def _value(row: dict, key: str) -> float | None:
    raw = row.get(key)
    if raw is None or str(raw).strip() == "":
        return None
    try:
        value = float(raw)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return value if math.isfinite(value) else None


def _exit_code(row: dict) -> int | None:
    value = _value(row, "exit_code")
    return None if value is None else int(value)


def _group_key(row: dict) -> str:
    dataset = str(row.get("dataset") or "unknown")
    subset = str(row.get("subset") or "")
    return f"{dataset}/{subset}" if subset else dataset


def _mean(rows: list[dict], key: str) -> float:
    values = [value for row in rows if (value := _value(row, key)) is not None]
    return sum(values) / len(values) if values else math.nan


def _ratio(rows: list[dict], numerator: str, denominator: str) -> float:
    total = 0.0
    base = 0.0
    for row in rows:
        top = _value(row, numerator)
        bottom = _value(row, denominator)
        if top is None or bottom is None or bottom <= 0:
            continue
        total += top
        base += bottom
    return total / base if base > 0 else math.nan


def _der_ratio(rows: list[dict]) -> float:
    total = 0.0
    base = 0.0
    for row in rows:
        miss = _value(row, "der_miss_sec")
        false_alarm = _value(row, "der_false_alarm_sec")
        confusion = _value(row, "der_confusion_sec")
        ref_sec = _value(row, "der_ref_sec")
        if None in (miss, false_alarm, confusion, ref_sec) or ref_sec <= 0:
            continue
        total += miss + false_alarm + confusion
        base += ref_sec
    return total / base if base > 0 else math.nan


def _percentile(values: list[float], percentile: float) -> float:
    if not values:
        return math.nan
    ordered = sorted(values)
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * percentile / 100.0
    low = math.floor(position)
    high = math.ceil(position)
    if low == high:
        return ordered[low]
    weight = position - low
    return ordered[low] * (1.0 - weight) + ordered[high] * weight


def _ci_pairs(rows: list[dict], metric: str) -> list[tuple[float, float]]:
    pairs: list[tuple[float, float]] = []
    for row in rows:
        if metric == "DER":
            parts = [_value(row, key) for key in DER_PARTS]
            if any(part is None for part in parts):
                continue
            numerator = sum(part for part in parts if part is not None)
            denominator = _value(row, "der_ref_sec")
        else:
            numerator_key, denominator_key = CI_METRICS[metric]
            numerator = _value(row, numerator_key)
            denominator = _value(row, denominator_key)
        if numerator is None or denominator is None or denominator <= 0:
            continue
        pairs.append((numerator, denominator))
    return pairs


def bootstrap_ci(
    rows: list[dict],
    metric: str,
    samples: int = BOOTSTRAP_SAMPLES,
    seed: int = BOOTSTRAP_SEED,
) -> tuple[float, float] | None:
    pairs = _ci_pairs(rows, metric)
    if len(pairs) < 2 or samples < 1:
        return None
    rng = random.Random(seed)
    count = len(pairs)
    estimates = []
    for _ in range(samples):
        numerator = 0.0
        denominator = 0.0
        for _ in range(count):
            top, bottom = pairs[rng.randrange(count)]
            numerator += top
            denominator += bottom
        estimates.append(numerator / denominator)
    return _percentile(estimates, 2.5), _percentile(estimates, 97.5)


def _fallback_ratio(rows: list[dict], numerator: str, denominator: str, key: str) -> float:
    value = _ratio(rows, numerator, denominator)
    return value if math.isfinite(value) else _mean(rows, key)


def aggregate(rows: list[dict]) -> dict:
    durations = [value for row in rows if (value := _value(row, "duration")) is not None]
    rtf_values = [
        value
        for row in rows
        if _exit_code(row) == 0 and (value := _value(row, "rtf")) is not None
    ]
    rss_values = [
        value for row in rows if (value := _value(row, "peak_rss_kb")) is not None
    ]
    attribution = _fallback_ratio(
        rows, "attribution_correct", "attribution_matched", "attribution"
    )
    der_ratio = _der_ratio(rows)
    return {
        "files": len(rows),
        "failed": sum(1 for row in rows if _exit_code(row) != 0),
        "errors": sum(1 for row in rows if str(row.get("error") or "").strip()),
        "duration_sec": sum(durations),
        "WER": _fallback_ratio(rows, "wer_errors", "ref_words", "WER"),
        "CER": _fallback_ratio(rows, "cer_errors", "ref_chars", "CER"),
        "DER": der_ratio if math.isfinite(der_ratio) else _mean(rows, "DER"),
        "SCA": _mean(rows, "SCA"),
        "MAE": _mean(rows, "MAE"),
        "cpWER": _fallback_ratio(rows, "cpwer_errors", "cpwer_ref_words", "cpWER"),
        "attribution": attribution,
        "rtf_p50": _percentile(rtf_values, 50),
        "rtf_p95": _percentile(rtf_values, 95),
        "rss_max": max(rss_values) if rss_values else math.nan,
    }


def aggregate_groups(rows: list[dict]) -> dict[str, dict]:
    groups: dict[str, list[dict]] = {}
    for row in rows:
        groups.setdefault(_group_key(row), []).append(row)
    return {key: aggregate(items) for key, items in sorted(groups.items())}


def _baseline_groups(payload: object) -> dict[str, dict]:
    if not isinstance(payload, dict):
        raise ValueError("baseline.json: ожидался объект")
    for key in ("groups", "runs", "datasets"):
        value = payload.get(key)
        if isinstance(value, dict):
            return {str(name): item for name, item in value.items() if isinstance(item, dict)}
    ignored = {"schema", "version", "created", "created_utc"}
    return {
        str(name): item
        for name, item in payload.items()
        if isinstance(item, dict) and name not in ignored
    }


def _baseline_value(item: dict, metric: str) -> float | None:
    lower = {str(key).lower(): value for key, value in item.items()}
    for alias in BASELINE_ALIASES[metric]:
        if alias in item:
            value = _value({"x": item[alias]}, "x")
            if value is not None:
                return value
        if alias.lower() in lower:
            value = _value({"x": lower[alias.lower()]}, "x")
            if value is not None:
                return value
    return None


def _fmt_rate(value: float | None) -> str:
    return "—" if value is None or not math.isfinite(value) else f"{value * 100:.2f}%"


def _fmt_number(value: float | None, digits: int = 3) -> str:
    return "—" if value is None or not math.isfinite(value) else f"{value:.{digits}f}"


def _fmt_ci(rows: list[dict], metric: str, point: float) -> str:
    interval = bootstrap_ci(rows, metric)
    if interval is None or not math.isfinite(point):
        return "n/a"
    low, high = interval
    return f"{point * 100:.2f}% [{low * 100:.2f}; {high * 100:.2f}]"


def _status(metric: str, base: float, current: float) -> tuple[str, str]:
    delta = current - base
    if metric in ("WER", "DER"):
        limit = min(0.05 * abs(base), 0.01)
        if delta > limit:
            return f"{delta * 100:+.2f} п.п.", "регресс"
        if delta < 0:
            return f"{delta * 100:+.2f} п.п.", "улучшение"
        return f"{delta * 100:+.2f} п.п.", "ok"
    if metric == "cpWER":
        if delta > 0.03:
            return f"{delta * 100:+.2f} п.п.", "регресс"
        if delta < 0:
            return f"{delta * 100:+.2f} п.п.", "улучшение"
        return f"{delta * 100:+.2f} п.п.", "ok"
    if metric == "attribution":
        if -delta > 0.03:
            return f"{delta * 100:+.2f} п.п.", "регресс"
        if delta > 0:
            return f"{delta * 100:+.2f} п.п.", "улучшение"
        return f"{delta * 100:+.2f} п.п.", "ok"
    if metric == "RTF_p50":
        if base <= 0:
            return "—", "нет данных"
        relative = (current - base) / base
        status = "регресс" if relative > 0.15 else "улучшение" if relative < 0 else "ok"
        return f"{relative * 100:+.1f}%", status
    if metric == "peak_rss_kb":
        if base <= 0:
            return "—", "нет данных"
        relative = (current - base) / base
        status = "регресс" if relative > 0.20 else "улучшение" if relative < 0 else "ok"
        return f"{relative * 100:+.1f}%", status
    return f"{delta * 100:+.2f} п.п.", "—"


def compare_baseline(groups: dict[str, dict], baseline: dict[str, dict]) -> list[dict]:
    comparisons: list[dict] = []
    for group, item in baseline.items():
        current = groups.get(group)
        for metric in BASELINE_ALIASES:
            base = _baseline_value(item, metric)
            if base is None:
                continue
            current_value = None
            if current is not None:
                current_value = current.get(metric if metric != "RTF_p50" else "rtf_p50")
                if metric == "peak_rss_kb":
                    current_value = current.get("rss_max")
            if current_value is None or not math.isfinite(current_value):
                comparisons.append(
                    {
                        "group": group,
                        "metric": metric,
                        "baseline": base,
                        "current": None,
                        "delta": "—",
                        "status": "нет в прогоне" if current is None else "нет данных",
                    }
                )
                continue
            delta, status = _status(metric, base, current_value)
            comparisons.append(
                {
                    "group": group,
                    "metric": metric,
                    "baseline": base,
                    "current": current_value,
                    "delta": delta,
                    "status": status,
                }
            )
    return comparisons


def _md(value: object) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ")


def _table(header: list[str], rows: list[list[str]]) -> list[str]:
    lines = ["| " + " | ".join(header) + " |", "|" + "|".join("---" for _ in header) + "|"]
    lines.extend("| " + " | ".join(_md(cell) for cell in row) + " |" for row in rows)
    return lines


def render(
    rows: list[dict],
    groups: dict[str, dict],
    baseline: dict[str, dict] | None,
    source: str,
    title: str,
    collar: float,
    skip_overlap: bool,
) -> str:
    overall = aggregate(rows)
    lines = [
        f"# {title}",
        "",
        f"- Источник: `{source}`",
        f"- Сформирован: {datetime.now().strftime('%Y-%m-%d %H:%M')}",
        f"- DER: collar={collar:.2f}, skip_overlap={skip_overlap}",
        "",
        "## Сводка",
        "",
        f"- Файлов: **{overall['files']}**",
        f"- Сбоев процесса: **{overall['failed']}**",
        f"- Строк с ошибками: **{overall['errors']}**",
        f"- Суммарная длительность: **{overall['duration_sec'] / 60:.1f} мин**",
        "- WER и CER посчитаны по сумме ошибок (micro), DER — по сумме компонентов.",
        "- Метрики агрегируются по строкам с значениями; сбои учтены отдельно.",
    ]
    if overall["failed"] or overall["errors"]:
        lines.extend(["", "**В прогоне есть сбои или ошибки — см. раздел «Ошибки».**"])
    lines.extend(["", "## По наборам", ""])

    header = [
        "Набор",
        "Файлов",
        "Сбоев",
        "Ошибок",
        "Мин",
        "WER",
        "CER",
        "DER",
        "SCA",
        "MAE",
        "cpWER",
        "Attribution",
        "RTF p50",
        "RTF p95",
        "RSS max, МБ",
    ]
    table_rows = []
    for key, item in [("ВСЕГО", overall), *groups.items()]:
        table_rows.append(
            [
                key,
                str(item["files"]),
                str(item["failed"]),
                str(item["errors"]),
                f"{item['duration_sec'] / 60:.1f}",
                _fmt_rate(item["WER"]),
                _fmt_rate(item["CER"]),
                _fmt_rate(item["DER"]),
                _fmt_rate(item["SCA"]),
                _fmt_number(item["MAE"], 2),
                _fmt_rate(item["cpWER"]),
                _fmt_rate(item["attribution"]),
                _fmt_number(item["rtf_p50"]),
                _fmt_number(item["rtf_p95"]),
                _fmt_number(
                    item["rss_max"] / 1024 if math.isfinite(item["rss_max"]) else math.nan,
                    1,
                ),
            ]
        )
    lines.extend(_table(header, table_rows))

    group_rows: dict[str, list[dict]] = {}
    for row in rows:
        group_rows.setdefault(_group_key(row), []).append(row)
    ci_rows = []
    for key, item, items in [
        ("ВСЕГО", overall, rows),
        *((name, value, group_rows[name]) for name, value in groups.items()),
    ]:
        ci_rows.append(
            [
                key,
                str(item["files"]),
                _fmt_ci(items, "WER", item["WER"]),
                _fmt_ci(items, "CER", item["CER"]),
                _fmt_ci(items, "DER", item["DER"]),
                _fmt_ci(items, "attribution", item["attribution"]),
            ]
        )
    lines.extend(
        [
            "",
            "## Доверительные интервалы",
            "",
            f"- Bootstrap по файлам: {BOOTSTRAP_SAMPLES} ресэмплов, seed {BOOTSTRAP_SEED}, "
            "95% перцентильный интервал micro-оценки.",
            "- `n/a` — меньше двух файлов с данными или нулевой знаменатель.",
            "",
        ]
    )
    lines.extend(
        _table(
            [
                "Набор",
                "Файлов",
                "WER 95% CI",
                "CER 95% CI",
                "DER 95% CI",
                "Attribution 95% CI",
            ],
            ci_rows,
        )
    )

    if baseline:
        comparisons = compare_baseline(groups, baseline)
        lines.extend(["", "## Сравнение с baseline", ""])
        if comparisons:
            compare_rows = []
            for item in comparisons:
                metric = item["metric"]
                if metric in RATE_METRICS:
                    base_text = _fmt_rate(item["baseline"])
                    current_text = _fmt_rate(item["current"])
                elif metric == "peak_rss_kb":
                    base_text = _fmt_number(item["baseline"] / 1024, 1) + " МБ"
                    current_text = (
                        _fmt_number(item["current"] / 1024, 1) + " МБ"
                        if item["current"] is not None
                        else "—"
                    )
                else:
                    base_text = _fmt_number(item["baseline"])
                    current_text = _fmt_number(item["current"])
                compare_rows.append(
                    [
                        item["group"],
                        metric,
                        base_text,
                        current_text,
                        item["delta"],
                        item["status"],
                    ]
                )
            lines.extend(
                _table(
                    ["Набор", "Метрика", "Baseline", "Прогон", "Δ", "Статус"],
                    compare_rows,
                )
            )
        else:
            lines.append("В baseline нет метрик, совпавших с прогоном.")
        regressions = [item for item in comparisons if item["status"] == "регресс"]
        if regressions:
            lines.extend(["", "**Есть регрессы. Релиз блокируется по разделу 9 ТЗ.**"])

    errors = [row for row in rows if str(row.get("error") or "").strip()]
    lines.extend(["", "## Ошибки", ""])
    if errors:
        error_rows = [
            [
                _group_key(row),
                str(row.get("file_id") or ""),
                str(row.get("error") or ""),
            ]
            for row in errors[:50]
        ]
        lines.extend(_table(["Набор", "Файл", "Ошибка"], error_rows))
        if len(errors) > 50:
            lines.append(f"Показаны первые 50 из {len(errors)}.")
    else:
        lines.append("Ошибок нет.")

    lines.extend(
        [
            "",
            "## Ограничения",
            "",
            "- cpWER перебирает перестановки до 8 спикеров.",
            "- Attribution accuracy требует эталонный текст с метками спикеров.",
            "- RTF p50/p95 включает все успешные файлы; прогрев отдельно не исключается.",
            "- Peak RSS снят через `/usr/bin/time -v`, если он доступен.",
            "- CI bootstrap по файлам: на 1-2 файлах перцентильный интервал недооценивает разброс.",
            "",
        ]
    )
    return "\n".join(lines)


def _self_test() -> int:
    rows = [
        {
            "dataset": "ds",
            "subset": "test",
            "file_id": "one",
            "exit_code": "0",
            "error": "",
            "duration": "60",
            "WER": "0.1",
            "ref_words": "10",
            "wer_errors": "1",
            "CER": "0.05",
            "ref_chars": "20",
            "cer_errors": "1",
            "DER": "0.2",
            "der_ref_sec": "10",
            "der_miss_sec": "1",
            "der_false_alarm_sec": "1",
            "der_confusion_sec": "0",
            "SCA": "1",
            "MAE": "0",
            "cpWER": "0.1",
            "cpwer_ref_words": "10",
            "cpwer_errors": "1",
            "attribution": "0.9",
            "attribution_matched": "10",
            "attribution_correct": "9",
            "rtf": "1.0",
            "peak_rss_kb": "1000",
        },
        {
            "dataset": "ds",
            "subset": "test",
            "file_id": "two",
            "exit_code": "0",
            "error": "",
            "duration": "120",
            "WER": "0.2",
            "ref_words": "30",
            "wer_errors": "2",
            "CER": "0.1",
            "ref_chars": "40",
            "cer_errors": "2",
            "DER": "0.1",
            "der_ref_sec": "20",
            "der_miss_sec": "2",
            "der_false_alarm_sec": "0",
            "der_confusion_sec": "0",
            "SCA": "0",
            "MAE": "1",
            "cpWER": "0.2",
            "cpwer_ref_words": "20",
            "cpwer_errors": "4",
            "attribution": "0.8",
            "attribution_matched": "10",
            "attribution_correct": "8",
            "rtf": "0.5",
            "peak_rss_kb": "2000",
        },
    ]
    groups = aggregate_groups(rows)
    item = groups["ds/test"]
    assert item["files"] == 2
    assert abs(item["WER"] - 3 / 40) < 1e-12, item["WER"]
    assert abs(item["DER"] - 4 / 30) < 1e-12, item["DER"]
    assert abs(item["SCA"] - 0.5) < 1e-12
    assert abs(item["MAE"] - 0.5) < 1e-12
    assert abs(item["cpWER"] - 5 / 30) < 1e-12
    assert abs(item["attribution"] - 17 / 20) < 1e-12
    assert abs(item["rtf_p50"] - 0.75) < 1e-12
    assert abs(item["rtf_p95"] - 0.975) < 1e-12
    assert abs(_percentile([1, 2, 3, 4], 50) - 2.5) < 1e-12

    baseline = {
        "groups": {
            "ds/test": {
                "WER": 0.10,
                "DER": 0.12,
                "SCA": 0.8,
                "cpWER": 0.10,
                "attribution": 0.9,
                "RTF_p50": 0.5,
                "peak_rss_kb": 1500,
            }
        }
    }
    comparisons = compare_baseline(groups, _baseline_groups(baseline))
    statuses = {(row["group"], row["metric"]): row["status"] for row in comparisons}
    assert statuses[("ds/test", "WER")] == "улучшение"
    assert statuses[("ds/test", "DER")] == "регресс"
    assert statuses[("ds/test", "cpWER")] == "регресс"
    assert statuses[("ds/test", "attribution")] == "регресс"
    assert statuses[("ds/test", "RTF_p50")] == "регресс"
    assert statuses[("ds/test", "peak_rss_kb")] == "регресс"

    report = render(rows, groups, _baseline_groups(baseline), "memory.csv", "Тест", 0.0, False)
    assert "ds/test" in report and "Сравнение с baseline" in report
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "report.md"
        path.write_text(report, encoding="utf-8")
        assert path.stat().st_size > 0

    ci_rows = [
        {
            "dataset": "ci",
            "subset": "test",
            "file_id": "a",
            "wer_errors": "2",
            "ref_words": "10",
            "cer_errors": "1",
            "ref_chars": "10",
            "der_ref_sec": "100",
            "der_miss_sec": "10",
            "der_false_alarm_sec": "5",
            "der_confusion_sec": "5",
            "attribution_correct": "8",
            "attribution_matched": "10",
        },
        {
            "dataset": "ci",
            "subset": "test",
            "file_id": "b",
            "wer_errors": "0",
            "ref_words": "20",
            "cer_errors": "2",
            "ref_chars": "20",
            "der_ref_sec": "100",
            "der_miss_sec": "0",
            "der_false_alarm_sec": "10",
            "der_confusion_sec": "0",
            "attribution_correct": "10",
            "attribution_matched": "10",
        },
        {
            "dataset": "ci",
            "subset": "test",
            "file_id": "c",
            "wer_errors": "1",
            "ref_words": "10",
            "cer_errors": "0",
            "ref_chars": "10",
            "der_ref_sec": "100",
            "der_miss_sec": "0",
            "der_false_alarm_sec": "0",
            "der_confusion_sec": "0",
            "attribution_correct": "9",
            "attribution_matched": "10",
        },
    ]
    ci_aggregate = aggregate(ci_rows)
    assert abs(ci_aggregate["WER"] - 3 / 40) < 1e-12
    for metric, point in (
        ("WER", ci_aggregate["WER"]),
        ("CER", ci_aggregate["CER"]),
        ("DER", ci_aggregate["DER"]),
        ("attribution", ci_aggregate["attribution"]),
    ):
        interval = bootstrap_ci(ci_rows, metric)
        assert interval is not None, metric
        low, high = interval
        assert math.isfinite(low) and math.isfinite(high), metric
        assert low <= point <= high, (metric, low, point, high)
        assert low < high, metric
        assert bootstrap_ci(ci_rows, metric) == interval, metric
    assert bootstrap_ci(ci_rows[:1], "WER") is None
    assert bootstrap_ci([{"ref_words": "10"}, {"ref_words": "10"}], "WER") is None
    assert bootstrap_ci(ci_rows, "WER", samples=0) is None

    ci_report = render(ci_rows, aggregate_groups(ci_rows), None, "memory.csv", "Тест CI", 0.0, False)
    assert "## Доверительные интервалы" in ci_report and "WER 95% CI" in ci_report
    assert f"seed {BOOTSTRAP_SEED}" in ci_report
    single_report = render(ci_rows[:1], aggregate_groups(ci_rows[:1]), None, "memory.csv", "Т", 0.0, False)
    assert "n/a" in single_report

    print("self-test ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", type=Path, help="каталог прогона с metrics.csv")
    parser.add_argument("--csv", type=Path, help="путь к metrics.csv")
    parser.add_argument("--out", type=Path, help="путь к report.md")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--title", default="Отчёт Podtekst на открытых датасетах")
    parser.add_argument("--collar", type=float, default=0.0)
    parser.add_argument("--skip-overlap", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    if args.csv is None and args.run is None:
        parser.error("нужен --run или --csv")
    csv_path = args.csv or (args.run / "metrics.csv")
    out = args.out or (
        (args.run / "report.md") if args.run is not None else csv_path.with_name("report.md")
    )

    try:
        rows = _read_rows(csv_path)
        baseline = None
        if args.baseline:
            baseline = _baseline_groups(
                json.loads(args.baseline.read_text(encoding="utf-8"))
            )
        report = render(
            rows,
            aggregate_groups(rows),
            baseline,
            str(csv_path),
            args.title,
            args.collar,
            args.skip_overlap,
        )
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2

    try:
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(report, encoding="utf-8")
    except OSError as exc:
        print(f"ошибка записи {out}: {exc}", file=sys.stderr)
        return 2
    print(f"{out}: строк={len(rows)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
