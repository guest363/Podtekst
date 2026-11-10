#!/usr/bin/env python3
"""DER по паре RTTM через pyannote.metrics."""

from __future__ import annotations

import argparse
import math
import sys
import tempfile
import warnings
from pathlib import Path


def _require_pyannote():
    try:
        from pyannote.database.util import load_rttm
        from pyannote.metrics.diarization import DiarizationErrorRate
    except ImportError as exc:
        raise RuntimeError(
            "нужен pyannote.metrics; установите: "
            "python3 -m pip install -r tools/diar_eval/requirements-eval.txt"
        ) from exc
    return load_rttm, DiarizationErrorRate


def _rttm_files(path: Path) -> list[Path]:
    if path.is_dir():
        files = sorted(path.glob("*.rttm"))
        if not files:
            raise FileNotFoundError(f"{path}: нет файлов *.rttm")
        return files
    if path.is_file():
        return [path]
    raise FileNotFoundError(f"{path}: файл или каталог не найден")


def load_rttm_set(path: Path) -> dict:
    load_rttm, _ = _require_pyannote()
    result: dict = {}
    for file in _rttm_files(path):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            loaded = load_rttm(str(file))
        for uri, annotation in loaded.items():
            if uri in result:
                raise ValueError(f"повторяющийся file id {uri!r} в {path}")
            result[uri] = annotation
    return result


def _der_from_components(total: float, correct: float, false_alarm: float) -> float:
    # DER = (miss + confusion + false alarm) / total; correct already excludes
    # miss/confusion, but false alarm is extra speech, not a subtraction.
    errors = max(0.0, total - correct) + false_alarm
    if total > 0:
        return errors / total
    return 0.0 if errors == 0.0 else math.nan


def compute_der(
    reference_path: Path,
    hypothesis_path: Path,
    collar: float = 0.0,
    skip_overlap: bool = False,
) -> dict:
    _, DiarizationErrorRate = _require_pyannote()
    from pyannote.core import Annotation

    reference = load_rttm_set(Path(reference_path))
    hypothesis = load_rttm_set(Path(hypothesis_path))
    uris = list(reference)
    uris.extend(uri for uri in hypothesis if uri not in reference)

    per_file: dict[str, dict] = {}
    total = correct = miss = false_alarm = confusion = 0.0
    for uri in uris:
        ref = reference.get(uri, Annotation(uri=uri))
        hyp = hypothesis.get(uri, Annotation(uri=uri))
        metric = DiarizationErrorRate(collar=collar, skip_overlap=skip_overlap)
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            detailed = metric(ref, hyp, detailed=True)
        file_total = float(detailed.get("total", 0.0))
        file_correct = float(detailed.get("correct", 0.0))
        file_miss = float(detailed.get("missed detection", 0.0))
        file_fa = float(detailed.get("false alarm", 0.0))
        file_conf = float(detailed.get("confusion", 0.0))
        per_file[uri] = {
            "ref_sec": file_total,
            "correct_sec": file_correct,
            "miss_sec": file_miss,
            "false_alarm_sec": file_fa,
            "confusion_sec": file_conf,
            "der": _der_from_components(file_total, file_correct, file_fa),
        }
        total += file_total
        correct += file_correct
        miss += file_miss
        false_alarm += file_fa
        confusion += file_conf

    return {
        "der": _der_from_components(total, correct, false_alarm),
        "ref_sec": total,
        "correct_sec": correct,
        "miss_sec": miss,
        "false_alarm_sec": false_alarm,
        "confusion_sec": confusion,
        "per_file": per_file,
        "collar": collar,
        "skip_overlap": skip_overlap,
    }


def _percent(value: float) -> str:
    return "—" if math.isnan(value) else f"{value * 100:.2f}%"


def _print_result(result: dict, per_file: bool) -> None:
    if per_file:
        print(f"{'file':<32} {'ref_s':>8} {'DER':>8} {'miss':>8} {'FA':>8} {'conf':>8}")
        for uri, item in result["per_file"].items():
            print(
                f"{uri[:32]:<32} {item['ref_sec']:>8.2f} "
                f"{_percent(item['der']):>8} "
                f"{_percent(item['miss_sec'] / item['ref_sec'] if item['ref_sec'] else math.nan):>8} "
                f"{_percent(item['false_alarm_sec'] / item['ref_sec'] if item['ref_sec'] else math.nan):>8} "
                f"{_percent(item['confusion_sec'] / item['ref_sec'] if item['ref_sec'] else math.nan):>8}"
            )
    print(
        f"files={len(result['per_file'])} ref_speech={result['ref_sec']:.2f}s "
        f"collar={result['collar']:.2f} skip_overlap={result['skip_overlap']}"
    )
    ref_sec = result["ref_sec"]
    print(
        f"DER={_percent(result['der'])} "
        f"miss={_percent(result['miss_sec'] / ref_sec if ref_sec else math.nan)} "
        f"false_alarm={_percent(result['false_alarm_sec'] / ref_sec if ref_sec else math.nan)} "
        f"confusion={_percent(result['confusion_sec'] / ref_sec if ref_sec else math.nan)}"
    )


def _self_test() -> int:
    assert abs(_der_from_components(3.0, 2.0, 0.0) - 1.0 / 3.0) < 1e-12
    assert abs(_der_from_components(3.0, 2.0, 1.0) - 2.0 / 3.0) < 1e-12
    assert _der_from_components(0.0, 0.0, 0.0) == 0.0
    assert math.isnan(_der_from_components(0.0, 0.0, 1.0))

    ref_text = (
        "SPEAKER one 1 0.000 2.000 <NA> <NA> A <NA> <NA>\n"
        "SPEAKER one 1 3.000 1.000 <NA> <NA> B <NA> <NA>\n"
    )
    hyp_text = (
        "SPEAKER one 1 0.000 2.000 <NA> <NA> X <NA> <NA>\n"
        "SPEAKER one 1 3.000 1.000 <NA> <NA> X <NA> <NA>\n"
    )
    with tempfile.TemporaryDirectory() as tmp:
        ref = Path(tmp) / "ref.rttm"
        hyp = Path(tmp) / "hyp.rttm"
        ref.write_text(ref_text, encoding="utf-8")
        hyp.write_text(hyp_text, encoding="utf-8")
        try:
            result = compute_der(ref, hyp)
        except RuntimeError as exc:
            print(f"self-test: DER пропущен ({exc})")
        else:
            assert abs(result["der"] - 1.0 / 3.0) < 1e-9, result["der"]
            assert abs(result["ref_sec"] - 3.0) < 1e-9
            assert abs(result["per_file"]["one"]["confusion_sec"] - 1.0) < 1e-9

            empty = Path(tmp) / "empty.rttm"
            empty.write_text("", encoding="utf-8")
            full_miss = compute_der(ref, empty)
            assert abs(full_miss["der"] - 1.0) < 1e-9, full_miss["der"]

            second = Path(tmp) / "second.rttm"
            second.write_text(
                "SPEAKER two 1 0.000 1.000 <NA> <NA> A <NA> <NA>\n",
                encoding="utf-8",
            )
            union = compute_der(second, hyp)
            assert set(union["per_file"]) == {"one", "two"}, union["per_file"]

    print("self-test ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", nargs="?", type=Path, help="RTTM эталона, файл или каталог")
    parser.add_argument("hypothesis", nargs="?", type=Path, help="RTTM гипотезы, файл или каталог")
    parser.add_argument("--collar", type=float, default=0.0)
    parser.add_argument("--skip-overlap", action="store_true")
    parser.add_argument("--per-file", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    if args.reference is None or args.hypothesis is None:
        parser.error("нужны reference и hypothesis")

    try:
        result = compute_der(
            args.reference,
            args.hypothesis,
            collar=args.collar,
            skip_overlap=args.skip_overlap,
        )
    except (OSError, ValueError, RuntimeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    _print_result(result, args.per_file)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
