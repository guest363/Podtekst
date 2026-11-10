#!/usr/bin/env bash
# Pre-push: повторяет гейты CI для фронта и проверяет, что статика закоммичена.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/ui/web/frontend"

yarn lint
yarn check
yarn build

cd "$root"
printf '%s\n' "$(./scripts/frontend-stamp.sh)" > ui/web/static/.mr-stamp

if [ -n "$(git status --porcelain -- ui/web/static)" ]; then
  echo "ui/web/static не совпадает со сборкой — закоммитьте статику и .mr-stamp:" >&2
  git status --short -- ui/web/static >&2
  exit 1
fi
