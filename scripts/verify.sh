#!/usr/bin/env bash
# Единая точка входа для гейтов: правка команды не должна синхронизировать копии
# в документах, поэтому документы ссылаются на режимы, а не на команды.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

usage() {
  cat <<'EOF'
verify.sh — гейты репозитория.

  --fast    P11 и доки: секунды, без сборки
  --docs    только бюджеты и ссылки инструкций
  --rust    fmt, clippy, тесты workspace
  --ui      lint, check, build, штамп статики
  --tauri   clippy с tauri-ui (нужны системные WebKit-пакеты)
  --all     всё перечисленное
  -h, --help

Без аргументов — справка. Режимы можно комбинировать.
EOF
}

steps=()
seen=" "
add_step() {
  case "$seen" in
    *" $1 "*) return 0 ;;
  esac
  seen="$seen$1 "
  steps+=("$1")
}

for arg in "$@"; do
  case "$arg" in
    --fast) add_step p11; add_step docs ;;
    --docs) add_step docs ;;
    --rust) add_step fmt; add_step clippy; add_step test ;;
    --ui) add_step ui ;;
    --tauri) add_step tauri ;;
    --all)
      add_step p11
      add_step docs
      add_step fmt
      add_step clippy
      add_step test
      add_step ui
      add_step tauri
      ;;
    -h|--help) usage; exit 0 ;;
    *) echo "verify: неизвестный режим '$arg'" >&2; usage >&2; exit 2 ;;
  esac
done

if [ "${#steps[@]}" -eq 0 ]; then
  usage >&2
  exit 2
fi

# Во время правок P11 должен видеть рабочее дерево, в pre-push — диапазон пуша:
# иначе пустой diff скрывает отправляемое, а upstream — локальные правки.
p11_ref() {
  if [ -n "${MR_P11_REF:-}" ]; then
    printf '%s' "$MR_P11_REF"
    return 0
  fi
  if [ -n "$(git status --porcelain)" ]; then
    printf 'HEAD'
    return 0
  fi
  if upstream="$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null)"; then
    printf '%s...HEAD' "$upstream"
    return 0
  fi
  if git rev-parse --verify --quiet origin/main >/dev/null; then
    printf 'origin/main...HEAD'
    return 0
  fi
  printf 'P11: нет upstream и origin/main — видны только незакоммиченные правки.\n' >&2
  printf 'HEAD'
}

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

for step in "${steps[@]}"; do
  case "$step" in
    fmt) run cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check ;;
    clippy) run cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings ;;
    test) run cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml ;;
    tauri) run cargo clippy -p mr-app --features tauri-ui --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings ;;
    ui) run bash scripts/check-frontend.sh ;;
    p11) run scripts/p11-diff.sh --check "$(p11_ref)" ;;
    docs) run scripts/ai-docs-check.sh ;;
  esac
done

printf '\nverify: OK (%s)\n' "${steps[*]}"
