#!/usr/bin/env bash
# Meeting Recorder — запуск нативного Rust-ядра.
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"

# Добавляем пути Cargo и пользовательских бинарников (в GUI-сессиях GNOME/KDE они не подгружаются)
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

# Запуск с иконки идёт без терминала: ошибки не увидеть. Пишем их в лог
# и показываем уведомление, иначе клик выглядит как «ничего не произошло».
LOG="${XDG_STATE_HOME:-$HOME/.local/state}/meeting-recorder/run.log"
if [ ! -t 1 ]; then
    mkdir -p "$(dirname "$LOG")"
    exec >>"$LOG" 2>&1
fi

on_error() {
    local code=$?
    echo "Ошибка запуска, код $code. Лог: $LOG" >&2
    if command -v notify-send >/dev/null 2>&1; then
        notify-send -a "Запись встреч" -i "$SCRIPT_DIR/icon.png" "Приложение не запустилось" \
            "Код $code. Подробности: $LOG" >/dev/null 2>&1 || true
    fi
}
trap on_error ERR

CLEAN_ARGS=()
WANT_BUILD=0
WANT_WEB=0
for a in "$@"; do
    if [ "$a" = "--build" ] || [ "$a" = "--rebuild" ]; then
        WANT_BUILD=1
    elif [ "$a" = "--web" ] || [ "$a" = "--browser" ]; then
        WANT_WEB=1
        CLEAN_ARGS+=("$a")
    else
        CLEAN_ARGS+=("$a")
    fi
done

if [ "$WANT_WEB" = 1 ]; then
    FEATS=""
    SUFFIX="-web"
else
    FEATS="--features tauri-ui"
    SUFFIX=""
fi

BIN="$SCRIPT_DIR/src-tauri/target/release/meeting-recorder$SUFFIX"

# Сборка нужна ТОЛЬКО если бинарник отсутствует, изменились исходники или передан явный флаг --build / --rebuild.
# На обычный клик по иконке или вызов CLI запуск мгновенный без фоновых компиляций Cargo.
FRONT_DIR="$SCRIPT_DIR/ui/web/frontend"
FRONT_STAMP="$SCRIPT_DIR/ui/web/static/.mr-stamp"
NEEDS_FRONT_BUILD=0
if [ "$WANT_BUILD" = 1 ]; then
    NEEDS_FRONT_BUILD=1
elif [ ! -f "$SCRIPT_DIR/ui/web/static/js/app.js" ] || [ ! -f "$FRONT_STAMP" ]; then
    NEEDS_FRONT_BUILD=1
elif [ -n "$(find "$FRONT_DIR/src" "$FRONT_DIR/index.html" "$FRONT_DIR/vite.config.ts" -type f -newer "$FRONT_STAMP" 2>/dev/null | head -n 1)" ]; then
    NEEDS_FRONT_BUILD=1
fi

NEEDS_RUST_BUILD=0
if [ "$WANT_BUILD" = 1 ] || [ ! -f "$BIN" ]; then
    NEEDS_RUST_BUILD=1
elif [ -n "$(find "$SCRIPT_DIR/src-tauri/crates"/*/src "$SCRIPT_DIR/src-tauri/Cargo.toml" -type f -newer "$BIN" 2>/dev/null | head -n 1)" ]; then
    NEEDS_RUST_BUILD=1
elif [ "$NEEDS_FRONT_BUILD" = 1 ]; then
    NEEDS_RUST_BUILD=1
fi

# 3. Если требуется сборка (фронта или Rust), явно и красиво информируем пользователя!
ZENITY_PID=""
stop_progress() {
    if [ -n "$ZENITY_PID" ]; then
        kill "$ZENITY_PID" 2>/dev/null || true
        wait "$ZENITY_PID" 2>/dev/null || true
        ZENITY_PID=""
    fi
}

if [ "$NEEDS_FRONT_BUILD" = 1 ] || [ "$NEEDS_RUST_BUILD" = 1 ]; then
    if command -v notify-send >/dev/null 2>&1; then
        notify-send -a "Запись встреч" -i "$SCRIPT_DIR/icon.png" "Запись встреч" \
            "Идёт сборка обновления... Пожалуйста, подождите" 2>/dev/null || true
    fi
    if [ ! -t 1 ] && command -v zenity >/dev/null 2>&1 && [ -n "$DISPLAY$WAYLAND_DISPLAY" ]; then
        (zenity --progress --pulsate --title="Запись встреч" \
            --text="Сборка обновления Meeting Recorder...\nПожалуйста, подождите" \
            --no-cancel --icon="$SCRIPT_DIR/icon.png" --width=360 2>/dev/null) &
        ZENITY_PID=$!
    fi
fi

# Сборка фронта, если нужно
if [ "$NEEDS_FRONT_BUILD" = 1 ]; then
    if ! command -v node >/dev/null 2>&1; then
        stop_progress
        echo "Ошибка: установите Node.js 20+: https://nodejs.org (нужен только для сборки фронта)"
        exit 1
    fi
    if ! command -v corepack >/dev/null 2>&1 && ! command -v yarn >/dev/null 2>&1; then
        stop_progress
        echo "Ошибка: установите Yarn 4+: corepack enable"
        exit 1
    fi
    if command -v corepack >/dev/null 2>&1; then
        export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
        YARN=(corepack yarn)
    else
        YARN=(yarn)
    fi
    echo "→ Собираю фронт (Svelte + Vite)…"
    (cd "$FRONT_DIR" && "${YARN[@]}" install --immutable && "${YARN[@]}" build)
    FRONT_HASH="$(find "$FRONT_DIR/src" "$FRONT_DIR/index.html" "$FRONT_DIR/package.json" "$FRONT_DIR/yarn.lock" "$FRONT_DIR/vite.config.ts" -type f -exec sha256sum {} + 2>/dev/null | sha256sum | cut -d' ' -f1)"
    echo "$FRONT_HASH" > "$FRONT_STAMP"
fi

# Сборка Rust, если нужно
if [ "$NEEDS_RUST_BUILD" = 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        stop_progress
        if [ -f "$BIN" ]; then
            echo "Внимание: cargo не найден, запускаю существующий бинарник: $BIN" >&2
        else
            echo "Ошибка: установите Rust: https://rustup.rs (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh)"
            exit 1
        fi
    else
        echo "→ Сборка meeting-recorder (Rust)…"
        if [ "$WANT_WEB" = 1 ]; then
            (cd "$SCRIPT_DIR/src-tauri" && CARGO_TARGET_DIR=target/web cargo build --release -p mr-app)
            cp "$SCRIPT_DIR/src-tauri/target/web/release/meeting-recorder" "$BIN"
        else
            # shellcheck disable=SC2086
            (cd "$SCRIPT_DIR/src-tauri" && cargo build --release -p mr-app $FEATS)
        fi
        mkdir -p "$SCRIPT_DIR/dist"
        cp "$BIN" "$SCRIPT_DIR/dist/meeting-recorder-linux$SUFFIX"
    fi
fi

if [ "$NEEDS_FRONT_BUILD" = 1 ] || [ "$NEEDS_RUST_BUILD" = 1 ]; then
    stop_progress
    if command -v notify-send >/dev/null 2>&1; then
        notify-send -a "Запись встреч" -i "$SCRIPT_DIR/icon.png" "Запись встреч" \
            "Приложение готово, запуск!" 2>/dev/null || true
    fi
fi

exec "$BIN" "${CLEAN_ARGS[@]}"
