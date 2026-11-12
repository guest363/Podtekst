#!/usr/bin/env python3
"""Подготовка отдельных нормализованных и тихих двухдикторных встреч."""

import argparse
import json
from pathlib import Path

from prepare_fleurs_main import prepare
from prepare_fleurs_speakers import digest
from synth_meetings import DEFAULT_SEED, generate


def main(root: Path) -> None:
    prepare(root)
    proof_sha256 = digest((root / "diar-main-clips/proof.json").read_bytes())
    out = root / "diar-main-meetings"
    out.mkdir(parents=True, exist_ok=True)
    for variant, gain in (("normal", 0), ("quiet", -18)):
        manifest = out / f"{variant}.json"
        manifest.unlink(missing_ok=True)
        for language, lang in (("en", "en_us"), ("ru", "ru_ru")):
            clips = root / "diar-main-clips" / lang
            if {path.name for path in clips.glob("*.wav")} != {"00.wav", "01.wav"}:
                raise ValueError(f"лишние или отсутствующие клипы: {clips}")
            name = f"main-{language}-2spk-60s-ov0-{variant}"
            generate(clips, out / name, 2, 60, 0, DEFAULT_SEED, language=language,
                     name=name, manifest_out=manifest, second_speaker_db=gain,
                     target_speech_rms=0.1)
        document = json.loads(manifest.read_text(encoding="utf-8"))
        document["source_proof_sha256"] = proof_sha256
        manifest.write_text(json.dumps(document, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"Манифесты: {out / 'normal.json'}, {out / 'quiet.json'}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    main(parser.parse_args().root)
