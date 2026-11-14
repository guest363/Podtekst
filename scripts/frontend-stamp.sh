#!/usr/bin/env bash
# Единый расчёт штампа исходников фронта: его пишут run.sh и pre-push, сверяет CI.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

find ui/web/frontend/src ui/web/frontend/index.html ui/web/frontend/package.json \
  ui/web/frontend/yarn.lock ui/web/frontend/vite.config.ts -type f -print \
  | LC_ALL=C sort \
  | while IFS= read -r f; do sha256sum "$f" | sed 's/ \*/  /'; done \
  | sha256sum \
  | cut -d' ' -f1
