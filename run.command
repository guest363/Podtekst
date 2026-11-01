#!/usr/bin/env bash
# Подтекст (Podtekst) — запуск нативного ядра на macOS (двойной клик в Finder).
set -e
cd "$(dirname "$0")"
command -v cargo >/dev/null 2>&1 || { echo "[ОШИБКА] Установите Rust: https://rustup.rs"; read -p "Enter..."; exit 1; }
FEATS="--features tauri-ui"
for a in "$@"; do [ "$a" = "--web" ] || [ "$a" = "--browser" ] && FEATS=""; done
# Cargo сам инкрементально пересобирает изменённые исходники (после git pull)
# shellcheck disable=SC2086
(cd src-tauri && cargo build --release -p mr-app $FEATS)
# Ad-hoc подпись даёт TCC стабильную личность: иначе macOS может спрашивать
# доступ к микрофону при каждом запуске или молча отказать.
BIN="src-tauri/target/release/podtekst"
codesign --force --sign - "$BIN" 2>/dev/null || true
# Apple Silicon: x86_64-бинарник под Rosetta замедляет ONNX в разы. Это самая
# частая причина «расшифровка не доходит до конца» на M-чипах.
if [ "$(uname -m)" = "arm64" ]; then
    ARCHS="$(lipo -archs "$BIN" 2>/dev/null || true)"
    case "$ARCHS" in
        *arm64*) ;;
        *) echo "⚠️  Бинарник без arm64 ($ARCHS): он работает через Rosetta и расшифровка будет очень медленной." >&2
           echo "    Установите нативный Rust: rustup toolchain install stable-aarch64-apple-darwin" >&2 ;;
    esac
fi
# Все сборки проекта складываются в одну папку dist/ в корне репозитория.
mkdir -p dist
cp "$BIN" dist/podtekst-macos
exec "$BIN" "$@"
