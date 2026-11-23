import argparse
import re
from html import escape
from pathlib import Path
from shutil import copytree


ROOT = Path(__file__).resolve().parents[1]
MARKER = "        <!-- RELEASE_NOTES -->"
RELEASES_URL = "https://github.com/guest363/Podtekst/releases"


def render_release_notes():
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    versions = re.findall(r"^## \[(\d+\.\d+\.\d+)\]", changelog, re.MULTILINE)[:2]
    if not versions:
        raise ValueError("В CHANGELOG.md нет опубликованных версий")

    rows = []
    for version in versions:
        path = ROOT / ".github" / "release-notes" / f"v{version}.md"
        intro = path.read_text(encoding="utf-8").strip().split("\n\n", 1)[0]
        intro = re.sub(rf"^Подтекст v?{re.escape(version)}:\s*", "", intro)
        if not intro or intro.startswith("#"):
            raise ValueError(f"Добавьте короткий вступительный абзац в {path.name}")
        intro = intro[0].upper() + intro[1:]
        rows.append(
            f'          <div class="roadmap-update">\n'
            f'            <a class="text-link" href="{RELEASES_URL}/tag/v{version}" rel="noopener"'
            f' aria-label="Полное описание версии {version}">{version} <span aria-hidden="true">↗</span></a>\n'
            f'            <p>{escape(intro)}</p>\n'
            f'          </div>'
        )

    return "\n".join(
        [
            '        <details class="roadmap-updates">',
            f'          <summary>Что нового <span class="roadmap-update-version">{versions[0]}</span></summary>',
            *rows,
            "        </details>",
        ]
    )


def main():
    parser = argparse.ArgumentParser(description="Собрать статический лендинг Подтекста.")
    parser.add_argument("output", type=Path, help="Новый каталог для собранного сайта")
    args = parser.parse_args()

    source = ROOT / "site"
    index = (source / "index.html").read_text(encoding="utf-8")
    if index.count(MARKER) != 1:
        raise ValueError("В site/index.html нужна одна метка RELEASE_NOTES")
    index = index.replace(MARKER, render_release_notes())
    copytree(source, args.output)
    (args.output / "index.html").write_text(index, encoding="utf-8")


if __name__ == "__main__":
    main()
