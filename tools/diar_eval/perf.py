#!/usr/bin/env python3
"""Замеры скорости конвейера: capture baseline, check регресса, общие helpers."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import random
import secrets
import shutil
import statistics
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[1]
DEFAULT_BASELINE = HERE / "perf-baseline.json"
BOOTSTRAP_SEED = 20261119
BOOTSTRAP_REPS = 1000

STAGE_NAMES = ("decode", "models", "vad_model", "vad", "asr", "diar", "diar_models", "json")
STAGE_COLUMNS = tuple(f"{name}_ms" for name in STAGE_NAMES)
METRIC_COLUMNS = (
    "perf_ms",
    "calib_ms",
    "rtf_perf",
    "rtf_norm",
    *STAGE_COLUMNS,
    "other_ms",
    "peak_rss_inproc_kb",
    "input_channels",
    "calib_version",
    "calib_dim",
)

# Сырой RTF шумит от загрузки машины, поэтому регресс решает медиана rtf_norm,
# а стадии проверяются только при заметном вкладе — иначе ловим дрожание таймера.
TOTAL_REGRESSION = 0.05
STAGE_REGRESSION = 0.10
STAGE_ABS_MS = 20.0


def bench_root() -> Path:
    return Path(
        os.environ.get("MR_DIAR_BENCH_ROOT", "~/.cache/podtekst-bench")
    ).expanduser()


def perf_path_for_transcript(transcript: Path | str) -> Path | None:
    """`X.transcript.json` → `X.perf.json`; прочие имена не трогаем."""
    path = Path(transcript)
    suffix = ".transcript.json"
    if not path.name.endswith(suffix):
        return None
    return path.with_name(path.name[: -len(suffix)] + ".perf.json")


def record_id(record: dict) -> str:
    """Вычисленный file_id: явный `id` либо `dataset/subset/stem`."""
    explicit = str(record.get("id") or "").strip()
    if explicit:
        return explicit
    dataset = str(record.get("dataset") or "unknown")
    subset = str(record.get("subset") or "")
    stem = Path(str(record.get("audio") or "file")).stem
    return f"{dataset}/{subset}/{stem}".strip("/")


def _slash(path: object) -> str:
    return str(path or "").replace("\\", "/").strip()


def _requested_ids(ids: str | None) -> list[str]:
    return [item.strip() for item in (ids or "").split(",") if item.strip()]


def _audio_matches(record: dict, wanted: str) -> bool:
    suffix = f"{_slash(wanted).rstrip('/')}.wav"
    return _slash(record.get("audio")).endswith(suffix)


def requested_id_for(record: dict, ids: str | None) -> str | None:
    """Id набора, которым выбрана запись: точный, иначе суффикс пути аудио."""
    requested = _requested_ids(ids)
    for wanted in requested:
        if record_id(record) == wanted:
            return wanted
    for wanted in requested:
        if _audio_matches(record, wanted):
            return wanted
    return None


def filter_by_ids(records: list[dict], ids: str | None) -> list[dict]:
    """Фильтр по id: точное совпадение или однозначный суффикс пути аудио."""
    requested = _requested_ids(ids)
    if not requested:
        return records
    selected: list[dict] = []
    used: set[int] = set()
    for wanted in requested:
        candidates = [record for record in records if record_id(record) == wanted]
        if not candidates:
            candidates = [record for record in records if _audio_matches(record, wanted)]
        if not candidates:
            raise ValueError(f"id не найден в манифесте: {wanted}")
        if len(candidates) > 1:
            found = ", ".join(sorted({record_id(record) for record in candidates}))
            raise ValueError(f"id неоднозначен: {wanted} → {found}")
        index = next(
            index for index, record in enumerate(records) if record is candidates[0]
        )
        if index not in used:
            selected.append(candidates[0])
            used.add(index)
    return selected


def _number(value: object) -> float | None:
    text = str(value).strip() if value is not None else ""
    if not text or text.lower() in {"nan", "inf", "-inf"}:
        return None
    try:
        parsed = float(text)
    except ValueError:
        return None
    return parsed if math.isfinite(parsed) else None


def median(values) -> float | None:
    clean = [value for value in values if value is not None and math.isfinite(value)]
    return statistics.median(clean) if clean else None


def result_key(row: dict) -> str:
    """Ключ baseline: id набора, иначе вычисленный file_id (старые прогоны)."""
    return str(row.get("requested_id") or row.get("file_id") or "").strip()


def read_metrics(path: Path | str) -> dict[str, dict]:
    path = Path(path)
    if not path.is_file():
        raise FileNotFoundError(f"metrics.csv не найден: {path}")
    rows: dict[str, dict] = {}
    with path.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            key = result_key(row)
            if key:
                rows[key] = row
    return rows


def summarize_rows(rows: list[dict]) -> dict:
    """Медианы по повторам; rtf_norm считается из медиан, а не из отношений."""
    summary = {
        column: median([_number(row.get(column)) for row in rows])
        for column in METRIC_COLUMNS
    }
    duration = median([_number(row.get("duration")) for row in rows])
    perf_ms = summary.get("perf_ms")
    calib_ms = summary.get("calib_ms")
    if duration and duration > 0 and perf_ms is not None:
        summary["rtf_perf"] = perf_ms / 1000.0 / duration
        if calib_ms:
            summary["rtf_norm"] = (perf_ms / calib_ms) / duration
    profiles = {
        str(row.get("calib_profile") or "").strip()
        for row in rows
        if str(row.get("calib_profile") or "").strip()
    }
    summary["calib_profile"] = profiles.pop() if len(profiles) == 1 else None
    return summary


def summarize_runs(files: list[Path]) -> tuple[dict[str, dict], int]:
    """Склеивает несколько `metrics.csv` и берёт медиану по файлам."""
    grouped: dict[str, list[dict]] = {}
    for path in files:
        for row in read_metrics(path).values():
            grouped.setdefault(result_key(row), []).append(row)
    records = {file_id: summarize_rows(rows) for file_id, rows in grouped.items()}
    return records, len(files)


def collect_run_files(paths: list[Path]) -> list[Path]:
    files: list[Path] = []
    for path in paths:
        if path.is_dir():
            reps = sorted(path.glob("rep-*/metrics.csv"))
            if reps:
                files.extend(reps)
            elif (path / "metrics.csv").is_file():
                files.append(path / "metrics.csv")
            else:
                raise FileNotFoundError(f"в каталоге нет metrics.csv: {path}")
        elif path.is_file():
            files.append(path)
        else:
            raise FileNotFoundError(f"прогон не найден: {path}")
    return files


def aggregate_records(records: dict[str, dict]) -> dict:
    """Агрегат набора: медиана по файлам (каждый сценарий весит одинаково)."""
    result = {}
    for column in METRIC_COLUMNS:
        result[column] = median(
            [_number(record.get(column)) for record in records.values()]
        )
    profiles = {
        str(record.get("calib_profile"))
        for record in records.values()
        if record.get("calib_profile")
    }
    result["calib_profile"] = profiles.pop() if len(profiles) == 1 else None
    return result


def bootstrap_ci(values: list[float]) -> tuple[float | None, float | None]:
    clean = [value for value in values if value is not None and math.isfinite(value)]
    if not clean:
        return None, None
    rng = random.Random(BOOTSTRAP_SEED)
    medians = sorted(
        statistics.median([clean[rng.randrange(len(clean))] for _ in clean])
        for _ in range(BOOTSTRAP_REPS)
    )
    low = medians[int(0.025 * (BOOTSTRAP_REPS - 1))]
    high = medians[int(0.975 * (BOOTSTRAP_REPS - 1))]
    return low, high


def cpu_model() -> str:
    system = platform.system()
    try:
        if system == "Linux":
            text = Path("/proc/cpuinfo").read_text(encoding="utf-8", errors="replace")
            for line in text.splitlines():
                if line.lower().startswith("model name"):
                    return line.split(":", 1)[1].strip()
        elif system == "Darwin":
            result = subprocess.run(
                ["sysctl", "-n", "machdep.cpu.brand_string"],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            if result.returncode == 0 and result.stdout.strip():
                return result.stdout.strip()
        elif system == "Windows":
            value = os.environ.get("PROCESSOR_IDENTIFIER", "").strip()
            if value:
                return value
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return platform.processor() or "unknown"


def _bench_id(root: Path) -> str | None:
    """Стабильный локальный id стенда: снимает коллизию одинаковых CPU."""
    path = root / "machine-id"
    try:
        if path.is_file():
            value = path.read_text(encoding="utf-8").strip()
            if value:
                return value
        root.mkdir(parents=True, exist_ok=True)
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            value = path.read_text(encoding="utf-8").strip()
            if value:
                return value
            descriptor = os.open(path, os.O_WRONLY | os.O_TRUNC, 0o600)
        value = secrets.token_hex(6)
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(value + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        return value
    except OSError as exc:
        print(
            f"предупреждение: machine-id недоступен ({exc}); machine_id без bench_id",
            file=sys.stderr,
        )
        return None


def machine_fingerprint(root: Path | None = None) -> dict:
    root = Path(root).expanduser() if root else bench_root()
    bench_id = _bench_id(root)
    cpu = cpu_model()
    system = platform.system()
    arch = platform.machine()
    logical_cpus = os.cpu_count() or 1
    parts = [cpu, system, arch, str(logical_cpus)]
    if bench_id:
        parts.append(bench_id)
    return {
        "machine_id": hashlib.sha256("\n".join(parts).encode("utf-8")).hexdigest()[:12],
        "bench_id": bench_id,
        "cpu": cpu,
        "os": system,
        "arch": arch,
        "logical_cpus": logical_cpus,
    }


def _relative(current: float | None, baseline: float | None) -> float | None:
    if current is None or baseline is None or baseline <= 0:
        return None
    return current / baseline - 1.0


def compare_perf(
    baseline: dict, current: dict
) -> tuple[list[str], list[tuple[str, str, float, float, float]]]:
    """Сверяет медианы файлов и агрегат; возвращает ошибки и строки таблицы."""
    errors: list[str] = []
    table: list[tuple[str, str, float, float, float]] = []
    base_records = baseline.get("records") or {}
    run_records = current.get("records") or {}
    missing = sorted(set(base_records) - set(run_records))
    extra = sorted(set(run_records) - set(base_records))
    errors.extend(f"нет файла в прогоне: {file_id}" for file_id in missing)
    if extra:
        print(f"предупреждение: лишние файлы вне baseline: {', '.join(extra)}", file=sys.stderr)

    def check_total(label: str, base: dict, run: dict) -> None:
        base_value = _number(base.get("rtf_norm"))
        run_value = _number(run.get("rtf_norm"))
        change = _relative(run_value, base_value)
        if base_value is None or run_value is None or change is None:
            errors.append(f"{label}: нет rtf_norm для сравнения")
            return
        table.append((label, "total rtf_norm", base_value, run_value, change))
        if change > TOTAL_REGRESSION:
            errors.append(f"{label}: total rtf_norm +{change * 100:.1f}% > +5%")

    def check_stages(label: str, base: dict, run: dict, enforce: bool) -> None:
        for column in STAGE_COLUMNS:
            base_value = _number(base.get(column))
            run_value = _number(run.get(column))
            if base_value is None or run_value is None:
                continue
            change = _relative(run_value, base_value)
            absolute = run_value - base_value
            if change is None:
                continue
            table.append((label, column, base_value, run_value, change))
            if enforce and change > STAGE_REGRESSION and absolute > STAGE_ABS_MS:
                errors.append(
                    f"{label}: {column} +{change * 100:.1f}% и +{absolute:.1f} мс"
                )

    for file_id in sorted(set(base_records) & set(run_records)):
        check_total(file_id, base_records[file_id], run_records[file_id])
        # Стадии по файлам — только в таблицу: ленивая загрузка diar-моделей
        # шумит на десятки мс, порог применяется к агрегату набора.
        check_stages(file_id, base_records[file_id], run_records[file_id], False)
    check_total("aggregate", baseline.get("aggregate") or {}, current.get("aggregate") or {})
    check_stages("aggregate", baseline.get("aggregate") or {}, current.get("aggregate") or {}, True)
    return errors, table


def merge_baseline(baseline: dict, machine_id: str, machine: dict, suite: str) -> dict:
    machines = baseline.setdefault("machines", {})
    existing = machines.get(machine_id)
    if existing and existing.get("suite") not in (None, suite):
        raise ValueError(
            f"машина {machine_id}: baseline снят на suite={existing.get('suite')}, "
            f"текущий {suite}"
        )
    machines[machine_id] = machine
    return baseline


def clean_reps(out: Path) -> None:
    for path in Path(out).glob("rep-*"):
        if path.is_dir():
            shutil.rmtree(path)


def require_release(rows: dict[str, dict]) -> None:
    profiles = {
        str(row.get("calib_profile") or "").strip() for row in rows.values()
    }
    if profiles != {"release"}:
        raise ValueError(
            "baseline снимается только с release-профиля; "
            f"calib_profile={sorted(profiles)}"
        )


def _print_table(records: dict[str, dict]) -> None:
    header = f"{'file_id':48} {'perf_ms':>10} {'calib_ms':>9} {'rtf_perf':>9} {'rtf_norm':>10}"
    print(header)
    for file_id, record in sorted(records.items()):
        values = [
            record.get("perf_ms"),
            record.get("calib_ms"),
            record.get("rtf_perf"),
            record.get("rtf_norm"),
        ]
        cells = ["—" if value is None else f"{value:.4f}" for value in values]
        print(f"{file_id:48} {cells[0]:>10} {cells[1]:>9} {cells[2]:>9} {cells[3]:>10}")


def capture(args: argparse.Namespace) -> int:
    suite = json.loads((HERE / "suites" / f"{args.suite}.json").read_text(encoding="utf-8"))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if args.overwrite:
        clean_reps(out)
    baseline_path = Path(args.baseline)
    baseline: dict = {"schema": 1, "machines": {}}
    if baseline_path.is_file():
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
        baseline.setdefault("schema", 1)
        baseline.setdefault("machines", {})
    fingerprint = machine_fingerprint(args.root)
    existing = (baseline.get("machines") or {}).get(fingerprint["machine_id"])
    if existing and existing.get("suite") not in (None, suite["name"]):
        raise ValueError(
            f"машина {fingerprint['machine_id']}: baseline снят на suite="
            f"{existing.get('suite')}, текущий {suite['name']}"
        )
    app = Path(args.app).resolve()
    reps: list[dict[str, dict]] = []
    for rep in range(1, args.reps + 1):
        run_dir = out / f"rep-{rep:02d}"
        command = [
            sys.executable,
            str(HERE / "run_suite.py"),
            "--suite", suite["name"],
            "--perf",
            "--root", str(Path(args.root).expanduser()),
            "--app", str(app),
            "--out", str(run_dir),
        ]
        if args.overwrite:
            command.append("--overwrite")
        print("+", " ".join(command), flush=True)
        result = subprocess.run(command, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"повтор {rep}: run_suite вернул {result.returncode}")
        rows = read_metrics(run_dir / "metrics.csv")
        require_release(rows)
        for file_id, row in rows.items():
            if row.get("error"):
                raise RuntimeError(f"{file_id}: {row['error']}")
            if _number(row.get("perf_ms")) is None:
                raise RuntimeError(f"{file_id}: нет perf sidecar (MR_PERF)")
        reps.append(rows)

    file_ids = sorted(set(reps[0]))
    for rows in reps[1:]:
        if set(rows) != set(file_ids):
            raise RuntimeError("повторы вернули разные наборы файлов")

    records = {
        file_id: summarize_rows([rows[file_id] for rows in reps])
        for file_id in file_ids
    }
    aggregate = aggregate_records(records)
    machine = {
        "cpu": fingerprint["cpu"],
        "os": fingerprint["os"],
        "arch": fingerprint["arch"],
        "logical_cpus": fingerprint["logical_cpus"],
        "bench_id": fingerprint["bench_id"],
        "calib_version": int(aggregate.get("calib_version") or 1),
        "calib_dim": int(aggregate["calib_dim"]) if aggregate.get("calib_dim") else None,
        "calib_profile": "release",
        "suite": suite["name"],
        "reps": args.reps,
        "records": records,
        "aggregate": aggregate,
    }

    merge_baseline(baseline, fingerprint["machine_id"], machine, suite["name"])
    baseline_path.parent.mkdir(parents=True, exist_ok=True)
    baseline_path.write_text(
        json.dumps(baseline, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    _print_table(records)
    print(
        f"machine_id={fingerprint['machine_id']} calib_ms={aggregate.get('calib_ms')} "
        f"baseline={baseline_path}"
    )
    return 0


def _calib_signature(records: dict[str, dict]) -> tuple[int | None, int | None, set[str]]:
    versions = {
        int(value)
        for record in records.values()
        if (value := _number(record.get("calib_version"))) is not None
    }
    dims = {
        int(value)
        for record in records.values()
        if (value := _number(record.get("calib_dim"))) is not None
    }
    profiles = {
        str(record.get("calib_profile"))
        for record in records.values()
        if record.get("calib_profile")
    }
    version = next(iter(versions)) if len(versions) == 1 else None
    dim = next(iter(dims)) if len(dims) == 1 else None
    return version, dim, profiles


def check(args: argparse.Namespace) -> int:
    baseline_path = Path(args.baseline)
    if not baseline_path.is_file():
        print(f"ошибка: baseline не найден: {baseline_path}", file=sys.stderr)
        return 2
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    fingerprint = machine_fingerprint(args.root)
    machine = (baseline.get("machines") or {}).get(fingerprint["machine_id"])
    if machine is None:
        known = ", ".join(sorted((baseline.get("machines") or {}))) or "нет"
        print(
            f"ошибка: машина {fingerprint['machine_id']} отсутствует в baseline; есть: {known}",
            file=sys.stderr,
        )
        return 2
    try:
        files = collect_run_files([Path(path) for path in args.run])
        records, runs = summarize_runs(files)
    except (OSError, ValueError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    version, dim, profiles = _calib_signature(records)
    if version != machine.get("calib_version") or dim != machine.get("calib_dim"):
        print(
            f"ошибка: калибровка {version}/dim {dim} не совпадает с baseline "
            f"{machine.get('calib_version')}/dim {machine.get('calib_dim')}",
            file=sys.stderr,
        )
        return 2
    if profiles and profiles != {"release"}:
        print(f"ошибка: check только для release, профили: {sorted(profiles)}", file=sys.stderr)
        return 2
    current = {"records": records, "aggregate": aggregate_records(records)}
    expected = {
        "records": machine.get("records") or {},
        "aggregate": machine.get("aggregate") or {},
    }
    errors, table = compare_perf(expected, current)
    header = f"{'file_id':44} {'field':20} {'baseline':>12} {'current':>12} {'diff':>9}"
    print(header)
    for label, field, base_value, run_value, change in table:
        print(
            f"{label:44} {field:20} {base_value:12.4f} {run_value:12.4f} "
            f"{change * 100:8.1f}%"
        )
    if errors:
        for error in errors:
            print(f"регресс: {error}", file=sys.stderr)
        return 1
    print(
        f"ok: machine_id={fingerprint['machine_id']} runs={runs} "
        f"files={len(records)} calib_ms={current['aggregate'].get('calib_ms')}"
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    capture_parser = sub.add_parser("capture", help="снять baseline на текущей машине")
    capture_parser.add_argument("--suite", default="perf")
    capture_parser.add_argument("--reps", type=int, default=3)
    capture_parser.add_argument("--app", type=Path, default=REPO_ROOT / "src-tauri/target/release/podtekst")
    capture_parser.add_argument("--root", type=Path, default=bench_root())
    capture_parser.add_argument("--out", type=Path, required=True)
    capture_parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    capture_parser.add_argument("--overwrite", action="store_true")
    capture_parser.set_defaults(func=capture)

    check_parser = sub.add_parser("check", help="сверить metrics.csv с baseline")
    check_parser.add_argument("--run", type=Path, nargs="+", required=True)
    check_parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    check_parser.add_argument("--root", type=Path, default=bench_root())
    check_parser.set_defaults(func=check)

    args = parser.parse_args(argv)
    if getattr(args, "reps", 1) <= 0:
        print("ошибка: --reps должен быть больше нуля", file=sys.stderr)
        return 2
    try:
        return args.func(args)
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
