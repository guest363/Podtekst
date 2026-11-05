#!/usr/bin/env python3
"""Проверка опубликованного релиза: latest.json, URL, SHA-256 и подписи."""

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime
from pathlib import Path

from updater_manifest import (
    REQUIRED_PLATFORMS,
    load_public_key,
    sha256_file,
    verify_minisign_file,
)

UNSIGNED_DOC = "docs/UNSIGNED-INSTALL.md"

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def fail(message):
    print(f"::error::{message}")
    sys.exit(1)


def api_get(url, token):
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "podtekst-verify",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return response.read()


def download(url, destination, retries=3):
    headers = {"User-Agent": "podtekst-verify"}
    last = ""
    for attempt in range(retries):
        try:
            request = urllib.request.Request(url, headers=headers)
            with urllib.request.urlopen(request, timeout=120) as response:
                if response.status != 200:
                    fail(f"{url}: HTTP {response.status}")
                destination.parent.mkdir(parents=True, exist_ok=True)
                with destination.open("wb") as out:
                    shutil.copyfileobj(response, out)
                return destination
        except urllib.error.HTTPError as e:
            last = f"HTTP {e.code}"
            if e.code in (404, 429, 500, 502, 503) and attempt + 1 < retries:
                time.sleep(5)
                continue
            fail(f"{url}: {last}")
        except urllib.error.URLError as e:
            last = str(e)
            if attempt + 1 < retries:
                time.sleep(5)
                continue
            fail(f"{url}: {last}")
    fail(f"{url}: {last}")


def parse_sums(path):
    sums = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split(None, 1)
        if len(parts) != 2 or not re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            fail(f"некорректная строка SHA256SUMS: {line!r}")
        name = parts[1].lstrip("*")
        if name in sums or "/" in name or "\\" in name:
            fail(f"некорректное имя в SHA256SUMS: {name!r}")
        sums[name] = parts[0]
    return sums


def latest_tag(repo, token):
    data = json.loads(api_get(f"https://api.github.com/repos/{repo}/releases/latest", token))
    tag = data.get("tag_name", "")
    if not tag.startswith("v"):
        fail(f"последний релиз вернул странный тег: {tag!r}")
    return tag


def check_alias(repo, version, require_latest):
    url = f"https://github.com/{repo}/releases/latest/download/latest.json"
    for attempt in range(6):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "podtekst-verify"})
            with urllib.request.urlopen(request, timeout=30) as response:
                alias = json.loads(response.read())
            if alias.get("version") == version:
                return
        except (urllib.error.URLError, json.JSONDecodeError):
            pass
        if attempt < 5:
            time.sleep(10)
    if require_latest:
        fail(f"releases/latest/download/latest.json не указывает на {version}")
    print(f"Предупреждение: последний релиз ещё не {version}")


def main():
    parser = argparse.ArgumentParser(description="Проверить опубликованный релиз")
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "guest363/podtekst"))
    parser.add_argument("--tag", help="тег релиза; пусто - последний релиз")
    parser.add_argument("--require-latest", action="store_true")
    args = parser.parse_args()

    root = args.root.resolve()
    token = os.environ.get("GITHUB_TOKEN", "")
    try:
        latest = latest_tag(args.repo, token)
    except (urllib.error.URLError, json.JSONDecodeError) as e:
        if not args.tag:
            fail(f"не удалось определить последний релиз: {e}")
        latest = None
        print(f"Предупреждение: GitHub API недоступен ({e}), проверяю тег {args.tag}")

    tag = args.tag or latest
    if not tag or not tag.startswith("v"):
        fail(f"некорректный тег: {tag!r}")
    version = tag[1:]
    if latest and latest != tag and args.require_latest:
        print(f"Предупреждение: API последнего релиза пока возвращает {latest}")

    tauri_conf = json.loads(
        (root / "src-tauri/crates/mr-app/tauri.conf.json").read_text(encoding="utf-8")
    )
    key_id, public_key = load_public_key(tauri_conf)
    base = f"https://github.com/{args.repo}/releases/download/{urllib.parse.quote(tag, safe='')}"

    work = Path(tempfile.mkdtemp(prefix="podtekst-verify-"))
    try:
        latest_path = download(f"{base}/latest.json", work / "latest.json")
        manifest = json.loads(latest_path.read_text(encoding="utf-8"))
        if manifest.get("version") != version:
            fail(f"latest.json: version={manifest.get('version')!r}, ожидалась {version}")
        notes = manifest.get("notes")
        if not isinstance(notes, str) or UNSIGNED_DOC not in notes:
            fail(f"latest.json: notes не содержит {UNSIGNED_DOC}")
        pub_date = manifest.get("pub_date")
        try:
            datetime.fromisoformat(str(pub_date).replace("Z", "+00:00"))
        except ValueError:
            fail(f"latest.json: некорректный pub_date {pub_date!r}")
        platforms = manifest.get("platforms")
        if not isinstance(platforms, dict) or set(platforms) != REQUIRED_PLATFORMS:
            fail(f"latest.json: платформы {sorted(platforms or {})}, нужны {sorted(REQUIRED_PLATFORMS)}")

        sums_path = download(f"{base}/SHA256SUMS", work / "SHA256SUMS")
        sums = parse_sums(sums_path)
        if sums.get("latest.json") != sha256_file(latest_path):
            fail("SHA256SUMS: хэш latest.json не совпал")
        verified = {"latest.json"}

        for platform, target in sorted(platforms.items()):
            url = target.get("url") if isinstance(target, dict) else None
            signature = target.get("signature") if isinstance(target, dict) else None
            if not url or not signature:
                fail(f"latest.json: у {platform} нет url или signature")
            if not url.startswith(base + "/"):
                fail(f"latest.json: URL {platform} ведёт не в этот релиз")
            name = urllib.parse.unquote(url.rsplit("/", 1)[-1])
            if name not in sums:
                fail(f"SHA256SUMS: нет {name}")
            artifact = download(url, work / name)
            if sha256_file(artifact) != sums[name]:
                fail(f"SHA-256 не совпал: {name}")
            verify_minisign_file(artifact, signature, key_id, public_key, version)
            sidecar = work / f"{name}.sig"
            download(url + ".sig", sidecar)
            verify_minisign_file(
                artifact, sidecar.read_text(encoding="utf-8"), key_id, public_key, version
            )
            verified.update({name, f"{name}.sig"})

        for name, expected in sorted(sums.items()):
            if name in verified:
                continue
            path = download(f"{base}/{urllib.parse.quote(name, safe='')}", work / name)
            if sha256_file(path) != expected:
                fail(f"SHA-256 не совпал: {name}")

        if latest is None or latest == tag or args.require_latest:
            check_alias(args.repo, version, args.require_latest)
        print(
            f"Проверка OK: {tag}, платформ {len(platforms)}, "
            f"файлов в SHA256SUMS {len(sums)}, подписи верные"
        )
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
