#!/usr/bin/env python3
"""Отдельная детерминированная двухдикторная выборка FLEURS."""

import argparse
import json
import tarfile
from pathlib import Path

from prepare_fleurs_speakers import digest


def choose(tsv: Path) -> list[list[str]]:
    rows = [line.split("\t") for line in tsv.read_text(encoding="utf-8").splitlines()]
    rows = sorted((r for r in rows if len(r) >= 7 and r[0].isdigit()
                   and r[6] in ("MALE", "FEMALE")), key=lambda r: (int(r[0]), r[1]))
    for first in rows:
        for second in rows:
            if (first[0] != second[0] and first[1] != second[1]
                    and first[6] != second[6] and first[2].strip() != second[2].strip()):
                return [first, second]
    raise ValueError("не найдены клипы разных предложений и полов")


def prepare(root: Path) -> None:
    source = root / "tmp/hf/fleurs/data"
    output = root / "diar-main-clips"
    proof = {"schema": 1, "selection": "different_sentence_opposite_metadata_gender", "languages": {}}
    for lang in ("ru_ru", "en_us"):
        tsv = source / lang / "test.tsv"
        chosen = choose(tsv)
        target = output / lang
        target.mkdir(parents=True, exist_ok=True)
        wanted = {row[1]: (i, row) for i, row in enumerate(chosen)}
        records = []
        with tarfile.open(source / lang / "audio/test.tar.gz", "r|gz") as tar:
            for member in tar:
                name = Path(member.name).name
                if name not in wanted or not member.isfile():
                    continue
                index, row = wanted.pop(name)
                stream = tar.extractfile(member)
                if stream is None:
                    raise ValueError("нет аудио в архиве")
                audio = stream.read()
                filename = f"{index:02d}.wav"
                text = json.dumps({"text": row[2]}, ensure_ascii=False).encode("utf-8")
                (target / filename).write_bytes(audio)
                (target / f"{index:02d}.json").write_bytes(text)
                records.append({"source_id": row[1], "sentence_id": row[0], "gender": row[6],
                                "file": filename, "sha256": digest(audio), "text_sha256": digest(text)})
                if not wanted:
                    break
        if wanted:
            raise ValueError("источник аудио отсутствует в архиве")
        proof["languages"][lang] = {"tsv_sha256": digest(tsv.read_bytes()),
                                    "selected": sorted(records, key=lambda r: r["file"])}
    (output / "proof.json").write_text(json.dumps(proof, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    prepare(parser.parse_args().root)
