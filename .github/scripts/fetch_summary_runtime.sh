#!/usr/bin/env bash
# Runtime не коммитится: pinned SHA-256, в ресурсы Tauri кладутся только
# нужные файлы.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." >/dev/null 2>&1 && pwd)"
MANIFEST="$ROOT/src-tauri/crates/mr-app/summary-runtime-manifest.json"

TARGET=""
ARCHIVE=""
DEST=""
ALLOW_CROSS=0
KEEP_TEMP=0
LIMIT_BYTES=$((60 * 1024 * 1024))

usage() {
    cat <<'EOF'
Использование: fetch_summary_runtime.sh [--target KEY] [--archive FILE]
                                        [--dest DIR] [--allow-cross] [--keep-temp]

KEY: linux-x86_64 | macos-arm64 | windows-x86_64 (по умолчанию — текущая ОС).
Без --archive архив скачивается по URL из summary-runtime-manifest.json.
Каталог по умолчанию: src-tauri/binaries/runtime/<triple>.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target) TARGET="${2:?}"; shift 2 ;;
        --archive) ARCHIVE="${2:?}"; shift 2 ;;
        --dest) DEST="${2:?}"; shift 2 ;;
        --allow-cross) ALLOW_CROSS=1; shift ;;
        --keep-temp) KEEP_TEMP=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Неизвестный аргумент: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if command -v python3 >/dev/null 2>&1; then
    PY=python3
elif command -v python >/dev/null 2>&1; then
    PY=python
else
    echo "Нужен python3 или python (разбор манифеста и zip)" >&2
    exit 1
fi
[ -f "$MANIFEST" ] || {
    echo "Нет манифеста: $MANIFEST" >&2
    exit 1
}

host_target() {
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64|Linux-amd64) echo linux-x86_64 ;;
        Darwin-arm64|Darwin-aarch64) echo macos-arm64 ;;
        MINGW*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64) echo windows-x86_64 ;;
        *) echo "" ;;
    esac
}

HOST="$(host_target)"
[ -n "$TARGET" ] || TARGET="$HOST"
[ -n "$TARGET" ] || {
    echo "Не удалось определить целевой ключ; передайте --target" >&2
    exit 1
}
if [ "$TARGET" != "$HOST" ] && [ "$ALLOW_CROSS" != 1 ]; then
    echo "Цель $TARGET не совпадает с хостом ${HOST:-unknown}." >&2
    echo "Для осмотра чужого архива добавьте --allow-cross (каталог не будет проверен запуском)." >&2
    exit 1
fi

# shellcheck disable=SC2046
eval "$("$PY" - "$MANIFEST" "$TARGET" <<'PY'
import json
import shlex
import sys

manifest = json.load(open(sys.argv[1], encoding="utf-8"))
target = manifest["targets"][sys.argv[2]]
print(f"TRIPLE={shlex.quote(target['triple'])}")
print(f"URL={shlex.quote(target['url'])}")
print(f"SHA256={shlex.quote(target['sha256'])}")
print(f"ASSET={shlex.quote(target['asset'])}")
print(f"EXPECTED_BYTES={int(target['installed_bytes'])}")
print(f"LAUNCHER={shlex.quote(manifest.get('launcher', 'llama-server'))}")
print("PRUNE=(" + " ".join(shlex.quote(p) for p in target["prune"]) + ")")
print("LICENSE_FILES=(" + " ".join(shlex.quote(p) for p in target.get("license_files", [])) + ")")
print("REQUIRED_FLAGS=(" + " ".join(shlex.quote(f) for f in manifest.get("required_flags", [])) + ")")
PY
)"

[ -n "$DEST" ] || DEST="$ROOT/src-tauri/binaries/runtime/$TRIPLE"

TMP="$(mktemp -d "${TMPDIR:-/tmp}/podtekst-summary-runtime.XXXXXX")"
cleanup() {
    if [ "$KEEP_TEMP" = 1 ]; then
        echo "Временный каталог: $TMP"
    else
        rm -rf "$TMP"
    fi
}
trap cleanup EXIT

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

if [ -z "$ARCHIVE" ]; then
    ARCHIVE="$TMP/$ASSET"
    echo "→ Скачиваю $URL"
    curl -fL --retry 3 --retry-delay 2 -o "$ARCHIVE" "$URL"
elif [ ! -f "$ARCHIVE" ]; then
    echo "Нет архива: $ARCHIVE" >&2
    exit 1
fi

echo "→ Проверяю SHA-256"
ACTUAL="$(sha256_file "$ARCHIVE")"
if [ "$ACTUAL" != "$SHA256" ]; then
    echo "SHA-256 не совпал: $ACTUAL != $SHA256" >&2
    exit 1
fi

mkdir -p "$TMP/extract"
echo "→ Распаковываю $(basename "$ARCHIVE")"
case "$ARCHIVE" in
    *.zip)
        "$PY" - "$ARCHIVE" "$TMP/extract" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    archive.extractall(sys.argv[2])
PY
        ;;
    *.tar.gz|*.tgz)
        tar -xzf "$ARCHIVE" -C "$TMP/extract"
        ;;
    *)
        echo "Неизвестный формат архива: $ARCHIVE" >&2
        exit 1
        ;;
esac

SRC_ROOT="$("$PY" - "$TMP/extract" "$LAUNCHER" <<'PY'
import os
import sys

root, launcher = sys.argv[1], sys.argv[2]
names = {launcher, launcher + ".exe"}
for dirpath, _dirnames, filenames in os.walk(root):
    if names.intersection(filenames):
        print(dirpath)
        break
PY
)"
[ -n "$SRC_ROOT" ] || {
    echo "В архиве нет $LAUNCHER" >&2
    exit 1
}

rm -rf "$DEST"
mkdir -p "$DEST"

echo "→ Оставляю только runtime-файлы"
shopt -s nullglob
SEEN_FILE="$TMP/seen"
: > "$SEEN_FILE"
for pattern in "${PRUNE[@]}" "${LICENSE_FILES[@]}"; do
    for file in "$SRC_ROOT"/$pattern; do
        [ -e "$file" ] || [ -L "$file" ] || continue
        real="$("$PY" -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$file")"
        if grep -Fxq "$real" "$SEEN_FILE"; then
            continue
        fi
        printf '%s\n' "$real" >> "$SEEN_FILE"
        name="$(basename "$file")"
        if [ "$TARGET" = "$HOST" ]; then
            # fs::copy в Tauri разыменовывает symlink; кладём содержимое
            # сразу под именем, которое ищет динамический загрузчик.
            case "$name" in
                *.so|*.so.*)
                    soname="$(readelf -d "$real" 2>/dev/null \
                        | sed -n 's/.*SONAME.*\[\(.*\)\].*/\1/p' | head -n 1 || true)"
                    [ -n "$soname" ] && name="$soname"
                    ;;
                *.dylib)
                    if command -v otool >/dev/null 2>&1; then
                        install="$(otool -D "$real" 2>/dev/null \
                            | sed -n '2p' | xargs -n1 basename 2>/dev/null || true)"
                        [ -n "$install" ] && name="$install"
                    fi
                    ;;
            esac
            cp -f "$real" "$DEST/$name"
        else
            # Кросс-осмотр: без проверки загрузчиком, для отчёта о размере.
            cp -a "$file" "$DEST/$name"
        fi
    done
done

cp -f "$ROOT/LICENSES/llama.cpp-LICENSE.txt" "$DEST/llama.cpp-LICENSE.txt"

INSTALLED="$("$PY" - "$DEST" <<'PY'
import os
import sys

total = 0
for dirpath, _dirnames, filenames in os.walk(sys.argv[1]):
    for name in filenames:
        path = os.path.join(dirpath, name)
        if not os.path.islink(path):
            total += os.path.getsize(path)
print(total)
PY
)"
FILES="$("$PY" -c 'import os,sys; print(sum(len(f) for _,_,f in os.walk(sys.argv[1])))' "$DEST")"
echo "→ Установлено: $FILES файлов, $INSTALLED байт (лимит $LIMIT_BYTES)"
if [ "$INSTALLED" -gt "$LIMIT_BYTES" ]; then
    echo "Runtime больше бюджета 60 МБ" >&2
    exit 1
fi

if [ "$TARGET" = "$HOST" ]; then
    echo "→ Проверяю запуск $LAUNCHER"
    case "$(uname -s)" in
        Darwin) export DYLD_LIBRARY_PATH="$DEST:${DYLD_LIBRARY_PATH:-}" ;;
        Linux) export LD_LIBRARY_PATH="$DEST:${LD_LIBRARY_PATH:-}" ;;
    esac
    VERSION_OUT="$("$DEST/$LAUNCHER" --version 2>&1 || true)"
    case "$VERSION_OUT" in
        *"build 12182"*) ;;
        *)
            echo "llama-server --version не подтвердил build 12182: $VERSION_OUT" >&2
            exit 1
            ;;
    esac
    case "$VERSION_OUT" in
        *"commit fd27b2034"*) ;;
        *)
            echo "llama-server --version не подтвердил commit fd27b2034: $VERSION_OUT" >&2
            exit 1
            ;;
    esac
    HELP_OUT="$("$DEST/$LAUNCHER" --help 2>&1 || true)"
    for flag in "${REQUIRED_FLAGS[@]}"; do
        case "$HELP_OUT" in
            *"$flag"*) ;;
            *)
                echo "В --help нет обязательного флага $flag" >&2
                exit 1
                ;;
        esac
    done
    if [ "$(uname -s)" = "Linux" ]; then
        for file in "$DEST"/*.so "$DEST/$LAUNCHER"; do
            [ -f "$file" ] || continue
            LDD_OUT="$(ldd "$file" 2>&1 || true)"
            case "$LDD_OUT" in
                *"not found"*)
                    echo "Не найдены системные библиотеки для $file:" >&2
                    printf '%s\n' "$LDD_OUT" >&2
                    exit 1
                    ;;
            esac
        done
    elif [ "$(uname -s)" = "Darwin" ] && command -v otool >/dev/null 2>&1; then
        for file in "$DEST"/*.dylib "$DEST/$LAUNCHER"; do
            [ -f "$file" ] || continue
            OTOOL_OUT="$(otool -L "$file" 2>&1 || true)"
            case "$OTOOL_OUT" in
                *"not found"*)
                    echo "otool: не найдены зависимости $file" >&2
                    exit 1
                    ;;
            esac
        done
    fi
else
    echo "→ Кросс-цель: запуск и ldd/otool не проверяются"
fi

echo "Готово: $DEST"
echo "  triple: $TRIPLE"
echo "  archive sha256: $SHA256"
echo "  installed: $INSTALLED байт"
