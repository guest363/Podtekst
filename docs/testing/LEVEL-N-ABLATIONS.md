# Локализация ошибок кандидата N

Этот прогон выполнен пользователем и записан в §15 основного отчёта.
Исправление сборки, объяснение RU frontend и следующий прогон на 18 файлах
описаны в [проверке комбинации](LEVEL-N-COMBINATION-CHECK.md).

N не принят как baseline. Полный прогон пользователя сохранён в [§14 отчёта](BENCHMARK-REPORT.md). Последующие изменения добавляют измерительные средства; обычные параметры и алгоритмические решения N не меняются. Тесты, сборка и инференс агентами не запускались.

## Почему пока не меняем VAD

По четырём сохранённым main RTTM подтверждается miss из CSV, но подходящих VAD/ASR dev-артефактов этих входов нет ни в каталоге benchmark, ни в стандартной папке данных. У доступных 14 benchmark dev-run другие входы; SHA-256 не совпадают.

В 10-мс окнах пропущенного времени RMS ниже −50 dBFS составляет 7,16 из 7,76 с для EN normal, 8,74 из 9,02 с для EN quiet, 9,96 из 11,12 с для RU normal и 9,00 из 9,47 с для RU quiet. Низкая энергия не доказывает отсутствие речи. Однако whole-clip RTTM включает внутренние паузы, а итоговые границы RU совпадают с границами слов. Поэтому из одного DER miss нельзя заключить, что всё это время потерял детектор. Пороги, speech padding и исходный RTTM сохранены.

## Независимые абляции

`MR_DIAR_EXPERIMENT` разрешён только при `MR_DIAR_BENCH=1`. Конфигурация фиксируется на процесс, проверяется до моделей и записывается в `diar/experiment.json` и `ev=diar.experiment`. Неизвестные/повторные ключи и пустая строка отклоняются; для обычного N переменную нужно удалить, а не установить в пустую строку.

| Опция | По умолчанию N | Альтернатива |
|---|---|---|
| `anchors` | `turns` | `windows`: локальные окна формируют AHC, отдельные опоры не извлекаются |
| `centroids` | `frozen` | `online`: центроиды обновляются после назначения каждого окна |
| `embedding` | `frontend` | `raw`: исходный сигнал только для финальной диаризации; VAD/поиск границ сохраняют прежний вход |
| `guards` | `on` | `off`: пропускаются обе отбраковки малых кластеров и coverage split/protection |

В `guards=off` сохраняются cannot-link, min/max speakers, прежние tiny/centroid merge, сглаживание и PyAnnote assignment. Это не полный откат к старому коду. Комбинация `anchors=windows,centroids=online,embedding=raw` помогает проверить совместное влияние изменений, но также не подменяет сохранённый бинарник M. Каждый компонент сначала проверяется отдельно. ASR-сигнал и ASR-фикс не переключаются.

## Короткий прогон пользователем

Сначала локальные проверки новых веток:

```bash
cargo test -p mr-app --locked --manifest-path src-tauri/Cargo.toml --lib transcribe::experiment
ROOT="$HOME/.cache/podtekst-bench"
PY="$ROOT/venv/bin/python"
"$PY" -m unittest discover -s tools/diar_eval -p 'test_*.py'
```

После штатных проверок компиляции один dev-opt бинарник нужен для всех режимов:

```bash
cargo build --profile dev-opt -p mr-app --locked --manifest-path src-tauri/Cargo.toml
OUT="$ROOT/runs/n-ablation-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
cp "$PWD/src-tauri/target/dev-opt/podtekst" "$OUT/podtekst"
APP="$OUT/podtekst"
export PODTEKST_DATA_DIR="$ROOT/appdata" MR_DIAR_BENCH=1
export MR_DIAG_DUMP=1 MR_LOG_TEXT=off
export MR_LOG='info,run=debug,vad=debug,diar=debug,cluster=debug'
unset MR_DIAR_EXPERIMENT

"$PY" tools/diar_eval/run_batch.py --app "$APP" \
  --manifest "$ROOT/diar-main-meetings/normal.json" --out "$OUT/main-normal" \
  --dev-runs-dir "$ROOT/appdata/logs/dev-runs"
"$PY" tools/diar_eval/run_batch.py --app "$APP" \
  --manifest "$ROOT/diar-main-meetings/quiet.json" --out "$OUT/main-quiet" \
  --dev-runs-dir "$ROOT/appdata/logs/dev-runs"

"$PY" tools/diar_eval/run_batch.py --app "$APP" \
  --manifest "$ROOT/runs/level-fix/manifests/synthetic4.json" --out "$OUT/historical-default" \
  --dev-runs-dir "$ROOT/appdata/logs/dev-runs"
for experiment in anchors=windows centroids=online embedding=raw guards=off; do
  MR_DIAR_EXPERIMENT="$experiment" "$PY" tools/diar_eval/run_batch.py --app "$APP" \
    --manifest "$ROOT/runs/level-fix/manifests/synthetic4.json" \
    --out "$OUT/historical-${experiment%%=*}" \
    --dev-runs-dir "$ROOT/appdata/logs/dev-runs"
done
```

Это четыре main-файла и исторические четыре в пяти режимах. Скорость dev-opt нельзя сравнивать с прежним release. Затем варианты, объяснившие регресс, проверяют на прежнем Vox5. Новый полный ASR450/матрица нужен после выбранной алгоритмической правки, а не для каждой диагностической ветки.

## Где пропало покрытие

`--dev-runs-dir` копирует dev-run каждого входа в результаты **до запуска следующего файла**, включая геометрию стадий и журналы stdout/stderr. Подходящий dev-журнал с тем же run ID сохраняется как `events-N.log`. Связь проверяется по новому manifest, пути и SHA-256 аудио; отсутствие или неоднозначность считается ошибкой строки. Колонка `dev_run` содержит путь к копии. Это защищает от ротации dev-артефактов при последующих запусках.

В новом run сохраняются отдельные сегменты и вероятности VAD raw/frontend, существующие combined/grouped и геометрия `asr → pre-cluster → acoustic → document`. Последний этап находится до финальной межканальной дедупликации и проверки профилей; итоговый документ доступен отдельно в `transcript/post.json`.

По пути `dev_run` нужной строки выполнить:

```bash
"$PY" tools/diar_eval/stage_coverage.py --run /путь/из/колонки/dev_run \
  --reference "$ROOT/diar-main-meetings/main-en-2spk-60s-ov0-normal/meeting.rttm" \
  --audio "$ROOT/diar-main-meetings/main-en-2spk-60s-ov0-normal/meeting.wav" \
  --out "$OUT/coverage-en-normal.json"
```

Анализатор проверяет SHA-256 входа, считает точные объединения интервалов и потери/возврат покрытия между стадиями. Это **speech union coverage**, без назначения меток и кратности перекрывающейся речи; оно не является DER и не используется вместо прежнего gate. Если combined уже не покрывает эталон, нужен разбор входа и независимых границ речи. Если покрытие исчезло после ASR или word assignment, изменение VAD не исправит этот этап.

## Сохранность результатов

`run_batch.py` отказывается заменять существующий `metrics.csv` без явного `--overwrite`. Манифест запуска содержит SHA-256 бинарника, параметры эксперимента и каталог dev-run; git commit обозначает checkout, а не доказывает исходники бинарника. Env-gated регрессия создаёт уникальный каталог каждого прогона.

Перезаписанные per-file строки M Crowd300 не восстанавливаются из агрегата WER. Агрегаты §13 остаются историческим источником; новое парное сравнение с M Crowd без исходных строк не заявляется. Сохранившийся базовый прогон может использоваться для отдельного парного сравнения.
