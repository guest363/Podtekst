#!/usr/bin/env bash
# Веса не коммитятся: pinned SHA-256, путь отдаётся через GITHUB_ENV.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." >/dev/null 2>&1 && pwd)"
MANIFEST="$ROOT/src-tauri/crates/mr-models/models-manifest.json"
MODEL_ID="sum-qwen3-4b-instruct-2507-q4km"
DEST="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/summary-model"

usage() {
    cat <<'EOF'
Использование: fetch_summary_model.sh [--dest DIR] [--manifest FILE]

Скачивает и проверяет модель итогов из models-manifest.json.
По умолчанию: $RUNNER_TEMP/summary-model (или TMPDIR/summary-model).
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dest) DEST="${2:?}"; shift 2 ;;
        --manifest) MANIFEST="${2:?}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Неизвестный аргумент: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if command -v python3 >/dev/null 2>&1; then
    PY=python3
elif command -v python >/dev/null 2>&1; then
    PY=python
else
    echo "Нужен python3 или python (разбор манифеста и размер файла)" >&2
    exit 1
fi
[ -f "$MANIFEST" ] || {
    echo "Нет манифеста: $MANIFEST" >&2
    exit 1
}

# shellcheck disable=SC2046
eval "$("$PY" -c '
import json, shlex, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
entry = next(m for m in manifest["models"] if m["id"] == sys.argv[2])
item = entry["files"][0]
print("MODEL_FILE=" + shlex.quote(item["filename"]))
print("MODEL_URL=" + shlex.quote(item["url_hf"]))
print("MODEL_SHA256=" + shlex.quote(item["sha256"]))
print("MODEL_BYTES=" + str(int(item["size_bytes"])))
' "$MANIFEST" "$MODEL_ID")"

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

mkdir -p "$DEST"
MODEL="$DEST/$MODEL_FILE"
ACTUAL=""
if [ -f "$MODEL" ]; then
    ACTUAL="$(sha256_file "$MODEL")"
fi
if [ "$ACTUAL" != "$MODEL_SHA256" ]; then
    echo "→ Скачиваю $MODEL_URL"
    curl -fL --retry 3 --retry-delay 5 -o "$MODEL.part" "$MODEL_URL"
    mv "$MODEL.part" "$MODEL"
fi

ACTUAL="$(sha256_file "$MODEL")"
if [ "$ACTUAL" != "$MODEL_SHA256" ]; then
    echo "SHA-256 не совпал: $ACTUAL != $MODEL_SHA256" >&2
    exit 1
fi
SIZE="$("$PY" -c 'import os,sys; print(os.path.getsize(sys.argv[1]))' "$MODEL")"
if [ "$SIZE" != "$MODEL_BYTES" ]; then
    echo "Размер не совпал: $SIZE != $MODEL_BYTES" >&2
    exit 1
fi

if [ -n "${GITHUB_ENV:-}" ]; then
    echo "MR_SUMMARY_MODEL=$MODEL" >> "$GITHUB_ENV"
fi
if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "model=$MODEL" >> "$GITHUB_OUTPUT"
    echo "model_sha=$MODEL_SHA256" >> "$GITHUB_OUTPUT"
fi
echo "→ Модель: $MODEL ($SIZE байт, SHA-256 $MODEL_SHA256)"
