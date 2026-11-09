#!/usr/bin/env bash
# Release-гейт: dev-sink не должен попадать в release-бинарник.
# Маркер PODTEKST_DEV_LOGS_V1 живёт только в logging/dev.rs (debug_assertions).
set -euo pipefail

bin="${1:-}"
if [ -z "$bin" ] || [ ! -f "$bin" ]; then
    echo "usage: $0 <release-binary>" >&2
    exit 2
fi

if grep -aq "PODTEKST_DEV_LOGS_V1" "$bin"; then
    echo "::error::в release-бинарнике найден маркер dev-логов: $bin" >&2
    exit 1
fi

echo "release binary clean (no dev-log marker): $bin"
