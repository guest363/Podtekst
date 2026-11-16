#!/usr/bin/env bash
# P11-аудит: проверяет добавленные в diff строки с комментариями.
# По умолчанию показывает добавленные комментарии для ручной сверки.
# С флагом --check завершается с кодом 1, если найдены запрещённые
# комментарии-пересказы («что делает код»), разделители или английский текст.
# С флагом --test запускает встроенный регрессионный тест эвристики.
set -u

# Один эвристический шаблон на self-test и на --check (P1): «что делает код»,
# разделители, английские глаголы. Комментарий — `//`, `/*`, `#` (Python/YAML/TOML).
what_pattern="^(.*\+.*)?(//|/\*+|(^|[^[:alnum:]_])#([^!]|$))[[:space:]]*([-=_*]{3,}|TODO\b|FIXME\b|[a-zA-Z]+ing\b|check\b|parse\b|returns?\b|calculate\b|get\b|set\b|handle\b|loop\b|helper\b|init(ialize)?\b|update\b|проверяем|проверка\b|проверить|получаем|получение|возвращает|возвращаем|созда[её]м|создание|инициализир|обрабатыва|обработка|добавляем|вызываем|функция\b|метод\b|хелпер\b)"

if [ "${1:-}" = "--test" ]; then
  bad_samples=(
    "+ // Check if file exists"
    "+   // parsing input data"
    "+ // Helper function"
    "+ // Проверяем статус устройства"
    "+ /// Возвращает размер буфера"
    "+ // -----------------------------"
    "+ /* TODO: fix this later */"
    "+/** Возвращает размер буфера */"
    "# проверяем статус устройства"
    "+ # parse response body"
    "+# Helper"
  )
  good_samples=(
    "+/// Cannot-link по 1,8-секундным кускам, а не по репликам"
    "+ // Метрика диаризации не должна зависеть от ASR"
    "+ // Два куска одной длинной реплики"
    "+ // Разрыв уже выбрал k. Порог расстояния"
    "+ // SAFETY: буфер выровнен по 16 байт"
    "+ // shortcut: временный буфер, переделать при stereo"
    "+ // Ограничение CoreAudio: tap требует 48кГц"
    "+ // [P3] Изоляция UI от ядра"
    "# Только пины манифеста: обновление моделей вручную"
    "#!/usr/bin/env bash"
    "+#!/usr/bin/env python3"
  )
  for s in "${bad_samples[@]}"; do
    if ! printf '%s\n' "$s" | grep -qiE "$what_pattern"; then
      echo "p11-diff self-test FAIL: не распознан плохой комментарий: $s" >&2
      exit 1
    fi
  done
  for s in "${good_samples[@]}"; do
    if printf '%s\n' "$s" | grep -qiE "$what_pattern"; then
      echo "p11-diff self-test FAIL: ложное срабатывание на хорошем комментарии: $s" >&2
      exit 1
    fi
  done
  echo "p11-diff: self-test OK"
  exit 0
fi

check_mode=0
ref="HEAD"

for arg in "$@"; do
  case "$arg" in
    --check)
      check_mode=1
      ;;
    *)
      ref="$arg"
      ;;
  esac
done

top="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  echo "P11: не git-репозиторий — проверять нечего." >&2
  exit 2
}
cd "$top" || exit 2
if ! git rev-parse --verify --quiet "$ref" > /dev/null; then
  # Раньше опечатка в ref давала зелёный «чисто» — ложный пропуск гейта.
  echo "P11: неизвестный ref '$ref' — проверка не выполнена." >&2
  exit 2
fi

# Комментарии: // и /* для Rust/TS/CSS, # для Python/YAML/TOML/shell.
# Shebang и `#!`-строки исключаем: это не комментарии.
comment_re='(//|/\*|(^|[^[:alnum:]_])#([^!]|$))'
exclude_re='^[[:space:]]*\+[[:space:]]*#!'

changed="$(git diff --unified=0 "$ref" -- . ':!ui/web/static' ':!scripts/p11-diff.sh' ':!*.md' || exit 2)"
tracked_comments="$(printf '%s\n' "$changed" | grep "^+" | grep -v "^+++" \
  | grep -vE "$exclude_re" | grep -E "$comment_re" | grep -v "://" || true)"

# Новые (untracked) файлы в diff не попадают: ровно их агент и создаёт.
untracked="$(git ls-files --others --exclude-standard)"
untracked_comments=""
if [ -n "$untracked" ]; then
  untracked_comments="$(printf '%s\n' "$untracked" | while IFS= read -r file; do
    case "$file" in
      ui/web/static/*|scripts/p11-diff.sh|*.md) continue ;;
    esac
    [ -f "$file" ] || continue
    grep -nE "$comment_re" "$file" 2>/dev/null | grep -vE '^[0-9]+:[[:space:]]*#!' \
      | grep -v '://' | sed "s|^|+$file:|" || true
  done)"
fi

out="$(printf '%s\n%s\n' "$tracked_comments" "$untracked_comments" | sed '/^$/d')"

if [ -z "$out" ]; then
  echo "P11: добавленных комментариев нет — чисто."
  exit 0
fi

if [ "$check_mode" -eq 1 ]; then
  bad="$(printf '%s\n' "$out" | grep -iE "$what_pattern" || true)"
  if [ -n "$bad" ]; then
    echo "❌ P11: обнаружены запрещённые комментарии («ЧТО делает код» / разделители / англ. язык):"
    printf '%s\n' "$bad"
    echo "---"
    echo "Правило P11: пересказ кода запрещён. Обоснуйте «ЗАЧЕМ» или удалите комментарий."
    exit 1
  fi
fi

printf '%s\n' "$out"
n="$(printf '%s\n' "$out" | wc -l)"
echo "---"
echo "P11: добавленных строк с комментариями: $n — каждая обязана быть «зачем», иначе удалить."
