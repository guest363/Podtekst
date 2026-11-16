#!/usr/bin/env bash
# Бюджеты, ссылки и адаптеры инструкций: расхождение копий и мёртвые пути агент читает
# как правду, поэтому гейт стоит до ревью.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

fail=0
bad() {
  printf 'ai-docs: %s\n' "$*" >&2
  fail=1
}

# Бюджеты калиброваны практикой AGENTS.md: корень — карта, детали — по ссылке.
# Крейтовый контекст шире корня, но не должен становиться второй документацией.
check_budget() {
  file="$1"
  max="$2"
  if [ ! -f "$file" ]; then
    bad "нет файла $file"
    return 0
  fi
  lines="$(wc -l < "$file")"
  if [ "$lines" -gt "$max" ]; then
    bad "$file: $lines строк > $max — вынесите детали по ссылке"
  fi
}

check_budget AGENTS.md 110
check_budget CONTEXT.md 120
check_budget docs/architecture.md 120
check_budget docs/ai/code-style.md 220
check_budget docs/ai/workflow.md 130
check_budget docs/ai/agents-template.md 80
check_budget .agent/rules/audio-record.md 30
check_budget CLAUDE.md 8
for f in src-tauri/crates/*/AGENTS.md ui/web/frontend/AGENTS.md; do
  check_budget "$f" 120
done

# P-идентификаторы стабильны: на них ссылаются код и коммиты.
for n in {1..12}; do
  grep -qE "^## P${n}([^0-9]|$)" docs/ai/code-style.md \
    || bad "docs/ai/code-style.md: пропал раздел P${n}"
done

# Команды живут в verify.sh: копия в карте устаревает молча.
if grep -qE '^[[:space:]]*(cargo|yarn) ' AGENTS.md; then
  bad "AGENTS.md: команды cargo/yarn — только в scripts/verify.sh"
fi

# Адаптер хоста обязан ссылаться на канон, иначе он живёт своей жизнью.
adapter=.agent/rules/audio-record.md
for needle in AGENTS.md docs/ai/code-style.md docs/ai/workflow.md scripts/verify.sh; do
  grep -qF "$needle" "$adapter" || bad "$adapter: нет ссылки на $needle"
done

# Claude Code читает CLAUDE.md вместо AGENTS.md: без импорта карта не видна.
if [ "$(head -n 1 CLAUDE.md | tr -d '[:space:]')" != "@AGENTS.md" ]; then
  bad "CLAUDE.md: первая строка должна быть @AGENTS.md"
fi

# Битая ссылка уводит агента в пустоту: проверяются md-ссылки всех инструкций.
scan=(AGENTS.md CONTEXT.md CONTRIBUTING.md docs/architecture.md docs/ai/code-style.md
  docs/ai/workflow.md docs/ai/agents-template.md .agent/rules/audio-record.md)
bad_links=""
for f in "${scan[@]}"; do
  [ -f "$f" ] || continue
  while IFS= read -r target; do
    case "$target" in
      ''|'#'*|http*|mailto:*) continue ;;
    esac
    target="${target%%#*}"
    [ -z "$target" ] && continue
    if [ ! -e "$target" ] && [ ! -e "$(dirname "$f")/$target" ]; then
      bad_links="$bad_links$f -> $target"$'\n'
    fi
  done < <(grep -oE '\]\([^) ]+' "$f" | sed 's/^](//')
done
if [ -n "$bad_links" ]; then
  bad "битые ссылки:"
  printf '%s' "$bad_links" >&2
fi

# Карта не должна вести в пустоту: путь с каталогом обязан существовать.
map_missing=""
while IFS= read -r p; do
  [ -e "$p" ] || map_missing="$map_missing$p"$'\n'
done < <(grep -oE '`[A-Za-z0-9_./-]+/[A-Za-z0-9_./-]+\.(md|json|yml)`' AGENTS.md | tr -d '`' | sort -u)
if [ -n "$map_missing" ]; then
  bad "AGENTS.md: нет путей:"
  printf '%s' "$map_missing" >&2
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
printf 'ai-docs: OK — бюджеты, ссылки и адаптеры\n'
