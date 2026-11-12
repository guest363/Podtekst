# Следующий шаг после абляций N

## Итог §16 и текущие изменения

Сравнение выполнено пользователем (`13c75f7`). Комбинация windows/raw не
принимается: main совпадает с default, DER Vox5 ухудшился с 35,56% до 37,79%.
Обычный режим, VAD и пороги сохранены. В команде Vox5 ниже исправлен
пропущенный `--data-root "$ROOT/data"`.

Причина тихого EN установлена по исходным отсчётам и сохранённому scaling.
Первый EN-источник имеет crest factor 40,22 дБ: максимум на 10,41975 с,
выше половины максимума — четыре отсчёта. После усиления его пик 10,251
заставляет общий headroom gain 0,096575 уменьшить уровень на 20,30 дБ.
Target RMS 0,1 до headroom становится примерно 0,0096575 (−40,30 dBFS).
Для RU headroom gain 0,890160 даёт около −21,01 dBFS. Поэтому EN normal
и RU normal не были выровнены по фактическому уровню. Пик и исходные WAV
не изменены; измеренные наборы не пересоздавались.

Генератор дополнительно сохраняет `headroom_gain_db`,
`source_rms_post_headroom` и `track_rms_post_headroom`. Уровень источника
относится к полному клипу после gain/headroom, до PCM-квантования; это не
VAD-оценка речи. Изменение метаданных не меняет отсчёты.

Во встроенном self-test генератора исправлен assert, обращавшийся к
`normal_info` в другом тесте: он перенесён в тест нормализованной пары.
Встроенные тесты подключены к unittest discovery. Агент их не запускал.

## Независимое прослушивание

Фрагменты и provenance сохранены локально в
`$ROOT/runs/n-combination-20261112-210124/audition-en-13c75f7/`.
Исходный A содержит контекст 8,70–10,85 с; проверяемый участок —
8,99–10,68 с (0,29–1,98 с внутри фрагмента). B сначала содержал контекст
10,75–12,05 с; затем для уточнения вырезан ровно участок 10,97–11,90 с
без контекста. Для B применён только постоянный listening gain 13,7438
без клиппинга; он не используется в конвейере или эталоне.

Пользователь подтвердил A как паузу/дыхание/шум. В конце точного B
пользователь слышит начало речевого звука. Точная граница его появления
не установлена; B сохраняется в эталоне целиком. Отсутствие слов ASR
не используется как доказательство неречевого участка.

## Отдельный частично уточнённый эталон

Создан локальный каталог
`$ROOT/reference-versions/main-en-human-a-partial-v1/`. Он содержит
`annotation.json`, воспроизводящий `build_reference.py`, manifest и два
EN RTTM. A размечен в координатах одного исходного клипа; проверены
SHA-256 клипа, source proof и порядок источников. Только этот подтверждённый
неречевой хвост исключён в трёх повторениях того же источника: по 5,07 с
для normal и quiet. Другие интервалы, B и весь RU не корректировались.
Исходные RTTM не переписаны, их SHA-256 записаны в annotation/manifest.

Это частичное уточнение по прослушиванию, не полностью размеченный speech
reference и не принятый baseline. Новый DER агент не считал. Его можно
получить из готовых гипотез без инференса:

```bash
ROOT="$HOME/.cache/podtekst-bench"
PY="$ROOT/venv/bin/python"
REF="$ROOT/reference-versions/main-en-human-a-partial-v1"
RUN="$ROOT/runs/n-combination-20261112-210124"
for mode in default windows-raw; do
  for variant in normal quiet; do
    name="main-en-2spk-60s-ov0-$variant"
    "$PY" tools/diar_eval/evaluate.py "$REF/$name.rttm" \
      "$RUN/$mode-main-$variant/rttm/$name.rttm" --per-file \
      > "$REF/score-$mode-$variant.txt"
  done
done
```

Новые числа следует указывать рядом с прежними с явной версией эталона.
Они не исправляют и не отменяют регресс относительно старой базы Vox5.
Ни текущий VAD, ни его вероятности не использовались для выбора границ A.

## Предыдущий регламент

Прогон пользователя `runs/n-ablation-20261112-195552` и §15 основного отчёта сохраняются. Он выполнен на копии дерева с исправленным hex-кодированием; это диагностическая правка и не основание отклонять полученные сравнения.

В основном дереве оба несовместимых вызова `LowerHex` удалены: manifest входа и пример `embedding_separability` используют существующий `mr_models::sha256_of_file`. Новых зависимостей и второй реализации SHA-256 нет. Сборка и тесты агентом не запускались.

## RU frontend — повторный проход не требовался

В сохранённых RU main-логах `amplified_fraction=0`, `gain_p95=1`, `normalized=false`. Speech RMS p90 равен 0,1562 для normal и 0,1179 для quiet, выше target 0,1. Bridge сохранил исходный канал; VAD выполнил один проход. Отсутствие отдельного frontend-файла здесь не означает потерю вероятностей.

Новая диагностика сохраняет frontend-вероятности также для этого случая, с `reused_from: raw`. Модель повторно не вызывается. `stage_coverage.py` переносит эту пометку в отчёт. Старые артефакты не переписываются.

## Сначала сборка

Команды выполняет пользователь:

```bash
cargo check -p mr-app --all-targets --locked --manifest-path src-tauri/Cargo.toml
cargo test -p mr-app --locked --manifest-path src-tauri/Cargo.toml --lib transcribe::experiment
ROOT="$HOME/.cache/podtekst-bench"
PY="$ROOT/venv/bin/python"
"$PY" -m unittest discover -s tools/diar_eval -p 'test_*.py'
```

Проверка `--all-targets` включает пример, где находился второй несовместимый вызов.

## Только main и Vox5 на одном бинарнике

Сравнить default N с `anchors=windows,embedding=raw`: 8 main-файлов и 10 Vox-файлов в двух режимах. Центроиды, guards, VAD, ASR и пороги остаются прежними. Комбинация пока не становится обычным режимом: исторические метки клипов сами по себе её не оправдывают.

```bash
cargo build --profile dev-opt -p mr-app --locked --manifest-path src-tauri/Cargo.toml
OUT="$ROOT/runs/n-combination-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
cp "$PWD/src-tauri/target/dev-opt/podtekst" "$OUT/podtekst"
APP="$OUT/podtekst"
export PODTEKST_DATA_DIR="$ROOT/appdata" MR_DIAR_BENCH=1
export MR_DIAG_DUMP=1 MR_LOG_TEXT=off
export MR_LOG='info,run=debug,vad=debug,diar=debug,cluster=debug'

run_set() {
  label="$1"
  manifest="$2"
  shift 2
  "$PY" tools/diar_eval/run_batch.py --app "$APP" --manifest "$manifest" \
    --out "$OUT/$mode-$label" --dev-runs-dir "$ROOT/appdata/logs/dev-runs" "$@"
}
for mode in default windows-raw; do
  if [ "$mode" = default ]; then
    unset MR_DIAR_EXPERIMENT
  else
    export MR_DIAR_EXPERIMENT='anchors=windows,embedding=raw'
  fi
  run_set main-normal "$ROOT/diar-main-meetings/normal.json"
  run_set main-quiet "$ROOT/diar-main-meetings/quiet.json"
  run_set vox5 "$ROOT/runs/level-fix/manifests/vox5.json" --data-root "$ROOT/data" --engine whisper --language en
done
unset MR_DIAR_EXPERIMENT
```

Одна копия бинарника исключает смену кода между режимами. Сравниваются DER-компоненты, число спикеров и атрибуция; скорость dev-opt не сравнивается с release. Историческую синтетику и ASR450 для этой диагностической правки повторять не требуется.

## Какие интервалы проверять на речь

Новый `stage_coverage.py` добавляет `reference_union_covered_sec` и точный список `reference_union_uncovered_intervals`. Последний пригоден для прослушивания и независимой разметки; низкая энергия сама по себе не считается тишиной.

Без запуска моделей агент пересчитал только геометрию существующих четырёх main-прогонов. Новые JSON лежат в `$ROOT/runs/n-ablation-20261112-195552/review-intervals-8a9b801/`; исходные отчёты не заменены. В combined обнаружены 11 / 12 интервалов miss для EN normal/quiet и 10 / 11 для RU normal/quiet.

`hypothesis_union_sec` — вся длительность гипотезы, включая участки вне эталона; это не длительность покрытого эталона. Например, EN normal: гипотеза 49,344 с, покрытый эталон 48,821 с, вне эталона 0,523 с, miss 8,359 с.

Если независимая проверка обнаружит речь в miss, исследовать recall VAD на этих интервалах. Если подтвердит паузы, сохранить исправленную разметку отдельной версией и пересчитать сравниваемые кандидаты на ней. Исходный RTTM и прежние метрики сохраняются; обрезать эталон по VAD проверяемого приложения нельзя.
