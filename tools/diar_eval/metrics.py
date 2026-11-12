#!/usr/bin/env python3
"""Метрики расшифровки, диаризации и атрибуции слов."""

from __future__ import annotations

import argparse
import json
import math
import sys
import tempfile
import unicodedata
import unittest
from pathlib import Path

_JIWER = None
CPWER_VERSION = "speaker-stream-v2"


def _jiwer():
    global _JIWER
    if _JIWER is None:
        try:
            import jiwer
        except ImportError as exc:
            raise RuntimeError(
                "нужен jiwer; установите: "
                "python3 -m pip install -r tools/diar_eval/requirements-eval.txt"
            ) from exc
        _JIWER = jiwer
    return _JIWER


def normalize_text(text: object) -> str:
    if text is None:
        return ""
    value = unicodedata.normalize("NFKC", str(text)).lower().replace("ё", "е")
    chars = [char for char in value if not unicodedata.category(char).startswith("P")]
    return " ".join("".join(chars).split())


def tokenize(text: object) -> list[str]:
    return normalize_text(text).split()


def segment_tokens(segment: dict) -> list[str]:
    tokens: list[str] = []
    for item in segment.get("words") or []:
        if isinstance(item, dict):
            raw = item.get("w", item.get("word", ""))
        else:
            raw = item
        token = normalize_text(raw)
        if token:
            tokens.extend(token.split())
    if tokens:
        return tokens
    return tokenize(segment.get("text", ""))


def segments_text(segments: list[dict]) -> str:
    return " ".join(
        part for part in (" ".join(segment_tokens(segment)) for segment in segments) if part
    )


def speaker_count(segments: list[dict]) -> int:
    return len({str(segment.get("speaker", "")) for segment in segments})


def speaker_count_accuracy(
    reference_counts: list[int | None], hypothesis_counts: list[int | None]
) -> float:
    pairs = [
        (int(ref), int(hyp))
        for ref, hyp in zip(reference_counts, hypothesis_counts)
        if ref is not None and hyp is not None
    ]
    if not pairs:
        return math.nan
    return sum(1 for ref, hyp in pairs if ref == hyp) / len(pairs)


def speaker_count_mae(
    reference_counts: list[int | None], hypothesis_counts: list[int | None]
) -> float:
    values = [
        abs(int(ref) - int(hyp))
        for ref, hyp in zip(reference_counts, hypothesis_counts)
        if ref is not None and hyp is not None
    ]
    if not values:
        return math.nan
    return sum(values) / len(values)


def _counts(reference: str, hypothesis: str, characters: bool) -> dict:
    jiwer = _jiwer()
    if characters:
        output = jiwer.process_characters([reference], [hypothesis])
        ref_units = len(reference)
        return {
            "value": output.cer,
            "errors": int(output.substitutions + output.deletions + output.insertions),
            "ref_units": ref_units,
            "substitutions": int(output.substitutions),
            "deletions": int(output.deletions),
            "insertions": int(output.insertions),
        }
    output = jiwer.process_words([reference], [hypothesis])
    return {
        "value": output.wer,
        "errors": int(output.substitutions + output.deletions + output.insertions),
        "ref_units": len(reference.split()),
        "substitutions": int(output.substitutions),
        "deletions": int(output.deletions),
        "insertions": int(output.insertions),
    }


def wer_details(reference: object, hypothesis: object) -> dict:
    ref = normalize_text(reference)
    hyp = normalize_text(hypothesis)
    details = _counts(ref, hyp, characters=False)
    return {
        "wer": details["value"],
        "errors": details["errors"],
        "ref_words": details["ref_units"],
        "substitutions": details["substitutions"],
        "deletions": details["deletions"],
        "insertions": details["insertions"],
    }


def cer_details(reference: object, hypothesis: object) -> dict:
    ref = normalize_text(reference)
    hyp = normalize_text(hypothesis)
    details = _counts(ref, hyp, characters=True)
    return {
        "cer": details["value"],
        "errors": details["errors"],
        "ref_chars": details["ref_units"],
        "substitutions": details["substitutions"],
        "deletions": details["deletions"],
        "insertions": details["insertions"],
    }


def _speaker_blocks(segments: list[dict]) -> tuple[list[str], dict[str, list[str]]]:
    order: list[str] = []
    blocks: dict[str, list[str]] = {}
    for segment in segments:
        speaker = str(segment.get("speaker", ""))
        tokens = segment_tokens(segment)
        if not tokens:
            continue
        if speaker not in blocks:
            blocks[speaker] = []
            order.append(speaker)
        blocks[speaker].extend(tokens)
    return order, blocks


def _wer_errors(reference: str, hypothesis: str) -> int:
    if not reference:
        return len(hypothesis.split())
    if not hypothesis:
        return len(reference.split())
    output = _jiwer().process_words([reference], [hypothesis])
    return int(output.substitutions + output.deletions + output.insertions)


def cpwer(
    reference_segments: list[dict],
    hypothesis_segments: list[dict],
    max_speakers: int = 8,
) -> dict:
    ref_order, ref_blocks = _speaker_blocks(reference_segments)
    hyp_order, hyp_blocks = _speaker_blocks(hypothesis_segments)
    ref_text = " ".join(" ".join(ref_blocks[speaker]) for speaker in ref_order)
    ref_words = len(ref_text.split())
    n_ref = len(ref_order)
    n_hyp = len(hyp_order)

    if n_ref == 0 or ref_words == 0:
        return {
            "version": CPWER_VERSION,
            "cpwer": math.nan,
            "errors": 0,
            "ref_words": 0,
            "mapping": {},
            "speakers_ref": n_ref,
            "speakers_hyp": n_hyp,
        }
    if max(n_ref, n_hyp) > max_speakers:
        raise ValueError(
            f"cpWER: {max(n_ref, n_hyp)} спикеров, разрешено до {max_speakers}"
        )

    size = max(n_ref, n_hyp)
    references = [" ".join(ref_blocks[speaker]) for speaker in ref_order] + [""] * (size - n_ref)
    hypotheses = [" ".join(hyp_blocks[speaker]) for speaker in hyp_order] + [""] * (size - n_hyp)
    costs = [[_wer_errors(ref, hyp) for hyp in hypotheses] for ref in references]
    assignment = _hungarian_assign(costs)
    best_errors = sum(costs[row][column] for row, column in enumerate(assignment))
    best_mapping = {
        speaker: hyp_order[assignment[row]] if assignment[row] < n_hyp else None
        for row, speaker in enumerate(ref_order)
    }
    return {
        "version": CPWER_VERSION,
        "cpwer": best_errors / ref_words,
        "errors": best_errors,
        "ref_words": ref_words,
        "mapping": best_mapping,
        "speakers_ref": n_ref,
        "speakers_hyp": n_hyp,
    }


def _hungarian_assign(cost: list[list[float]]) -> list[int]:
    if not cost:
        return []
    rows = len(cost)
    cols = len(cost[0])
    if any(len(row) != cols for row in cost):
        raise ValueError("матрица должна быть прямоугольной")
    size = max(rows, cols)
    matrix = [[0.0] * size for _ in range(size)]
    for row in range(rows):
        for col in range(cols):
            matrix[row][col] = float(cost[row][col])

    infinity = float("inf")
    u = [0.0] * (size + 1)
    v = [0.0] * (size + 1)
    p = [0] * (size + 1)
    way = [0] * (size + 1)
    for row in range(1, size + 1):
        p[0] = row
        col0 = 0
        minv = [infinity] * (size + 1)
        used = [False] * (size + 1)
        while True:
            used[col0] = True
            row0 = p[col0]
            delta = infinity
            col1 = 0
            for col in range(1, size + 1):
                if used[col]:
                    continue
                current = matrix[row0 - 1][col - 1] - u[row0] - v[col]
                if current < minv[col]:
                    minv[col] = current
                    way[col] = col0
                if minv[col] < delta:
                    delta = minv[col]
                    col1 = col
            for col in range(size + 1):
                if used[col]:
                    u[p[col]] += delta
                    v[col] -= delta
                else:
                    minv[col] -= delta
            col0 = col1
            if p[col0] == 0:
                break
        while True:
            col1 = way[col0]
            p[col0] = p[col1]
            col0 = col1
            if col0 == 0:
                break

    assignment = [-1] * rows
    for col in range(1, size + 1):
        if p[col] and p[col] <= rows:
            target = col - 1
            assignment[p[col] - 1] = target if target < cols else -1
    return assignment


def _word_speakers(segments: list[dict]) -> tuple[list[str], list[str]]:
    tokens: list[str] = []
    speakers: list[str] = []
    for segment in segments:
        speaker = str(segment.get("speaker", ""))
        for token in segment_tokens(segment):
            tokens.append(token)
            speakers.append(speaker)
    return tokens, speakers


def attribution_accuracy(
    reference_segments: list[dict], hypothesis_segments: list[dict]
) -> dict:
    ref_tokens, ref_speakers = _word_speakers(reference_segments)
    hyp_tokens, hyp_speakers = _word_speakers(hypothesis_segments)
    if not ref_tokens or not hyp_tokens:
        return {"accuracy": math.nan, "correct": 0, "matched": 0}

    output = _jiwer().process_words(
        [" ".join(ref_tokens)], [" ".join(hyp_tokens)]
    )
    ref_order = list(dict.fromkeys(ref_speakers))
    hyp_order = list(dict.fromkeys(hyp_speakers))
    ref_index = {speaker: index for index, speaker in enumerate(ref_order)}
    hyp_index = {speaker: index for index, speaker in enumerate(hyp_order)}
    matrix = [[0] * len(hyp_order) for _ in ref_order]

    for chunk in output.alignments[0]:
        if chunk.type != "equal":
            continue
        for offset in range(chunk.ref_end_idx - chunk.ref_start_idx):
            ref_speaker = ref_speakers[chunk.ref_start_idx + offset]
            hyp_speaker = hyp_speakers[chunk.hyp_start_idx + offset]
            matrix[ref_index[ref_speaker]][hyp_index[hyp_speaker]] += 1

    matched = sum(sum(row) for row in matrix)
    if matched == 0:
        return {"accuracy": math.nan, "correct": 0, "matched": 0}

    try:
        import numpy as np
        from scipy.optimize import linear_sum_assignment

        rows, cols = linear_sum_assignment(-np.asarray(matrix, dtype=float))
        pairs = list(zip(rows.tolist(), cols.tolist()))
    except ImportError:
        assignment = _hungarian_assign(
            [[-float(value) for value in row] for row in matrix]
        )
        pairs = [(row, col) for row, col in enumerate(assignment) if col >= 0]

    correct = sum(matrix[row][col] for row, col in pairs)
    return {
        "accuracy": correct / matched,
        "correct": correct,
        "matched": matched,
    }


def _number(value: object, default: float) -> float:
    try:
        parsed = float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default
    return parsed if math.isfinite(parsed) else default


def _normalize_segments(obj: object, source: Path) -> list[dict]:
    if isinstance(obj, list):
        items = obj
    elif isinstance(obj, dict) and isinstance(obj.get("utterances"), list):
        items = obj["utterances"]
    elif isinstance(obj, dict) and isinstance(obj.get("segments"), list):
        items = obj["segments"]
    else:
        raise ValueError(f"{source}: ожидался список utterances или segments")

    segments: list[dict] = []
    for index, item in enumerate(items):
        if not isinstance(item, dict):
            raise ValueError(f"{source}: сегмент {index} не объект")
        speaker = item.get("speaker")
        if speaker is None:
            speaker = item.get("spk")
        if speaker is None:
            speaker = item.get("channel")
        if speaker is None:
            speaker = "SPEAKER_00"
        start = _number(item.get("start", item.get("start_sec", 0.0)), 0.0)
        end = _number(item.get("end", item.get("end_sec", start)), start)

        words: list[dict] = []
        raw_words = item.get("words")
        if not isinstance(raw_words, list):
            raw_words = []
        for word in raw_words:
            if isinstance(word, dict):
                raw = word.get("w", word.get("word", ""))
                word_start = _number(
                    word.get("start_sec", word.get("start", start)), start
                )
                word_end = _number(word.get("end_sec", word.get("end", word_start)), word_start)
            else:
                raw = word
                word_start = start
                word_end = end
            token = normalize_text(raw)
            if token:
                words.append(
                    {"w": token, "start_sec": word_start, "end_sec": word_end}
                )

        text = str(item.get("text") or "")
        if not text and words:
            text = " ".join(word["w"] for word in words)
        segments.append(
            {
                "speaker": str(speaker),
                "text": text,
                "start": start,
                "end": end,
                "words": words,
            }
        )
    segments.sort(key=lambda segment: (segment["start"], segment["end"]))
    return segments


def load_segments(path: Path | str) -> list[dict]:
    source = Path(path)
    if not source.is_file():
        raise FileNotFoundError(f"{source}: файл не найден")
    if source.suffix.lower() == ".jsonl":
        items = [
            json.loads(line)
            for line in source.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        return _normalize_segments(items, source)
    text = source.read_text(encoding="utf-8")
    try:
        return _normalize_segments(json.loads(text), source)
    except json.JSONDecodeError:
        if source.suffix.lower() in {".json", ".jsonl"}:
            raise
        return [
            {
                "speaker": "SPEAKER_00",
                "text": text,
                "start": 0.0,
                "end": 0.0,
                "words": [],
            }
        ]


def _finite(value: float | None) -> float | None:
    return value if value is not None and math.isfinite(value) else None


def _summary(reference_segments: list[dict], hypothesis_segments: list[dict]) -> dict:
    ref_text = segments_text(reference_segments)
    hyp_text = segments_text(hypothesis_segments)
    wer = wer_details(ref_text, hyp_text)
    cer = cer_details(ref_text, hyp_text)
    speakers_ref = speaker_count(reference_segments)
    speakers_hyp = speaker_count(hypothesis_segments)

    cp: dict | None = None
    error = None
    try:
        cp = cpwer(reference_segments, hypothesis_segments)
    except ValueError as exc:
        error = str(exc)

    attribution = None
    if speakers_ref >= 2 and speakers_hyp >= 1:
        attribution = attribution_accuracy(reference_segments, hypothesis_segments)

    sca = None
    if speakers_ref > 0 and speakers_hyp > 0:
        sca = 1.0 if speakers_ref == speakers_hyp else 0.0
    mae = (
        float(abs(speakers_ref - speakers_hyp))
        if speakers_ref > 0 and speakers_hyp > 0
        else math.nan
    )

    return {
        "WER": wer["wer"],
        "CER": cer["cer"],
        "ref_words": wer["ref_words"],
        "wer_errors": wer["errors"],
        "ref_chars": cer["ref_chars"],
        "cer_errors": cer["errors"],
        "speakers_ref": speakers_ref,
        "speakers_hyp": speakers_hyp,
        "SCA": sca,
        "MAE": mae,
        "cpWER": cp["cpwer"] if cp else math.nan,
        "cpwer_version": CPWER_VERSION,
        "attribution": attribution["accuracy"] if attribution else math.nan,
        "attribution_matched": attribution["matched"] if attribution else 0,
        "attribution_correct": attribution["correct"] if attribution else 0,
        "error": error,
    }


class _SelfTest(unittest.TestCase):
    def test_normalize(self) -> None:
        self.assertEqual(normalize_text("Ёлка, ПРИВЕТ!  Мир"), "елка привет мир")
        self.assertEqual(normalize_text("какой-то, тест"), "какойто тест")
        self.assertEqual(normalize_text("ﬁle"), "file")
        self.assertEqual(normalize_text(None), "")

    def test_wer_cer(self) -> None:
        result = wer_details("a b c d", "a x c d e")
        self.assertAlmostEqual(result["wer"], 0.5)
        self.assertEqual(result["errors"], 2)
        self.assertEqual(result["ref_words"], 4)
        self.assertEqual(wer_details("", ""), {
            "wer": 0.0,
            "errors": 0,
            "ref_words": 0,
            "substitutions": 0,
            "deletions": 0,
            "insertions": 0,
        })
        cer = cer_details("ab cd", "ab xd")
        self.assertAlmostEqual(cer["cer"], 0.2)
        self.assertEqual(cer["ref_chars"], 5)
        self.assertEqual(cer["errors"], 1)

    def test_speaker_counts(self) -> None:
        ref = [2, 3, 3, 2]
        hyp = [2, 3, 2, 2]
        self.assertAlmostEqual(speaker_count_accuracy(ref, hyp), 0.75)
        self.assertAlmostEqual(speaker_count_mae(ref, hyp), 0.25)
        self.assertTrue(math.isnan(speaker_count_accuracy([], [])))

    def test_cpwer_permutation(self) -> None:
        ref = [
            {"speaker": "A", "text": "альфа бета"},
            {"speaker": "B", "text": "гамма дельта"},
        ]
        hyp = [
            {"speaker": "X", "text": "гамма дельта"},
            {"speaker": "Y", "text": "альфа бета"},
        ]
        result = cpwer(ref, hyp)
        self.assertAlmostEqual(result["cpwer"], 0.0)
        self.assertEqual(result["ref_words"], 4)

    def test_cpwer_missing_word(self) -> None:
        ref = [
            {"speaker": "A", "text": "альфа бета"},
            {"speaker": "B", "text": "гамма дельта"},
        ]
        hyp = [
            {"speaker": "X", "text": "альфа бета"},
            {"speaker": "Y", "text": "гамма"},
        ]
        result = cpwer(ref, hyp)
        self.assertAlmostEqual(result["cpwer"], 0.25)
        self.assertEqual(result["errors"], 1)

    def test_cpwer_too_many_speakers(self) -> None:
        ref = [{"speaker": f"S{i}", "text": "слово"} for i in range(9)]
        hyp = [{"speaker": "X", "text": "слово"}]
        with self.assertRaises(ValueError):
            cpwer(ref, hyp)

    def test_cpwer_extra_and_empty_speakers(self) -> None:
        ref = [{"speaker": "A", "text": "a b"}]
        hyp = [
            {"speaker": "X", "text": "a"},
            {"speaker": "Y", "text": "b"},
            {"speaker": "Z", "text": "c"},
        ]
        self.assertAlmostEqual(cpwer(ref, hyp)["cpwer"], 1.5)
        self.assertAlmostEqual(cpwer(ref, [])["cpwer"], 1.0)

    def test_cpwer_collapsed_identical_speaker_streams_are_penalized(self) -> None:
        ref = [
            {"speaker": "A", "text": "альфа бета"},
            {"speaker": "B", "text": "альфа бета"},
        ]
        hyp = [{"speaker": "X", "text": "альфа бета альфа бета"}]
        result = cpwer(ref, hyp)
        self.assertEqual(result["errors"], 4)
        self.assertEqual(result["ref_words"], 4)
        self.assertEqual(result["cpwer"], 1.0)
        self.assertEqual(result["version"], CPWER_VERSION)

    def test_cpwer_cannot_align_words_across_speaker_boundaries(self) -> None:
        ref = [{"speaker": "A", "text": "альфа"}, {"speaker": "B", "text": "бета гамма"}]
        hyp = [{"speaker": "X", "text": "альфа бета"}, {"speaker": "Y", "text": "гамма"}]
        self.assertEqual(cpwer(ref, hyp)["errors"], 2)
        self.assertAlmostEqual(cpwer(ref, hyp)["cpwer"], 2 / 3)

    def test_cpwer_split_stream_counts_missing_and_extra_words(self) -> None:
        ref = [{"speaker": "A", "text": "альфа бета"}]
        hyp = [{"speaker": "X", "text": "альфа"}, {"speaker": "Y", "text": "бета"}]
        self.assertEqual(cpwer(ref, hyp)["errors"], 2)
        self.assertEqual(cpwer(ref, hyp)["cpwer"], 1.0)

    def test_attribution(self) -> None:
        ref = [
            {"speaker": "A", "text": "a b"},
            {"speaker": "B", "text": "c d"},
        ]
        hyp = [{"speaker": "X", "text": "a b c d"}]
        result = attribution_accuracy(ref, hyp)
        self.assertEqual(result["matched"], 4)
        self.assertEqual(result["correct"], 2)
        self.assertAlmostEqual(result["accuracy"], 0.5)

        swapped = [
            {"speaker": "X", "text": "c d"},
            {"speaker": "Y", "text": "a b"},
        ]
        self.assertAlmostEqual(attribution_accuracy(ref, swapped)["accuracy"], 1.0)

    def test_hungarian_fallback(self) -> None:
        cost = [[4.0, 1.0, 3.0], [2.0, 0.0, 5.0], [3.0, 2.0, 2.0]]
        assignment = _hungarian_assign(cost)
        total = sum(cost[row][col] for row, col in enumerate(assignment))
        self.assertAlmostEqual(total, 5.0)

        rectangular = [[4.0, 1.0, 3.0], [2.0, 0.0, 5.0]]
        assignment = _hungarian_assign(rectangular)
        total = sum(rectangular[row][col] for row, col in enumerate(assignment) if col >= 0)
        self.assertAlmostEqual(total, 3.0)

    def test_segment_text_and_loader(self) -> None:
        segments = [
            {"speaker": "A", "text": "Ёлка!", "words": []},
            {"speaker": "B", "text": "", "words": [{"w": "Мир"}]},
        ]
        self.assertEqual(segments_text(segments), "елка мир")
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ref.transcript.json"
            path.write_text(
                json.dumps(
                    {
                        "utterances": [
                            {"speaker": "A", "start": 0.0, "end": 0.5, "text": "Ёлка!"}
                        ]
                    }
                ),
                encoding="utf-8",
            )
            loaded = load_segments(path)
            self.assertEqual(segments_text(loaded), "елка")
            self.assertEqual(loaded[0]["speaker"], "A")

            plain = Path(tmp) / "ref.txt"
            plain.write_text("Привет, мир!", encoding="utf-8")
            loaded_plain = load_segments(plain)
            self.assertEqual(segments_text(loaded_plain), "привет мир")
            self.assertEqual(speaker_count(loaded_plain), 1)


def _self_test() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(_SelfTest)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def _emit(summary: dict, as_json: bool) -> None:
    if as_json:
        payload = {
            key: (_finite(value) if isinstance(value, float) else value)
            for key, value in summary.items()
        }
        print(json.dumps(payload, ensure_ascii=False, indent=2))
        return
    print(
        f"WER={summary['WER'] * 100:.2f}% "
        f"({summary['wer_errors']}/{summary['ref_words']}) "
        f"CER={summary['CER'] * 100:.2f}% "
        f"({summary['cer_errors']}/{summary['ref_chars']})"
    )
    if math.isfinite(summary["cpWER"]):
        print(f"cpWER={summary['cpWER'] * 100:.2f}% version={summary['cpwer_version']}")
    if math.isfinite(summary["attribution"]):
        print(
            f"attribution={summary['attribution'] * 100:.2f}% "
            f"({summary['attribution_correct']}/{summary['attribution_matched']})"
        )
    else:
        print("attribution=— (нужен эталон с метками минимум двух спикеров)")
    sca = "—" if summary["SCA"] is None else f"{summary['SCA']:.2f}"
    mae = "—" if not math.isfinite(summary["MAE"]) else f"{summary['MAE']:.2f}"
    print(
        f"speakers: ref={summary['speakers_ref']} hyp={summary['speakers_hyp']} "
        f"SCA={sca} MAE={mae}"
    )
    if summary["error"]:
        print(f"предупреждение: {summary['error']}", file=sys.stderr)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", type=Path, help="эталон: .transcript.json, .jsonl или .txt")
    parser.add_argument("--hyp", type=Path, help="гипотеза: .transcript.json")
    parser.add_argument("--json", action="store_true", help="вывод в JSON")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return _self_test()
    if args.ref is None or args.hyp is None:
        parser.error("нужны --ref и --hyp")

    try:
        reference = load_segments(args.ref)
        hypothesis = load_segments(args.hyp)
        summary = _summary(reference, hypothesis)
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    _emit(summary, args.json)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
