#!/usr/bin/env python3
"""Мониторинг апстримов проверенных компонентов.

Сеть нужна только здесь и в CI: клиент версии не проверяет.
"""

import argparse
import json
import re
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "src-tauri/crates/mr-models/models-manifest.json"
PINS = ROOT / "src-tauri/crates/mr-app/components-pins.json"
TIMEOUT = 20
USER_AGENT = "podtekst-components-monitor"
SHA256_REVISION = re.compile(r"[0-9a-f]{12,40}")

PIN_PAGES = {
    "gyan": (
        "https://api.github.com/repos/GyanD/codexffmpeg/releases/latest",
        r'"tag_name"\s*:\s*"v?(\d+\.\d+(?:\.\d+)?)"',
    ),
    "martin-riedl": ("https://ffmpeg.martin-riedl.de/", r"Release:\s*</b>\s*(\d+\.\d+(?:\.\d+)?)"),
    "johnvansickle": ("https://johnvansickle.com/ffmpeg/", r"release:\s*(\d+\.\d+(?:\.\d+)?)"),
}


def load(path, required):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        if required:
            sys.exit(f"components-monitor: не читается {path}: {error}")
        return None
    except json.JSONDecodeError as error:
        sys.exit(f"components-monitor: невалидный JSON в {path}: {error}")


def fetch(url):
    request = urllib.request.Request(
        url, headers={"User-Agent": USER_AGENT, "Accept": "application/json, text/html"}
    )
    with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
        return response.read().decode("utf-8", errors="replace")


def fetch_json(url):
    return json.loads(fetch(url))


def version_key(value):
    return tuple(int(part) for part in re.findall(r"\d+", value))


def probe_model(kind, repo, cache):
    if (kind, repo) in cache:
        return cache[(kind, repo)]
    try:
        if kind == "hf":
            latest = fetch_json(f"https://huggingface.co/api/models/{repo}").get("sha") or ""
        else:
            latest = fetch_json(f"https://api.github.com/repos/{repo}/releases/latest").get("tag_name") or ""
        result = ("ok", latest) if latest else ("empty", "")
    except (OSError, ValueError):
        result = ("error", "")
    cache[(kind, repo)] = result
    return result


def probe_pin(source, cache):
    if source not in PIN_PAGES:
        return "unknown", []
    if source in cache:
        return cache[source]
    page, pattern = PIN_PAGES[source]
    try:
        result = ("ok", re.findall(pattern, fetch(page)))
    except (OSError, ValueError):
        result = ("error", [])
    cache[source] = result
    return result


def short_revision(revision):
    if SHA256_REVISION.match(revision):
        return revision[:12]
    return revision


def model_rows(manifest, offline, rows, cache):
    models = manifest.get("models") if isinstance(manifest, dict) else None
    if not isinstance(models, list):
        return
    for model in models:
        if not isinstance(model, dict):
            continue
        model_id = model.get("id") or "?"
        upstream = model.get("upstream") or {}
        kind = upstream.get("kind")
        repo = upstream.get("repo")
        revision = upstream.get("revision")
        version = upstream.get("version") or model.get("version") or "—"
        if not (kind and repo and revision):
            rows.append(
                {
                    "component": model_id,
                    "current": str(model.get("version") or "—"),
                    "upstream": "—",
                    "status": "нет upstream",
                    "link": "—",
                }
            )
            continue
        link = (
            f"https://huggingface.co/{repo}"
            if kind == "hf"
            else f"https://github.com/{repo}"
        )
        current = f"{version} · {short_revision(str(revision))}" if len(str(revision)) > 12 else version
        if offline:
            status, upstream_value = "офлайн", "—"
        else:
            state, latest = probe_model(kind, repo, cache)
            if state == "error":
                status, upstream_value = "ошибка сети", "—"
            elif state == "empty":
                status, upstream_value = "нет данных", "—"
            else:
                upstream_value = short_revision(latest)
                status = "актуально" if latest == revision else "есть новее"
        rows.append(
            {
                "component": model_id,
                "current": current,
                "upstream": upstream_value,
                "status": status,
                "link": link,
            }
        )


def pin_rows(pins, offline, rows, cache):
    packages = pins.get("packages") if isinstance(pins, dict) else None
    if not isinstance(packages, list):
        return
    seen = set()
    for package in packages:
        if not isinstance(package, dict):
            continue
        os_name, arch = package.get("os"), package.get("arch")
        source, version, url = package.get("source"), package.get("version"), package.get("url")
        if not (os_name and arch and source and version):
            continue
        if (os_name, arch) in seen:
            continue
        seen.add((os_name, arch))
        page = PIN_PAGES.get(source, ("—", ""))[0]
        if offline:
            status, upstream_value = "офлайн", "—"
        else:
            state, found = probe_pin(source, cache)
            if state == "unknown":
                status, upstream_value = "неизвестный источник", "—"
            elif state == "error":
                status, upstream_value = "ошибка сети", "—"
            elif not found:
                status, upstream_value = "нет данных", "—"
            else:
                latest = max(found, key=version_key)
                upstream_value = latest
                if version_key(latest) > version_key(version):
                    status = "есть новее"
                elif version in found:
                    status = "актуально"
                else:
                    status = "нет на апстриме"
        rows.append(
            {
                "component": f"ffmpeg {os_name}/{arch}",
                "current": version,
                "upstream": upstream_value,
                "status": status,
                "link": page if page != "—" else (url or "—"),
            }
        )


def summarize(rows):
    updated = ("есть новее", "нет на апстриме")
    unavailable = ("офлайн", "нет upstream", "нет данных", "неизвестный источник")
    return {
        "updates": sum(row["status"] in updated for row in rows),
        "network_errors": sum(row["status"] == "ошибка сети" for row in rows),
        "unchecked": sum(row["status"] in unavailable for row in rows),
    }


def print_markdown(rows, summary, offline):
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")
    print("## Апстримы компонентов")
    print()
    print(f"Обновлено: {stamp}." + (" Режим: офлайн, сеть не использовалась." if offline else ""))
    print()
    print("| Компонент | Сейчас | Апстрим | Статус | Ссылка |")
    print("|---|---|---|---|---|")
    for row in rows:
        print(
            f"| {row['component']} | {row['current']} | {row['upstream']} "
            f"| {row['status']} | {row['link']} |"
        )
    print()
    print(
        f"Итог: обновлений — {summary['updates']}, ошибок сети — {summary['network_errors']}, "
        f"не проверено — {summary['unchecked']}."
    )


def main():
    parser = argparse.ArgumentParser(description="Сверить пины компонентов с апстримами")
    parser.add_argument("--offline", action="store_true", help="не обращаться к сети")
    parser.add_argument("--json", action="store_true", help="машинный вывод вместо Markdown")
    args = parser.parse_args()

    manifest = load(MANIFEST, required=True)
    pins = load(PINS, required=False)

    rows = []
    model_cache = {}
    pin_cache = {}
    model_rows(manifest, args.offline, rows, model_cache)
    if pins is None:
        if not args.json:
            print(f"> Пины ffmpeg не найдены: {PINS}", file=sys.stderr)
    else:
        pin_rows(pins, args.offline, rows, pin_cache)

    summary = summarize(rows)
    if args.json:
        print(
            json.dumps(
                {
                    "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                    "offline": args.offline,
                    "pins_present": pins is not None,
                    "summary": summary,
                    "rows": rows,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    else:
        print_markdown(rows, summary, args.offline)
    return 0


if __name__ == "__main__":
    sys.exit(main())
