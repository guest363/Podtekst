#!/usr/bin/env python3
"""Локальный регрессионный прогон синтетических встреч."""

import argparse
import csv
import math
import json
import os
import subprocess
import sys
from pathlib import Path

from synth_meetings import DEFAULT_SEED, generate
from prepare_fleurs_speakers import digest, validate


def check_metrics(rows: dict, scenarios: list, baseline: dict | None = None) -> list[str]:
    errors = []
    for language, speakers, overlap, gain in scenarios:
        name = f"synth-{language}-{speakers}spk-60s-ov{overlap}"
        if gain:
            name += "-spk2-18db-quieter"
        row = rows.get(name)
        if not row or row["exit_code"] != "0" or row["error"]:
            errors.append(f"{name}: ошибка выполнения")
            continue
        try:
            der = float(row["DER"])
            count = int(row["speakers_hyp"])
        except ValueError:
            errors.append(f"{name}: отсутствуют метрики")
            continue
        if not math.isfinite(der):
            errors.append(f"{name}: DER не является числом")
        elif language == "en" and der > 0.15:
            errors.append(f"{name}: DER {der:.2%} > 15%")
        elif speakers in (3, 4) and baseline and der > float(baseline[name]["DER"]) + 0.000001:
            errors.append(f"{name}: DER {der:.2%} > baseline")
        if language == "ru" and speakers == 2 and count != 2:
            errors.append(f"{name}: обнаружено спикеров {count}, ожидалось 2")
        if speakers in (3, 4) and baseline and abs(count - speakers) > abs(int(baseline[name]["speakers_hyp"]) - speakers):
            errors.append(f"{name}: ошибка числа спикеров больше baseline")
        print(f"{name}: DER={der:.2%}, speakers={count}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--baseline-from-csv", type=Path)
    args = parser.parse_args()
    if os.environ.get("MR_DIAR_BENCH") != "1":
        print("MR_DIAR_BENCH != 1: пропуск")
        return 0
    root = Path(os.environ["MR_DIAR_BENCH_ROOT"]).resolve()
    validate(root)
    out = root / "regression-verified"
    out.mkdir(parents=True, exist_ok=True)
    manifest = out / "manifest.json"
    manifest.unlink(missing_ok=True)
    scenarios = [("en", 2, 0, 0), ("ru", 2, 0, 0), ("ru", 3, 5, 0),
                 ("ru", 4, 15, 0), ("en", 2, 0, -18), ("ru", 2, 0, -18)]
    for language, speakers, overlap, gain in scenarios:
        name = f"synth-{language}-{speakers}spk-60s-ov{overlap}"
        if gain:
            name += "-spk2-18db-quieter"
        generate(root / "diar-regression-clips" / ("en_us" if language == "en" else "ru_ru"),
                 out / name, speakers, 60, overlap, DEFAULT_SEED, language=language,
                 name=name, manifest_out=manifest, second_speaker_db=gain)
    if args.baseline_from_csv:
        with args.baseline_from_csv.open(newline="", encoding="utf-8") as file:
            rows = {row["file_id"]: row for row in csv.DictReader(file)}
        names = [f"synth-ru-{speakers}spk-60s-ov{overlap}" for speakers, overlap in ((3, 5), (4, 15))]
        if any(name not in rows or rows[name]["exit_code"] != "0" or rows[name]["error"] or not math.isfinite(float(rows[name]["DER"])) for name in names):
            raise ValueError("неполные метрики baseline")
        payload = {"proof_sha256": digest((root / "diar-regression-clips/proof.json").read_bytes()), "metrics": {name: {"DER": float(rows[name]["DER"]), "speakers_hyp": int(rows[name]["speakers_hyp"])} for name in names}}
        destination = out / "baseline.json"
        destination.write_text(json.dumps(payload, indent=2), encoding="utf-8")
        print(f"Baseline записан: {destination}")
        return 0
    if args.prepare_only:
        print(f"Манифест подготовлен: {manifest}")
        return 0
    if args.baseline is None:
        parser.error("для проверки нужен --baseline, измеренный на подтверждённых клипах")
    baseline_doc = json.loads(args.baseline.read_text(encoding="utf-8"))
    if baseline_doc["proof_sha256"] != digest((root / "diar-regression-clips/proof.json").read_bytes()):
        raise ValueError("baseline относится к другой выборке")
    baseline = baseline_doc["metrics"]
    app = Path(os.environ["MR_DIAR_BENCH_APP"]).resolve()
    subprocess.run([sys.executable, str(Path(__file__).with_name("run_batch.py")),
                    "--manifest", str(manifest), "--out", str(out / "run"),
                    "--app", str(app), "--timeout", "900"], check=True)
    with (out / "run" / "metrics.csv").open(newline="", encoding="utf-8") as file:
        rows = {row["file_id"]: row for row in csv.DictReader(file)}
    errors = check_metrics(rows, scenarios, baseline)
    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
