#!/usr/bin/env bash
# Запись встреч — запуск нативного ядра на macOS (двойной клик в Finder).
set -e
cd "$(dirname "$0")"
command -v cargo >/dev/null 2>&1 || { echo "[ОШИБКА] Установите Rust: https://rustup.rs"; read -p "Enter..."; exit 1; }
FEATS="--features tauri-ui"
for a in "$@"; do [ "$a" = "--web" ] || [ "$a" = "--browser" ] && FEATS=""; done
# Cargo сам инкрементально пересобирает изменённые исходники (после git pull)
# shellcheck disable=SC2086
(cd src-tauri && cargo build --release -p mr-app $FEATS)
# Все сборки проекта складываются в одну папку dist/ в корне репозитория.
mkdir -p dist
cp src-tauri/target/release/meeting-recorder dist/meeting-recorder-macos
exec "src-tauri/target/release/meeting-recorder" "$@"
