#!/usr/bin/env python3
"""Версии, latest.json, SHA256SUMS и minisign-подписи релиза.

Схема latest.json - контракт tauri-plugin-updater: version, notes, pub_date,
platforms с url и signature. Размер пакета в схему не входит.
"""

import argparse
import base64
import datetime
import hashlib
import json
import re
import sys
from pathlib import Path
from urllib.parse import quote

PLATFORMS = {
    ".AppImage": "linux-x86_64",
    ".app.tar.gz": "darwin-aarch64",
    "-setup.exe": "windows-x86_64",
}
REQUIRED_PLATFORMS = set(PLATFORMS.values())
NOTES_LIMIT = 16 * 1024
UNSIGNED_DOC = "docs/UNSIGNED-INSTALL.md"

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def fail(message):
    print(f"::error::{message}")
    sys.exit(1)


def read_text(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError as e:
        fail(f"не читается {path}: {e}")


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def blake2b_file(path):
    digest = hashlib.blake2b(digest_size=64)
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.digest()


def app_versions(root):
    cargo = read_text(root / "src-tauri/crates/mr-app/Cargo.toml")
    version = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.MULTILINE)
    if not version:
        fail("в mr-app/Cargo.toml нет version")
    tauri = json.loads(read_text(root / "src-tauri/crates/mr-app/tauri.conf.json"))
    package = json.loads(read_text(root / "ui/web/frontend/package.json"))
    return {
        "mr-app/Cargo.toml": version.group(1),
        "tauri.conf.json": tauri.get("version"),
        "package.json": package.get("version"),
    }


def check_versions(root, tag):
    if not tag.startswith("v") or len(tag) < 2:
        fail(f"тег {tag!r} должен начинаться с v")
    version = tag[1:]
    bad = {name: value for name, value in app_versions(root).items() if value != version}
    if bad:
        details = ", ".join(f"{name}={value}" for name, value in bad.items())
        fail(f"тег {tag} не совпадает с версиями: {details}")
    return version


def check_notes(root, tag, allow_legacy_notes=False):
    path = root / ".github/release-notes" / f"{tag}.md"
    if not path.is_file():
        fail(f"нет описания релиза: {path.relative_to(root)}")
    text = read_text(path)
    if not text.strip():
        fail(f"пустое описание релиза: {path.relative_to(root)}")
    if not allow_legacy_notes and UNSIGNED_DOC not in text:
        fail(f"в {path.relative_to(root)} нет ссылки на {UNSIGNED_DOC}")
    return text


def truncate_notes(text):
    data = text.encode("utf-8")
    if len(data) <= NOTES_LIMIT:
        return text
    return data[:NOTES_LIMIT].decode("utf-8", errors="ignore")


def decode_signature_box(signature_text):
    text = signature_text.strip()
    try:
        decoded = base64.b64decode(text, validate=True).decode("utf-8")
    except Exception:
        decoded = ""
    if "untrusted comment:" in decoded:
        text = decoded
    primary = trusted = global_sig = None
    for line in (line.strip() for line in text.splitlines()):
        if not line or line.startswith("untrusted comment:"):
            continue
        if line.startswith("trusted comment:"):
            trusted = line[len("trusted comment:"):].lstrip()
        elif primary is None:
            primary = line
        elif global_sig is None:
            global_sig = line
    return primary, trusted, global_sig


def load_public_key(tauri_conf):
    encoded = tauri_conf.get("plugins", {}).get("updater", {}).get("pubkey")
    if not encoded:
        fail("в tauri.conf.json нет plugins.updater.pubkey")
    try:
        text = base64.b64decode(encoded).decode("utf-8")
        line = next(
            line.strip()
            for line in text.splitlines()
            if line.strip() and not line.startswith("untrusted comment:")
        )
        raw = base64.b64decode(line, validate=True)
    except Exception as e:
        fail(f"не разобрать публичный ключ обновления: {e}")
    if len(raw) != 42 or raw[:2] != b"Ed":
        fail("публичный ключ обновления не в формате minisign Ed25519")
    return raw[2:10], raw[10:]


def ed25519_verify(public_key, signature, data):
    try:
        from cryptography.exceptions import InvalidSignature
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    except ImportError:
        fail("для проверки подписей нужен пакет cryptography")
    try:
        Ed25519PublicKey.from_public_bytes(public_key).verify(signature, data)
    except InvalidSignature:
        fail("minisign-подпись не совпадает с файлом")


def verify_minisign_file(path, signature_text, key_id, public_key, version):
    primary_text, trusted, global_text = decode_signature_box(signature_text)
    if not primary_text or not trusted or not global_text:
        fail(f"подпись {path.name} неполная")
    try:
        primary = base64.b64decode(primary_text, validate=True)
        global_sig = base64.b64decode(global_text, validate=True)
    except Exception as e:
        fail(f"подпись {path.name} не декодируется: {e}")
    if len(primary) != 74:
        fail(f"подпись {path.name}: неверная длина")
    algorithm, signature_key_id, signature = primary[:2], primary[2:10], primary[10:]
    if signature_key_id != key_id:
        fail(f"подпись {path.name} сделана другим ключом")
    if algorithm == b"ED":
        data = blake2b_file(path)
    elif algorithm == b"Ed":
        data = path.read_bytes()
    else:
        fail(f"подпись {path.name}: неизвестный алгоритм {algorithm!r}")
    ed25519_verify(public_key, signature, data)
    ed25519_verify(public_key, global_sig, signature + trusted.encode("utf-8"))

    fields = {}
    for part in trusted.split("\t"):
        if ":" in part:
            key, value = part.split(":", 1)
            fields[key] = value
    signed_version = fields.get("version", "").lstrip("vV")
    if not signed_version:
        fail(f"подпись {path.name} не содержит версию")
    if signed_version != version:
        fail(f"подпись {path.name} сделана для версии {signed_version}, а тег {version}")
    signed_file = fields.get("file")
    if signed_file and signed_file != path.name:
        fail(f"подпись {path.name} сделана для файла {signed_file}")


def build_manifest(artifacts, tag, version, notes, repo, key_id, public_key):
    platforms = {}
    for sig_path in sorted(artifacts.glob("*.sig")):
        name = sig_path.name[: -len(".sig")]
        platform = next((value for suffix, value in PLATFORMS.items() if name.endswith(suffix)), None)
        if platform is None:
            fail(f"неизвестный тип пакета: {name}")
        if platform in platforms:
            fail(f"несколько пакетов для платформы {platform}")
        artifact = sig_path.with_name(name)
        if not artifact.is_file():
            fail(f"нет файла {artifact.name} для подписи {sig_path.name}")
        signature_text = read_text(sig_path).strip()
        verify_minisign_file(artifact, signature_text, key_id, public_key, version)
        platforms[platform] = {
            "url": f"https://github.com/{repo}/releases/download/{tag}/{quote(name)}",
            "signature": signature_text,
        }
    missing = REQUIRED_PLATFORMS - set(platforms)
    if missing:
        fail("latest.json неполный, нет платформ: " + ", ".join(sorted(missing)))
    return {
        "version": version,
        "notes": truncate_notes(notes),
        "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "platforms": platforms,
    }


def write_checksums(artifacts):
    lines = []
    for path in sorted(p for p in artifacts.iterdir() if p.is_file() and p.name != "SHA256SUMS"):
        lines.append(f"{sha256_file(path)}  {path.name}")
    (artifacts / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description="Собрать latest.json и SHA256SUMS релиза")
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--repo")
    parser.add_argument("--check-only", action="store_true")
    parser.add_argument(
        "--allow-legacy-notes",
        action="store_true",
        help="не требовать ссылку на UNSIGNED-INSTALL.md (только для dry run)",
    )
    args = parser.parse_args()

    root = args.root.resolve()
    version = check_versions(root, args.tag)
    notes = check_notes(root, args.tag, args.allow_legacy_notes)
    if args.check_only:
        print(f"Версии и заметки релиза согласованы: {args.tag}")
        return
    if not args.artifacts or not args.repo:
        parser.error("нужны --artifacts и --repo")

    artifacts = args.artifacts.resolve()
    if not artifacts.is_dir():
        fail(f"нет папки артефактов: {artifacts}")
    tauri_conf = json.loads(read_text(root / "src-tauri/crates/mr-app/tauri.conf.json"))
    key_id, public_key = load_public_key(tauri_conf)
    manifest = build_manifest(
        artifacts, args.tag, version, notes, args.repo, key_id, public_key
    )
    (artifacts / "latest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    write_checksums(artifacts)
    print(f"latest.json: {args.tag}, платформы: {', '.join(sorted(manifest['platforms']))}")
    print(f"SHA256SUMS: {len(list(artifacts.iterdir()))} файлов")


if __name__ == "__main__":
    main()
