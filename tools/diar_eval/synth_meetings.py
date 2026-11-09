#!/usr/bin/env python3
"""Синтетические встречи из клипов FLEURS: эталон для диаризации, cpWER и attribution.

Один клип FLEURS = один диктор с уникальным голосом. Встреча собирается
чередованием его реплик, поэтому разных дикторов внутри одного сегмента нет -
так эталон RTTM и текст остаются точными. Длительность и перекрытие задаются
в секундах и процентах от длительности предыдущей реплики: доля перекрытия
в речи получается близкой к запрошенной.
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf

SAMPLE_RATE = 16_000
PAUSE_SEC = (0.2, 0.5)
MIN_TAIL_SEC = 1.5
DEFAULT_SEED = 20261109


@dataclass
class Clip:
    path: Path
    text: str
    frames: int

    @property
    def duration(self) -> float:
        return self.frames / SAMPLE_RATE


@dataclass
class Turn:
    speaker: int
    start: int
    end: int
    text: str

    @property
    def start_sec(self) -> float:
        return self.start / SAMPLE_RATE

    @property
    def end_sec(self) -> float:
        return self.end / SAMPLE_RATE


def _read_samples(path: Path) -> np.ndarray:
    data, rate = sf.read(str(path), dtype="float32", always_2d=True)
    mono = data.mean(axis=1)
    if rate != SAMPLE_RATE and mono.size:
        count = int(round(mono.size * SAMPLE_RATE / rate))
        mono = np.interp(
            np.arange(count) * (rate / SAMPLE_RATE), np.arange(mono.size), mono
        ).astype(np.float32)
    return mono


def _ref_text(path: Path, tsv: dict[str, str]) -> str:
    candidate = path.with_suffix(".json")
    if not candidate.is_file():
        candidate = path.parent.parent / "ref" / f"{path.stem}.json"
    if candidate.is_file():
        try:
            doc = json.loads(candidate.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            doc = None
        items = doc if isinstance(doc, list) else [doc] if isinstance(doc, dict) else []
        text = " ".join(
            str(item.get("text") or "") for item in items if isinstance(item, dict)
        )
        if text.strip():
            return " ".join(text.split())
    if path.stem in tsv:
        return tsv[path.stem]
    txt = path.with_suffix(".txt")
    return " ".join(txt.read_text(encoding="utf-8").split()) if txt.is_file() else ""


def _read_tsv(clips_dir: Path) -> dict[str, str]:
    for candidate in (clips_dir / "fleurs.tsv", clips_dir.parent / "fleurs.tsv"):
        if not candidate.is_file():
            continue
        rows: dict[str, str] = {}
        lines = candidate.read_text(encoding="utf-8").splitlines()
        for line in lines[1:]:
            fields = line.split("\t")
            if len(fields) >= 2 and fields[1].strip():
                rows[Path(fields[0]).stem] = " ".join(fields[1].split())
        return rows
    return {}


def discover_clips(clips_dir: Path) -> list[Clip]:
    if not clips_dir.is_dir():
        raise FileNotFoundError(f"каталог клипов не найден: {clips_dir}")
    tsv = _read_tsv(clips_dir)
    clips: list[Clip] = []
    for path in sorted(clips_dir.glob("*.wav")):
        info = sf.info(str(path))
        if info.frames <= 0:
            continue
        text = _ref_text(path, tsv)
        if text:
            clips.append(Clip(path=path, text=text, frames=info.frames))
    if len(clips) < 2:
        raise ValueError(f"{clips_dir}: нужно минимум 2 клипа с текстом, найдено {len(clips)}")
    return clips


def _language_from(clips_dir: Path) -> str:
    names = {clips_dir.name.lower(), clips_dir.parent.name.lower()}
    for code in ("ru", "en", "de", "zh", "cn"):
        if any(name.startswith(code) for name in names):
            return {"cn": "zh"}.get(code, code)
    return "ru"


def _tail_text(text: str, keep_sec: float, total_sec: float) -> str:
    words = text.split()
    keep = int(len(words) * keep_sec / total_sec) if total_sec > 0 else 0
    return " ".join(words[:keep])


def build_meeting(
    clips: list[Clip],
    speakers: int,
    duration_sec: int,
    overlap_pct: int,
    seed: int,
) -> tuple[np.ndarray, list[Turn], list[Path]]:
    rng = random.Random(seed)
    chosen = rng.sample(range(len(clips)), speakers)
    sources = [clips[index].path for index in chosen]
    total = int(round(duration_sec * SAMPLE_RATE))
    track = np.zeros(total, dtype=np.float32)
    samples = {index: _read_samples(clips[index].path) for index in chosen}
    turns: list[Turn] = []
    order: list[int] = []
    cursor = 0

    while True:
        if not order:
            order = list(range(speakers))
            rng.shuffle(order)
            if len(order) > 1 and turns and order[0] == turns[-1].speaker:
                order[0], order[-1] = order[-1], order[0]
        speaker = order.pop(0)
        clip = clips[chosen[speaker]]
        if not turns:
            start = 0
            take = clip.frames
        else:
            previous = turns[-1]
            previous_len = previous.end - previous.start
            overlap = int(round(overlap_pct / 100 * previous_len))
            overlap = min(overlap, previous_len // 2, clip.frames // 2)
            if overlap:
                start = previous.end - overlap
            else:
                start = previous.end + int(round(rng.uniform(*PAUSE_SEC) * SAMPLE_RATE))
            take = clip.frames
            if start + take > total:
                remaining = total - start
                take = remaining if remaining >= MIN_TAIL_SEC * SAMPLE_RATE else 0
        if take <= 0:
            break
        end = start + take
        text = clip.text
        if take < clip.frames:
            text = _tail_text(clip.text, take / SAMPLE_RATE, clip.duration)
            if not text:
                break
        frame = samples[chosen[speaker]]
        track[start:end] += frame[:take]
        turns.append(Turn(speaker=speaker, start=start, end=end, text=text))

    if len({turn.speaker for turn in turns}) < speakers or len(turns) < 2:
        raise ValueError(
            f"не хватило места для {speakers} спикеров за {duration_sec} с: "
            "клипы слишком длинные"
        )
    used = turns[-1].end
    track = track[:used]
    peak = float(np.max(np.abs(track))) if track.size else 0.0
    if peak > 1.0:
        track *= 0.99 / peak
    return track, turns, sources


def write_rttm(path: Path, uri: str, turns: list[Turn]) -> None:
    lines = [
        f"SPEAKER {uri} 1 {turn.start_sec:.3f} {turn.end_sec - turn.start_sec:.3f} "
        f"<NA> <NA> SPEAKER_{turn.speaker:02d} <NA> <NA>"
        for turn in turns
    ]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_ref_text(path: Path, meta: dict, turns: list[Turn]) -> None:
    doc = {
        "meta": meta,
        "utterances": [
            {
                "speaker": f"SPEAKER_{turn.speaker:02d}",
                "start": round(turn.start_sec, 3),
                "end": round(turn.end_sec, 3),
                "text": turn.text,
            }
            for turn in turns
        ],
    }
    path.write_text(json.dumps(doc, ensure_ascii=False, indent=2), encoding="utf-8")


def _manifest_record(
    name: str,
    language: str,
    engine: str,
    speakers: int,
    overlap_pct: int,
    seed: int,
    out: Path,
    duration_sec: float,
) -> dict:
    return {
        "id": name,
        "dataset": "synth",
        "subset": language,
        "audio": str((out / "meeting.wav").resolve()),
        "ref_rttm": str((out / "meeting.rttm").resolve()),
        "ref_text": str((out / "meeting.ref.json").resolve()),
        "language": language,
        "engine": engine,
        "speakers": speakers,
        "duration": round(duration_sec, 3),
        "overlap_pct": overlap_pct,
        "seed": seed,
        "synthetic": True,
    }


def _merge_manifest(path: Path, record: dict) -> None:
    records: list[dict] = []
    if path.is_file():
        payload = json.loads(path.read_text(encoding="utf-8"))
        existing = payload.get("records") if isinstance(payload, dict) else payload
        if isinstance(existing, list):
            records = [item for item in existing if isinstance(item, dict)]
    records = [item for item in records if item.get("id") != record["id"]]
    records.append(record)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps({"schema": 1, "records": records}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )


def generate(
    clips_dir: Path,
    out: Path,
    speakers: int,
    duration_sec: int,
    overlap_pct: int,
    seed: int,
    language: str | None = None,
    engine: str | None = None,
    name: str | None = None,
    manifest_out: Path | None = None,
) -> dict:
    if speakers < 1:
        raise ValueError("--speakers должен быть больше нуля")
    if duration_sec < 1:
        raise ValueError("--duration должен быть больше нуля")
    language = language or _language_from(clips_dir)
    engine = engine or ("whisper" if language == "en" else "gigaam")
    clips = discover_clips(clips_dir)
    track, turns, sources = build_meeting(clips, speakers, duration_sec, overlap_pct, seed)

    out.mkdir(parents=True, exist_ok=True)
    sf.write(out / "meeting.wav", track, SAMPLE_RATE, subtype="PCM_16", format="WAV")
    name = name or f"synth-{language}-{speakers}spk-{duration_sec}s-ov{overlap_pct}"
    write_rttm(out / "meeting.rttm", name, turns)
    duration = track.size / SAMPLE_RATE
    meta = {
        "language": language,
        "engine": engine,
        "speakers": speakers,
        "duration_sec": round(duration, 3),
        "overlap_pct": overlap_pct,
        "seed": seed,
        "synthetic": True,
        "sources": [str(path) for path in sources],
    }
    write_ref_text(out / "meeting.ref.json", meta, turns)
    record = _manifest_record(
        name, language, engine, speakers, overlap_pct, seed, out, duration
    )
    _merge_manifest(manifest_out or out / "manifest.json", record)
    return {"record": record, "turns": turns, "duration": duration}


class _SelfTest(unittest.TestCase):
    def _write_clips(self, root: Path) -> None:
        specs = [(2.0, "альфа бета"), (2.5, "гамма дельта"), (3.0, "эпсилон дзета")]
        for index, (duration, text) in enumerate(specs):
            frames = int(duration * SAMPLE_RATE)
            time = np.arange(frames) / SAMPLE_RATE
            tone = 0.1 * np.sin(2 * math.pi * (220 + 60 * index) * time)
            sf.write(
                root / f"clip{index}.wav", tone.astype(np.float32), SAMPLE_RATE, subtype="PCM_16"
            )
            (root / f"clip{index}.json").write_text(
                json.dumps([{"speaker": "_", "start": 0.0, "end": duration, "text": text}]),
                encoding="utf-8",
            )

    def _generate(self, root: Path, overlap: int, seed: int = DEFAULT_SEED) -> Path:
        out = root / f"ov{overlap}"
        generate(
            clips_dir=root,
            out=out,
            speakers=2,
            duration_sec=20,
            overlap_pct=overlap,
            seed=seed,
            language="ru",
        )
        return out

    def test_meeting_is_consistent(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "clips"
            root.mkdir()
            self._write_clips(root)
            for overlap in (0, 15):
                out = self._generate(root, overlap)
                info = sf.info(str(out / "meeting.wav"))
                self.assertEqual(info.samplerate, SAMPLE_RATE)
                self.assertEqual(info.channels, 1)
                duration = info.frames / info.samplerate
                self.assertGreater(duration, 0)

                rttm = (out / "meeting.rttm").read_text(encoding="utf-8").splitlines()
                self.assertTrue(rttm)
                spans = []
                for line in rttm:
                    fields = line.split()
                    self.assertEqual(len(fields), 10, fields)
                    self.assertEqual(fields[0], "SPEAKER")
                    self.assertEqual(fields[2], "1")
                    start, length = float(fields[3]), float(fields[4])
                    self.assertGreater(length, 0)
                    self.assertGreaterEqual(start, 0)
                    self.assertLessEqual(start + length, duration + 1e-3)
                    spans.append((start, start + length, fields[7]))
                self.assertEqual(len({field[2] for field in spans}), 2)
                self.assertTrue(
                    all(spans[i][2] != spans[i + 1][2] for i in range(len(spans) - 1)),
                    spans,
                )
                self.assertEqual(spans[0][0], 0.0)
                self.assertAlmostEqual(spans[-1][1], duration, places=3)

                ref = json.loads((out / "meeting.ref.json").read_text(encoding="utf-8"))
                utterances = ref["utterances"]
                self.assertEqual(len(utterances), len(spans))
                self.assertEqual(ref["meta"]["speakers"], 2)
                for utterance, (start, end, speaker) in zip(utterances, spans):
                    self.assertAlmostEqual(utterance["start"], start, places=3)
                    self.assertAlmostEqual(utterance["end"], end, places=3)
                    self.assertEqual(utterance["speaker"], speaker)
                    self.assertTrue(utterance["text"].strip())

                overlaps = [
                    max(0.0, spans[i][1] - spans[i + 1][0]) for i in range(len(spans) - 1)
                ]
                if overlap == 0:
                    self.assertEqual(sum(overlaps), 0.0, overlaps)
                    gaps = [spans[i + 1][0] - spans[i][1] for i in range(len(spans) - 1)]
                    self.assertTrue(all(0.19 <= gap <= 0.51 for gap in gaps), gaps)
                else:
                    self.assertGreater(sum(overlaps), 0.5, overlaps)

                other = self._generate(root, overlap, seed=DEFAULT_SEED)
                self.assertEqual(
                    (out / "meeting.rttm").read_text(encoding="utf-8"),
                    (other / "meeting.rttm").read_text(encoding="utf-8"),
                )

                manifest = json.loads((out / "manifest.json").read_text(encoding="utf-8"))
                records = manifest["records"]
                self.assertEqual(len(records), 1)
                record = records[0]
                self.assertEqual(record["speakers"], 2)
                self.assertEqual(record["language"], "ru")
                self.assertEqual(record["engine"], "gigaam")
                for key in ("audio", "ref_rttm", "ref_text"):
                    self.assertTrue(Path(record[key]).is_file(), key)

                samples, _ = sf.read(str(out / "meeting.wav"), dtype="float32")
                self.assertGreater(float(np.max(np.abs(samples))), 0.0)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clips-dir", type=Path, help="каталог клипов FLEURS (en_us/ru_ru)")
    parser.add_argument("--out", type=Path, help="каталог для WAV/RTTM/ref/manifest")
    parser.add_argument("--speakers", type=int, choices=[2, 3, 4], default=2)
    parser.add_argument("--duration", type=int, default=60, help="длительность, с (60/120)")
    parser.add_argument("--overlap", type=int, choices=[0, 5, 15], default=0,
                        help="перекрытие, %% от длительности предыдущей реплики")
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--language", help="ru/en (по умолчанию из имени каталога)")
    parser.add_argument("--engine", choices=["auto", "gigaam", "whisper"])
    parser.add_argument("--name", help="id записи и file id в RTTM")
    parser.add_argument("--manifest-out", type=Path, help="общий manifest для run_batch")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(_SelfTest)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    if args.clips_dir is None or args.out is None:
        parser.error("нужны --clips-dir и --out")

    try:
        result = generate(
            clips_dir=args.clips_dir,
            out=args.out,
            speakers=args.speakers,
            duration_sec=args.duration,
            overlap_pct=args.overlap,
            seed=args.seed,
            language=args.language,
            engine=args.engine,
            name=args.name,
            manifest_out=args.manifest_out,
        )
    except (OSError, ValueError) as exc:
        print(f"ошибка: {exc}", file=sys.stderr)
        return 2
    record = result["record"]
    print(
        f"{args.out}: {record['id']} {result['duration']:.2f} с, "
        f"реплик {len(result['turns'])}, спикеров {args.speakers}, "
        f"overlap {args.overlap}%, язык {record['language']}, движок {record['engine']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
