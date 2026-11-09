#!/usr/bin/env python3
"""WER/CER после нормализации чисел и латиницы, не меняя raw-метрику."""

from __future__ import annotations

import argparse
import csv
import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

import metrics

NORM_METHOD = "numbers+latin+vowel-fold"
NUM = "<num>"

# Числительные 0-1000, тысячи, проценты и слова десятичных дробей: эталон Golos
# пишет числа словами, гипотеза - цифрами.
NUMERAL_WORDS = frozenset(
    """
    ноль нуль один одна одно два две три четыре пять шесть семь восемь девять
    десять одиннадцать двенадцать тринадцать четырнадцать пятнадцать
    шестнадцать семнадцать восемнадцать девятнадцать
    двадцать тридцать сорок пятьдесят шестьдесят семьдесят восемьдесят девяносто
    сто двести триста четыреста пятьсот шестьсот семьсот восемьсот девятьсот
    тысяча тысячи тысяч тысяче тысячу тысячам тысячами тысячах
    процент процента проценту процентом проценте проценты процентов процентам
    процентами процентах
    целая целые целых десятая десятые десятых сотая сотые сотых
    """.split()
)

# Транслитерация нужна, чтобы сравнивать латинские бренды с кириллицей;
# точность чтения не важна, важна устойчивость к одной и той же подмене.
LATIN_TO_CYRILLIC = {
    "a": "а",
    "b": "б",
    "c": "ц",
    "d": "д",
    "e": "е",
    "f": "ф",
    "g": "г",
    "h": "х",
    "i": "и",
    "j": "дж",
    "k": "к",
    "l": "л",
    "m": "м",
    "n": "н",
    "o": "о",
    "p": "п",
    "q": "к",
    "r": "р",
    "s": "с",
    "t": "т",
    "u": "у",
    "v": "в",
    "w": "в",
    "x": "кс",
    "y": "й",
    "z": "з",
}

VOWELS = frozenset("аеёиоуыэюя")


def normalize_numbers(text: str) -> str:
    """Заменить числа и числительные на NUM, схлопнув соседние."""
    tokens: list[str] = []
    inside_number = False
    for token in text.split():
        numeric = token in NUMERAL_WORDS or any(char.isdigit() for char in token)
        if numeric:
            if not inside_number:
                tokens.append(NUM)
                inside_number = True
            continue
        inside_number = False
        tokens.append(token)
    return " ".join(tokens)


def transliterate_latin(text: str) -> str:
    """Посимвольно перевести латиницу в кириллицу, кириллицу не трогая."""
    return " ".join(
        token
        if token == NUM
        else "".join(LATIN_TO_CYRILLIC.get(char, char) for char in token)
        for token in text.split()
    )


def fold_vowels(text: str) -> str:
    """Огрубить вокалическую структуру слова: стянуть подряд идущие гласные
    и убрать конечную гласную.

    Без этого `home` и `хоум` не сходятся: транслитерация даёт `хоме` и `хоум`.
    """
    tokens: list[str] = []
    for token in text.split():
        folded: list[str] = []
        for char in token:
            if char in VOWELS and folded and folded[-1] in VOWELS:
                continue
            folded.append(char)
        if len(folded) > 1 and folded[-1] in VOWELS:
            folded.pop()
        tokens.append("".join(folded) or token)
    return " ".join(tokens)


def normalize(text: object) -> str:
    """Полный конвейер: базовая нормализация, числа, латиница, огласовка."""
    return fold_vowels(
        transliterate_latin(normalize_numbers(metrics.normalize_text(text)))
    )


def normalize_pair(reference: object, hypothesis: object) -> tuple[str, str]:
    """Нормализовать эталон и гипотезу одним конвейером."""
    return normalize(reference), normalize(hypothesis)


def manifest_index(records: list[dict]) -> dict[str, dict]:
    """Индекс записей по id, file_id, имени аудио и dataset/subset/имя.

    Неоднозначные ключи выбрасываются: молча привязать строку к чужой записи
    хуже, чем не найти её вовсе.
    """
    index: dict[str, dict] = {}
    ambiguous: set[str] = set()
    for record in records:
        for key in _record_keys(record):
            if key in index:
                ambiguous.add(key)
            index.setdefault(key, record)
    for key in ambiguous:
        index.pop(key, None)
    return index


def _record_keys(record: dict) -> list[str]:
    keys = [
        str(record[field]).strip()
        for field in ("id", "file_id")
        if str(record.get(field) or "").strip()
    ]
    audio = str(record.get("audio") or "").strip()
    if audio:
        stem = Path(audio).stem
        keys.append(stem)
        dataset = str(record.get("dataset") or "").strip()
        subset = str(record.get("subset") or "").strip()
        if dataset and subset:
            keys.extend([f"{dataset}/{subset}/{stem}", f"{dataset}/{stem}"])
    return keys


def _load_manifest(path: Path) -> list[dict]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    records = payload.get("records") if isinstance(payload, dict) else payload
    if not isinstance(records, list) or not all(isinstance(item, dict) for item in records):
        raise ValueError(f"{path}: ожидался список записей или объект с records")
    return records


def _read_csv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as handle:
        return [dict(row) for row in csv.DictReader(handle)]


def _header(path: Path) -> list[str]:
    with path.open(encoding="utf-8", newline="") as handle:
        return next(csv.reader(handle), [])


def _write_csv(path: Path, rows: list[dict], fieldnames: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames, extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def _paths(row: dict, index: dict[str, dict], data_root: Path) -> tuple[Path, Path]:
    file_id = str(row.get("file_id") or "").strip()
    record = index.get(file_id) or index.get(file_id.rsplit("/", 1)[-1])
    if record is None:
        raise ValueError(f"нет записи {file_id!r} в манифесте")
    ref_text = str(record.get("ref_text") or "").strip()
    if not ref_text:
        raise ValueError("в записи манифеста нет ref_text")
    transcript = str(row.get("transcript") or "").strip()
    if not transcript:
        raise ValueError("в metrics.csv нет пути к транскрипту")
    return data_root / ref_text, Path(transcript)


def process(metrics_path: Path, manifest_path: Path, data_root: Path) -> list[dict]:
    rows = _read_csv(metrics_path)
    if "WER" not in _header(metrics_path):
        raise ValueError(f"{metrics_path}: нет колонки WER")
    index = manifest_index(_load_manifest(manifest_path))
    for row in rows:
        row["WER_norm"] = ""
        row["CER_norm"] = ""
        row["norm_method"] = ""
        if not str(row.get("WER") or "").strip():
            continue
        try:
            ref_path, hyp_path = _paths(row, index, data_root)
            reference = metrics.segments_text(metrics.load_segments(ref_path))
            hypothesis = metrics.segments_text(metrics.load_segments(hyp_path))
            ref_norm, hyp_norm = normalize_pair(reference, hypothesis)
            word = metrics.wer_details(ref_norm, hyp_norm)
            char = metrics.cer_details(ref_norm, hyp_norm)
        except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
            row["norm_method"] = f"error: {exc}"
            print(f"ошибка: {row.get('file_id')}: {exc}", file=sys.stderr)
            continue
        row["WER_norm"] = f"{word['wer']:.6f}"
        row["CER_norm"] = f"{char['cer']:.6f}"
        row["norm_method"] = NORM_METHOD
        # Счётчики для micro-агрегата: в CSV не пишутся (extrasaction="ignore").
        row["_norm_counts"] = {
            "wer_errors": word["errors"],
            "ref_words": word["ref_words"],
            "cer_errors": char["errors"],
            "ref_chars": char["ref_chars"],
        }
    return rows


def _number(value: object) -> float | None:
    try:
        parsed = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None
    return parsed if math.isfinite(parsed) else None


def _micro(rows: list[dict], errors_key: str, units_key: str, norm: bool) -> float:
    errors = 0.0
    units = 0.0
    for row in rows:
        if norm:
            if "_norm_counts" not in row:
                continue
            errors += row["_norm_counts"][errors_key]
            units += row["_norm_counts"][units_key]
            continue
        top = _number(row.get(errors_key))
        bottom = _number(row.get(units_key))
        if top is None or bottom is None or bottom <= 0:
            continue
        errors += top
        units += bottom
    return errors / units if units > 0 else math.nan


def _group_key(row: dict) -> str:
    dataset = str(row.get("dataset") or "unknown")
    subset = str(row.get("subset") or "")
    return f"{dataset}/{subset}" if subset else dataset


def summarize(rows: list[dict]) -> list[dict]:
    groups: dict[str, list[dict]] = {}
    for row in rows:
        groups.setdefault(_group_key(row), []).append(row)
    return [
        {
            "group": key,
            "files": len(items),
            "wer_raw": _micro(items, "wer_errors", "ref_words", False),
            "wer_norm": _micro(items, "wer_errors", "ref_words", True),
            "cer_raw": _micro(items, "cer_errors", "ref_chars", False),
            "cer_norm": _micro(items, "cer_errors", "ref_chars", True),
        }
        for key, items in sorted(groups.items())
    ]


def _rate(value: float) -> str:
    return "—" if not math.isfinite(value) else f"{value * 100:.2f}%"


def print_summary(groups: list[dict]) -> None:
    header = (
        f"{'набор':<26}{'файлов':>7}{'WER raw':>10}"
        f"{'WER norm':>10}{'CER raw':>10}{'CER norm':>10}"
    )
    print(header)
    for item in groups:
        print(
            f"{item['group']:<26}{item['files']:>7}"
            f"{_rate(item['wer_raw']):>10}{_rate(item['wer_norm']):>10}"
            f"{_rate(item['cer_raw']):>10}{_rate(item['cer_norm']):>10}"
        )


class _SelfTest(unittest.TestCase):
    def _wer(self, reference: str, hypothesis: str) -> float:
        ref, hyp = normalize_pair(reference, hypothesis)
        return metrics.wer_details(ref, hyp)["wer"]

    def test_numbers(self) -> None:
        self.assertEqual(
            self._wer(
                "В 21:00 выставь яркость на 70%",
                "в двадцать один ноль ноль выставь яркость на семьдесят процентов",
            ),
            0.0,
        )
        self.assertEqual(
            self._wer("866 663", "восемьсот шестьдесят шесть шестьсот шестьдесят три"),
            0.0,
        )
        self.assertEqual(normalize("телефон 866 663"), f"телефон {NUM}")

    def test_word_boundaries(self) -> None:
        self.assertEqual(normalize_numbers("стол дома"), "стол дома")
        self.assertEqual(normalize_numbers("одиннадцать"), NUM)
        self.assertEqual(normalize_numbers("одиннадцати"), "одиннадцати")
        self.assertEqual(normalize_numbers("эпизод двадцать пять сезона"), f"эпизод {NUM} сезона")

    def test_latin(self) -> None:
        self.assertEqual(self._wer("joy home", "джой хоум"), 0.0)
        self.assertEqual(normalize("youtube"), "йотуб")
        self.assertEqual(normalize("привет"), "привет")

    def test_plain_text_survives(self) -> None:
        self.assertEqual(normalize_numbers("альфа бета гамма"), "альфа бета гамма")
        self.assertEqual(transliterate_latin("альфа бета гамма"), "альфа бета гамма")
        self.assertEqual(self._wer("альфа бета", "альфа бета"), 0.0)
        self.assertAlmostEqual(self._wer("альфа бета", "альфа гамма"), 0.5)

    def test_cli_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "ref.json").write_text(
                json.dumps([{"speaker": "A", "text": "в двадцать один ноль ноль"}]),
                encoding="utf-8",
            )
            (root / "hyp.json").write_text(
                json.dumps({"utterances": [{"speaker": "X", "text": "в 21:00"}]}),
                encoding="utf-8",
            )
            (root / "manifest.json").write_text(
                json.dumps(
                    {
                        "records": [
                            {
                                "file_id": "ds/x/one",
                                "audio": "one.wav",
                                "dataset": "ds",
                                "subset": "x",
                                "ref_text": "ref.json",
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            (root / "metrics.csv").write_text(
                "dataset,subset,file_id,WER,wer_errors,ref_words,transcript\n"
                f"ds,x,ds/x/one,1.000000,5,5,{root / 'hyp.json'}\n",
                encoding="utf-8",
            )
            rows = process(root / "metrics.csv", root / "manifest.json", root)
            self.assertEqual(rows[0]["WER_norm"], "0.000000")
            self.assertEqual(rows[0]["norm_method"], NORM_METHOD)
            group = summarize(rows)[0]
            self.assertAlmostEqual(group["wer_raw"], 1.0)
            self.assertAlmostEqual(group["wer_norm"], 0.0)


def _self_test() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(_SelfTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        return 1
    print("self-test ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metrics", type=Path, help="metrics.csv прогона")
    parser.add_argument("--manifest", type=Path, help="манифест с ref_text")
    parser.add_argument(
        "--data-root",
        type=Path,
        help="корень относительных путей манифеста; по умолчанию каталог манифеста",
    )
    parser.add_argument("--out", type=Path, help="куда писать CSV; по умолчанию metrics_norm.csv")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    if args.metrics is None or args.manifest is None:
        parser.error("нужны --metrics и --manifest")

    out = args.out or args.metrics.with_name("metrics_norm.csv")
    data_root = args.data_root or args.manifest.parent

    try:
        rows = process(args.metrics, args.manifest, data_root)
        fieldnames = _header(args.metrics)
        for name in ("WER_norm", "CER_norm", "norm_method"):
            if name not in fieldnames:
                fieldnames.append(name)
        _write_csv(out, rows, fieldnames)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2

    print_summary(summarize(rows))
    sys.stdout.flush()
    failed = sum(1 for row in rows if row["norm_method"].startswith("error:"))
    print(f"\nзаписано: {out}", file=sys.stderr)
    if failed:
        print(f"строк без нормализации: {failed}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
