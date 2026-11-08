# Живой гейт и матрица итогов (H06, M18)

Релизный workflow `.github/workflows/release.yml` запускается только по тегу
`v*` или `workflow_dispatch`; на обычных PR тяжёлая модель не скачивается.
Job `real-summary` — обязательный гейт публикации: `publish` ждёт его успеха.

## Что делает релизный гейт

1. Скачивает pinned runtime `llama.cpp b12182` через
   `.github/scripts/fetch_summary_runtime.sh` (проверка SHA-256 архива,
   `--version`, `--help`, зависимостей и лимита 60 МБ).
2. Скачивает `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` через
   `.github/scripts/fetch_summary_model.sh` (URL/SHA-256/размер из
   `src-tauri/crates/mr-models/models-manifest.json`).
3. Запускает `summary_real` с `SUMMARY_REQUIRE_REAL=1` на коротком
   `scrum-ru.json`. Пропуск без env или отсутствие строки
   `summary_real: total=` валят job.
4. Запускает один 10-минутный синтетический кейс через
   `.github/scripts/summary_real_matrix.py` с
   `--budget-profile ci-loaded --enforce-budget`.
5. Пишет `verification/summary-real-manifest.json` с SHA-256 модели и runtime,
   fixture IDs, результатами, reviewer, профилем бюджета, измерениями
   `p50/p95/budget_met` и списком `not_run`; без манифеста или без
   подтверждённого 10-минутного бюджета job падает.
6. Загружает `summary-real-manifest.json`, `summary-real-matrix.json` и лог как
   артефакт `summary-real-evidence`.

Переменные живого прогона: `MR_SUMMARY_RUNTIME`, `MR_SUMMARY_MODEL`,
`MR_TEST_FIXTURES`, `MR_TEST_TRANSCRIPT`, `SUMMARY_REQUIRE_REAL=1`.
Транскрипты — синтетические fixture-файлы `mr-summarize` (P12: без реальных
записей).

Релизный гейт намеренно запускает **только** 10-минутный кейс. English,
injection, legal/psychology, 30/60/120 минут и tier 8 в обычный релиз не
входят; их покрывает отдельный ручной запуск ниже.

## Бюджетный профиль

`--budget-profile ci-loaded` — единственный профиль, который применяется в CI:

| Профиль | 10 мин | 30/60/120 мин | Где применяется |
|---|---|---|---|
| `spec` | 120 с (>=8) / 240 с (4–7) | 240/480, 480/900, 900/1800 | только справка; CI не гейтит |
| `ci-loaded` | **600 с для обоих tier** | бюджета нет | release quick + full matrix |

600 с — не SPEC-обещание, а зафиксированный замер: 10-минутный кейс
(`source_chars=7854`) занял **595.68 с** под нагрузкой (20 logical, 8 threads).
Поэтому в профиле `ci-loaded` стоит 600 с, а не 120 с. Для 30/60/120 минут
измерений нет: `budget_sec=null`, `budget_met=null`, они попадают в `not_run`
или измеряются без бюджетного гейта. Утверждение «120 с на 4 ядрах» не
делается.

`--enforce-budget` валит прогон, если измеренный p95 превышает бюджет кейса.
Кейсы без бюджета (30/60/120 в `ci-loaded`) не валят прогон и остаются
явно неоценёнными.

## Полная матрица (workflow_dispatch)

Полная матрица **не** запускается на теге или обычном релизе. В release.yml
есть input `full_matrix` (по умолчанию `false`) и job `full-matrix`:

```bash
gh workflow run release.yml --ref <branch> \
  -f full_matrix=true \
  -f matrix_runner=ubuntu-24.04
```

Job выполняет ровно:

```bash
python3 .github/scripts/summary_real_matrix.py \
  --exe src-tauri/target/debug/podtekst \
  --runtime src-tauri/binaries/runtime/x86_64-unknown-linux-gnu \
  --model "$MR_SUMMARY_MODEL" \
  --fixtures src-tauri/crates/mr-summarize/tests/fixtures \
  --out verification/summary-real-matrix-full.json \
  --full --cores 4,8 --runs 1/3 \
  --budget-profile ci-loaded --enforce-budget
```

и загружает `summary-real-matrix-full.json` + лог артефактом
`summary-real-matrix-full`. `matrix_runner` нужен для tier 8: на стандартном
4-ядерном runner он честно попадёт в `not_run` с причиной «хост даёт 4
logical cores»; полный tier 8 требует self-hosted Linux x86_64 runner с >=8
logical cores.

Локально те же кейсы:

```bash
cargo build -p mr-app --locked --manifest-path src-tauri/Cargo.toml
python3 .github/scripts/summary_real_matrix.py \
  --exe src-tauri/target/debug/podtekst \
  --runtime src-tauri/binaries/runtime/x86_64-unknown-linux-gnu \
  --model /path/to/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  --fixtures src-tauri/crates/mr-summarize/tests/fixtures \
  --out verification/summary-real-matrix-full.json \
  --full --cores 4,8 --runs 1/3 \
  --budget-profile ci-loaded --enforce-budget
```

- `--full` добавляет 30/60/120-минутные кейсы и English, injection,
  legal/psychology. Без `--full` выполняется только 10-минутный кейс.
- `--runs 1/3` читается как `LONG/SHORT`: 1 прогон для 30/60/120, 3 для
  10-минутных кейсов (включая quality-кейсы). Обычное `--runs N` задаёт
  одно число для всех.
- `--cores 4,8` сужает affinity через `taskset` (Linux), чтобы получить tier
  4–7 и >=8 logical cores. Без флага используется текущая affinity хоста.
- `--enforce-budget` проверяет только кейсы, у которых в профиле есть бюджет.

Запускать полную матрицу имеет смысл на 8+ logical cores и >=16 ГиБ RAM:
120-минутный map-reduce может идти десятки минут.

## Что попадает в отчёт

`verification/summary-real-matrix*.json`:

- `budget_profile`, `budget_profile_note`, `budget_enforced`, `runs_short`,
  `runs_long`.
- `cases[]`: `id`, fixture, duration, `source_chars`, `source_utterances`,
  preset, logical cores, `runs_per_case`, `runs_sec`, `p50_sec`, `p95_sec`,
  `budget_sec`, `budget_source`, `budget_met`, `peak_rss_mb` (VmHWM sidecar),
  `cancel_ms`, проверки языка/injection/sensitive и provenance.
- `not_run[]`: каждый не запущенный кейс с причиной (`quick`, нехватка ядер).
  Кейсы из этого списка **не измерены**, бюджет для них не заявляется.
- `budget_note`: явно фиксирует, что `budget_met` относится только к
  измеренному p95 и зафиксированному `source_chars`.

В `verification/summary-real-manifest.json` дополнительно есть
`budget.profile`, `budget.enforced`, `budget.measurements[]` с `p50_sec`,
`p95_sec`, `budget_sec`, `budget_met`, `peak_rss_mb`, `cancel_ms`, а также
`not_run` и полная матрица. Так релизное evidence не теряет ни измеренные
значения, ни непройденные кейсы.

## Смоук и метрика бандла

`smoke-linux` проверяет размер установленного pruned-runtime: бюджет 60 МиБ
(SPEC §15) применяется к каталогу `runtime/<triple>/`, а не к полному
AppImage. Полный размер AppImage пишется в `bundle-size.json` как метрика и
загружается артефактом `bundle-size-linux`.

## APK evidence (S-11)

`smoke-android` пишет `apk-evidence.json` по фактическому файлу из
`tauri-bundles-android`: путь, размер, SHA-256, `versionName`, `versionCode`,
ABI, `build_type=release` и тип подписи (`release`/`debug`). Step падает, если
APK отсутствует, пустой, SHA-256 не совпал, `versionCode` не положительный или
в ABI нет `arm64-v8a`. Файл загружается артефактом `apk-evidence`.

Размер APK из `G6-REPORT.md` — исторический локальный замер, а не бюджет:
он зависит от toolchain, R8 и ключа подписи. Авторитетное значение для
конкретного релиза — `apk-evidence.json` этого запуска.

## Reviewer

В манифесте `reviewer` — GitHub actor запуска. Второй оценщик
(`second_rater`) заполняется вручную при приёмке живых выходов (H14); CI его
не подменяет.
