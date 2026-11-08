# Живой гейт и матрица итогов (H06, M18)

Релизный workflow `.github/workflows/release.yml` запускается только по тегу
`v*` или `workflow_dispatch`; на обычных PR тяжёлая модель не скачивается.
Job `real-summary` — обязательный гейт публикации: `publish` ждёт его успеха.

## Что делает релизный гейт

1. Скачивает pinned runtime `llama.cpp b12182` через
   `.github/scripts/fetch_summary_runtime.sh` (проверка SHA-256 архива,
   `--version`, `--help`, зависимостей и лимита 60 МБ).
2. Скачивает `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` по URL из
   `src-tauri/crates/mr-models/models-manifest.json` и сверяет SHA-256 и размер.
3. Запускает `summary_real` с `SUMMARY_REQUIRE_REAL=1`. Пропуск без env или
   отсутствие строки `summary_real: total=` валят job.
4. Запускает 10-минутный синтетический кейс через
   `.github/scripts/summary_real_matrix.py` (режим по умолчанию `quick`).
5. Пишет `verification/summary-real-manifest.json` с SHA-256 модели и runtime,
   fixture IDs, результатами, reviewer и матрицей; без манифеста job падает.
6. Загружает `summary-real-manifest.json`, `summary-real-matrix.json` и лог как
   артефакт `summary-real-evidence`.

Переменные живого прогона: `MR_SUMMARY_RUNTIME`, `MR_SUMMARY_MODEL`,
`MR_TEST_FIXTURES`, `MR_TEST_TRANSCRIPT`, `SUMMARY_REQUIRE_REAL=1`.
Транскрипты — синтетические fixture-файлы `mr-summarize` (P12: без реальных
записей).

## Полная матрица

Полная матрица — ручной/ночной прогон на машине с runtime и моделью:

```bash
cargo build -p mr-app --locked --manifest-path src-tauri/Cargo.toml
python3 .github/scripts/summary_real_matrix.py \
  --exe src-tauri/target/debug/podtekst \
  --runtime src-tauri/binaries/runtime/x86_64-unknown-linux-gnu \
  --model /path/to/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  --fixtures src-tauri/crates/mr-summarize/tests/fixtures \
  --out verification/summary-real-matrix.json \
  --full --cores 4,8 --runs 3
```

- `--full` добавляет 30/60/120-минутные кейсы и English, injection,
  legal/psychology. Без `--full` выполняется только 10-минутный кейс.
- `--cores 4,8` сужает affinity через `taskset` (Linux), чтобы получить tier
  4–7 и >=8 logical cores. Без флага используется текущая affinity хоста.
- `--runs N` повторяет генерацию; в отчёт пишутся p50/p95. Для коротких кейсов
  рекомендуется 3 прогона, для длинных — 2 (SPEC §15). При `N=1` p50=p95
  измеренного прогона.
- `--enforce-budget` валит прогон при превышении бюджета SPEC §15. Без флага
  превышение только записывается в `budget_met`.

Запускать матрицу целиком имеет смысл на 8+ logical cores и >=16 ГиБ RAM:
120-минутный map-reduce может идти до 30 минут.

## Что попадает в отчёт

`verification/summary-real-matrix.json`:

- `cases[]`: `id`, fixture, duration, `source_chars`, `source_utterances`,
  preset, logical cores, `runs_sec`, `p50_sec`, `p95_sec`, `budget_sec`,
  `budget_source`, `budget_met`, `peak_rss_mb` (VmHWM sidecar), `cancel_ms`,
  проверки языка/injection/sensitive и provenance.
- `not_run[]`: каждый не запущенный кейс с причиной (`quick`, нехватка ядер).
  Кейсы из этого списка **не измерены**, бюджет для них не заявляется.
- `budget_note`: явно фиксирует, что `budget_met` относится только к
  измеренному p95.

Бюджеты SPEC §15: 10 мин — 120 с (>=8 cores) / 240 с (4–7); 30 мин — 240/480;
60 мин — 480/900; 120 мин — 900/1800. SPEC-бюджет 10-минутного кейса относится
к его входу ~9,5k символов; синтетический fixture даёт свой `source_chars`,
который пишется в отчёт. Пока полная матрица не прогнана, 30/60/120-минутные
значения остаются не измеренными; отчёт перечисляет их в `not_run`, а не
выдаёт за пройденные.

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
