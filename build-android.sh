#!/usr/bin/env bash
# Meeting Recorder — сборка Android APK одним скриптом.
# Фронт (ui/web/static) → Rust-ядро (aarch64-linux-android) → APK + подсказка adb install.
#
# Требования (один раз): Android SDK + NDK в ~/Android/Sdk, JDK 21, Rust (rustup), Node+Yarn.
# По умолчанию собирает debug-APK (подписан debug-ключом, ставится на устройство).
# RELEASE=1 ./build-android.sh — релизная (без подписи: нужен свой keystore для Play).
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"

# tauri-cli требует булево true/false для --ci, CI=1 из окружения ломает парсер clap
if [ "$CI" = "1" ]; then
    export CI=true
fi

export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-21-openjdk-amd64}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
# Свежий установленный NDK
export NDK_HOME="${NDK_HOME:-"$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -n 1)"}"
if [ -z "$NDK_HOME" ] || [ ! -d "$NDK_HOME" ]; then
    echo "Ошибка: NDK не найден в $ANDROID_HOME/ndk (sdkmanager install 'ndk;27.*')"
    exit 1
fi

# --- Фронт: как в run.sh, со штампом (пропуск, если исходники не менялись) ---
if ! command -v node >/dev/null 2>&1; then
    echo "Ошибка: установите Node.js 20+ (только для сборки фронта)"
    exit 1
fi
if command -v corepack >/dev/null 2>&1; then
    export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
    YARN=(corepack yarn)
else
    YARN=(yarn)
fi
FRONT_DIR="$SCRIPT_DIR/ui/web/frontend"
FRONT_STAMP="$SCRIPT_DIR/src-tauri/target/android-frontend.stamp"
FRONT_HASH="$(find "$FRONT_DIR/src" "$FRONT_DIR/index.html" "$FRONT_DIR/package.json" "$FRONT_DIR/yarn.lock" "$FRONT_DIR/vite.config.ts" -type f -exec sha256sum {} + 2>/dev/null | sha256sum | cut -d' ' -f1)"
if [ ! -f "$SCRIPT_DIR/ui/web/static/js/app.js" ] || [ "$FRONT_HASH" != "$(cat "$FRONT_STAMP" 2>/dev/null)" ]; then
    echo "→ Собираю фронт (Svelte + Vite)…"
    (cd "$FRONT_DIR" && "${YARN[@]}" install --immutable && "${YARN[@]}" build)
    echo "$FRONT_HASH" > "$FRONT_STAMP"
fi

# --- APK ---
MODE=(--debug)
if [ "${RELEASE:-0}" = "1" ]; then
    MODE=()
fi
# Build-скрипт mr-app пишет tauri.settings.gradle только когда env
# TAURI_ANDROID_PROJECT_PATH задан (его ставит CLI). Если до этого гоняли
# `cargo check` под android без env, скрипт закэширован и файл не появится —
# принудительный перезапуск build-скрипта гарантирует его генерацию.
touch "$SCRIPT_DIR/src-tauri/crates/mr-app/build.rs"
cd "$SCRIPT_DIR"
# Нативный tauri-cli: gradle-таска rustBuild ре-инвокует CLI по записанному в
# .tauri/cli.json пути — yarn-dlx для этого нестабилен (временные пути).
# Один раз: cargo install tauri-cli --version ^2.12 --locked
TAURI_BIN="$HOME/.cargo/bin/cargo-tauri"
if [ ! -x "$TAURI_BIN" ]; then
    echo "Ошибка: установите tauri-cli: cargo install tauri-cli --version ^2.12 --locked"
    exit 1
fi
"$TAURI_BIN" android build \
    --config src-tauri/crates/mr-app/tauri.conf.json \
    --features tauri-ui \
    --apk --target aarch64 "${MODE[@]}"

APK="$(find "$SCRIPT_DIR/src-tauri/crates/mr-app/gen/android/app/build/outputs" -name '*.apk' -printf '%T@ %p\n' 2>/dev/null | sort -n | tail -n 1 | cut -d' ' -f2-)"
if [ -n "$APK" ]; then
    # Все сборки проекта складываются в одну папку dist/ в корне репозитория.
    mkdir -p "$SCRIPT_DIR/dist"
    cp "$APK" "$SCRIPT_DIR/dist/meeting-recorder-android.apk"
    APK="$SCRIPT_DIR/dist/meeting-recorder-android.apk"
fi
echo ""
echo "✓ APK готов: $APK"
echo "  Установка на подключённое устройство:"
echo "  $ANDROID_HOME/platform-tools/adb install -r \"$APK\""
echo "  (debug-сборка ~200+ МБ из-за символов; RELEASE=1 даёт компактный unsigned-APK)"
