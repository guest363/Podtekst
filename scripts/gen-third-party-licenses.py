#!/usr/bin/env python3
"""Генерирует THIRD_PARTY_LICENSES.md из `cargo metadata --locked --all-features`.

Инвентарь обязан покрывать весь граф зависимостей, а не только уже
перечисленные пакеты: файл кладётся в каждый бандл как официальный
перечень лицензий.

Использование:
    python3 scripts/gen-third-party-licenses.py [--check]

--check завершается с кодом 1, если файл в репозитории расходится с графом
(гейт CI: правка Cargo.lock без обновления инвентаря не пройдёт).
"""

from __future__ import annotations

import argparse
import collections
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "THIRD_PARTY_LICENSES.md"
MANIFEST = ROOT / "src-tauri" / "Cargo.toml"

HEADER = """# Third-Party Licenses

Инвентарь Rust-зависимостей из `Cargo.lock` (весь граф, включая desktop-стек
Tauri/GTK). Стандартные тексты лицензий - в `LICENSES/`. Точные
copyright-уведомления пакетов - в upstream-репозиториях и на crates.io.
Файл сгенерирован `scripts/gen-third-party-licenses.py` из
`cargo metadata --format-version 1 --locked --all-features`; не правьте его вручную.
"""

FOOTER = """
## Не-Rust компоненты

* **ffmpeg / ffprobe** - см. `docs/ffmpeg-build.md` и `THIRD_PARTY_NOTICES.md`
* **llama.cpp / llama-server** - MIT, https://github.com/ggml-org/llama.cpp
* **ONNX Runtime** - MIT, https://github.com/microsoft/onnxruntime/blob/main/LICENSE
* **Svelte runtime** - MIT, https://github.com/sveltejs/svelte
* **AndroidX / Material / Kotlin** - Apache-2.0, https://developer.android.com/jetpack/androidx
* **Gradle wrapper** - Apache-2.0, https://gradle.org/license/
"""


def load_packages() -> list[dict]:
    out = subprocess.run(
        # --all-features обязателен: без него опциональные `tauri`/`gtk`
        # не попадают в граф, и инвентарь теряет весь desktop-стек.
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
            "--manifest-path",
            str(MANIFEST),
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(out)
    members = set(meta.get("workspace_members") or [])
    packages = []
    for pkg in meta["packages"]:
        if pkg["id"] in members:
            continue
        if pkg.get("source") is None:
            continue
        packages.append(pkg)
    packages.sort(key=lambda p: (p["name"], p["version"]))
    return packages


def render(packages: list[dict]) -> str:
    counts: collections.Counter[str] = collections.Counter(
        (p.get("license") or "не указана") for p in packages
    )
    lines = [HEADER, "## Сводка", ""]
    lines.append(f"* Всего пакетов: {len(packages)}")
    for license_name, count in counts.most_common():
        lines.append(f"* `{license_name}` - {count}")
    lines += ["", "## Пакеты", "", "| Пакет | Версия | Лицензия | Источник |", "|---|---|---|---|"]
    for pkg in packages:
        license_name = (pkg.get("license") or "").strip() or "см. репозиторий"
        source = pkg.get("repository") or pkg.get("homepage") or pkg.get("source") or ""
        lines.append(f"| `{pkg['name']}` | {pkg['version']} | {license_name} | {source} |")
    return "\n".join(lines) + "\n" + FOOTER


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="только сверить файл с графом")
    args = parser.parse_args()
    rendered = render(load_packages())
    if args.check:
        current = TARGET.read_text() if TARGET.is_file() else ""
        if current != rendered:
            print(
                "THIRD_PARTY_LICENSES.md устарел: запустите python3 scripts/gen-third-party-licenses.py",
                file=sys.stderr,
            )
            return 1
        print("THIRD_PARTY_LICENSES.md соответствует Cargo.lock")
        return 0
    TARGET.write_text(rendered)
    print(f"THIRD_PARTY_LICENSES.md обновлён")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
