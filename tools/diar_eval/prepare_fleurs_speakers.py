#!/usr/bin/env python3
"""Подготовка локальных клипов FLEURS с проверяемой различимостью дикторов."""

import argparse
import hashlib
import itertools
import json
import tarfile
from collections import defaultdict
from pathlib import Path


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def validate_selection(records: list[dict]) -> None:
    if len(records) != 4 or len({r["source_id"] for r in records}) != 4:
        raise ValueError("нужно четыре различных source_id")
    if any(r["gender"] not in ("MALE", "FEMALE") or not r["sentence_id"] for r in records):
        raise ValueError("неполные метаданные")
    for sentence in {r["sentence_id"] for r in records}:
        if sum(r["sentence_id"] == sentence for r in records) > 3:
            raise ValueError("слишком много записей одного предложения")
    for a, b in itertools.combinations(records, 2):
        if a["sentence_id"] != b["sentence_id"] and a["gender"] == b["gender"]:
            raise ValueError("нет попарного подтверждения разных дикторов")


def choose(tsv: Path) -> list[list[str]]:
    groups = defaultdict(list)
    for line in tsv.read_text(encoding="utf-8").splitlines():
        row = line.split("\t")
        if len(row) >= 7 and row[0].isdigit() and row[6] in ("MALE", "FEMALE"):
            groups[row[0]].append(row)
    ordered = sorted(groups.items(), key=lambda item: int(item[0]))
    triples = [rows for _, rows in ordered if len(rows) == 3 and len({r[1] for r in rows}) == 3 and len({r[6] for r in rows}) == 1]
    pairs = [rows for _, rows in ordered if len(rows) >= 2 and len({r[6] for r in rows}) == 1]
    if triples:
        chosen = sorted(triples[0], key=lambda r: r[1])
        chosen.append(next(row for _, rows in ordered for row in sorted(rows, key=lambda r: r[1]) if row[6] != chosen[0][6]))
    else:
        chosen = sorted(pairs[0], key=lambda r: r[1])[:2]
        chosen += next(sorted(rows, key=lambda r: r[1])[:2] for rows in pairs if rows[0][6] != chosen[0][6])
    validate_selection([{"source_id": r[1], "sentence_id": r[0], "gender": r[6]} for r in chosen])
    return chosen


def prepare(root: Path) -> None:
    source = root / "tmp/hf/fleurs/data"
    output = root / "diar-regression-clips"
    output.mkdir(parents=True, exist_ok=True)
    proof = {"schema": 1, "source": "https://simran-khanuja.github.io/assets/fleurs_slt.pdf", "languages": {}}
    for lang in ("ru_ru", "en_us"):
        tsv = source / lang / "test.tsv"
        archive = source / lang / "audio/test.tar.gz"
        chosen = choose(tsv)
        records = []
        target = output / lang
        target.mkdir(parents=True, exist_ok=True)
        wanted = {row[1]: (i, row) for i, row in enumerate(chosen)}
        with tarfile.open(archive, "r|gz") as tar:
            for member in tar:
                name = Path(member.name).name
                if name not in wanted or not member.isfile():
                    continue
                index, row = wanted[name]
                stream = tar.extractfile(member)
                if stream is None:
                    raise ValueError("нет аудио в архиве")
                audio = stream.read()
                filename = f"{index:02d}.wav"
                (target / filename).write_bytes(audio)
                text = json.dumps({"text": row[2]}, ensure_ascii=False).encode("utf-8")
                (target / f"{index:02d}.json").write_bytes(text)
                records.append({"source_id": row[1], "sentence_id": row[0], "gender": row[6], "file": filename, "sha256": digest(audio), "text_sha256": digest(text)})
                if len(records) == 4:
                    break
        records.sort(key=lambda r: r["file"])
        validate_selection(records)
        proof["languages"][lang] = {"tsv_sha256": digest(tsv.read_bytes()), "selected": records}
    (output / "proof.json").write_text(json.dumps(proof, indent=2), encoding="utf-8")


def validate(root: Path) -> None:
    output = root / "diar-regression-clips"
    proof = json.loads((output / "proof.json").read_text(encoding="utf-8"))
    if proof.get("schema") != 1:
        raise ValueError("неверная схема proof")
    for lang in ("ru_ru", "en_us"):
        tsv = root / "tmp/hf/fleurs/data" / lang / "test.tsv"
        entry = proof["languages"][lang]
        if digest(tsv.read_bytes()) != entry["tsv_sha256"]:
            raise ValueError("TSV не соответствует proof")
        records = entry["selected"]
        validate_selection(records)
        expected = choose(tsv)
        if [(r["source_id"], r["sentence_id"], r["gender"], r["file"]) for r in records] != [(r[1], r[0], r[6], f"{i:02d}.wav") for i, r in enumerate(expected)]:
            raise ValueError("выборка не совпадает с TSV")
        remaining = {r["source_id"]: r["sha256"] for r in records}
        with tarfile.open(root / "tmp/hf/fleurs/data" / lang / "audio/test.tar.gz", "r|gz") as tar:
            for member in tar:
                name = Path(member.name).name
                if name not in remaining or not member.isfile():
                    continue
                stream = tar.extractfile(member)
                if stream is None or digest(stream.read()) != remaining.pop(name):
                    raise ValueError("источник аудио не совпадает с архивом")
                if not remaining:
                    break
        if remaining:
            raise ValueError("источник аудио отсутствует в архиве")
        for record, row in zip(records, expected):
            audio = (output / lang / record["file"]).read_bytes()
            text = (output / lang / Path(record["file"]).with_suffix(".json")).read_bytes()
            if digest(audio) != record["sha256"] or digest(text) != record["text_sha256"] or json.loads(text)["text"] != row[2]:
                raise ValueError("хеш аудио или текст не совпадает")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    prepare(args.root)
    validate(args.root)
    print("Подготовлено 8 проверенных клипов")
