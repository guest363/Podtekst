# Dev-логи распознавания

Локальный инструмент для разбора неудачных расшифровок: структурированные события
по стадиям, кольцо последних строк для crash-отчёта и артефакты запуска.
Работает только в dev-сборке (`debug_assertions = true`); в release-бинарнике
нет ни кода sink, ни строк событий.

## Запуск

```bash
./run.sh --dev                    # окно Tauri, профиль dev-opt
./run.sh --dev --web              # HTTP API + браузер, без tauri-ui
./run.sh --dev --transcribe запись.wav
```

`--dev` собирает профиль `dev-opt` (наследует release, включает
`debug-assertions`, `overflow-checks`, `debug = 1`, `strip = none`) в
`src-tauri/target/dev*/dev-opt/podtekst`. Обычный `./run.sh` продолжает
собирать `--release` и работает без dev-логов.

Пути по умолчанию:

```text
<app_data>/logs/dev/dev-YYYYMMDD-HHMMSS-pid.log   события
<app_data>/logs/dev-runs/<run_id>/                артефакты запуска
```

`<app_data>` — `$PODTEKST_DATA_DIR`, `$MEETING_RECORDER_DATA_DIR` или
`${XDG_DATA_HOME:-~/.local/share}/podtekst` (Linux); на macOS и Windows —
стандартная папка данных приложения. `run.sh --dev` печатает оба пути перед
запуском.

## Переменные

| Переменная | По умолчанию | Значение |
|---|---|---|
| `MR_LOG` | `info` (в `run.sh --dev`: `info,vad=debug,asr=debug,diar=debug,json=debug`) | `off/error/warn/info/debug/trace`, плюс переопределения `vad=trace,asr=debug` |
| `MR_LOG_FORMAT` | `text` | `text` или `jsonl` |
| `MR_LOG_TEXT` | `preview` | `off`, `preview` (≤ 80 символов), `full` |
| `MR_LOG_PATHS` | `redacted` | `redacted` (имя записи → хеш, домашний каталог → `~`) или `full` |
| `MR_DIAG_DUMP` | `1` в `run.sh --dev` | `0` (только `manifest.json`), `1`, `full` (+ эмбеддинги в `diar/summary.json`) |
| `MR_DIAG_MAX_MB` | `64` | лимит артефактов на запуск |
| `MR_DIAG_RING` | `2048` | последних событий в кольце для crash-отчёта |
| `MR_DIAG_KEEP_RUNS` | `20` | сколько папок `dev-runs` хранить (плюс 200 МБ / 14 дней) |

`MR_LOG=off` не создаёт `dev.log`, но `MR_DIAG_DUMP` продолжает работать.

## Формат событий

Событие — стабильное имя `ev=<target>.<действие>` в тексте сообщения, поля —
`key=value` до `;`:

```text
2026-11-09T14:52:03.400+03:00 INFO  vad    ev=vad.summary run=20261109-145201-7f3a stage=vad ch=0 windows=112500 speech_win=32100 speech_pct=28.5 silent_skip=79000 resets=4 p50=0.02 p95=0.91 segs=42 speech_sec=187.5
```

`MR_LOG_FORMAT=jsonl` даёт те же поля в JSON:

```bash
jq -c 'select(.ev=="vad.summary")' dev.log
```

`run=` и `stage=` добавляются из thread-local контекста; `stage` меняется по ходу
конвейера (`decode`, `models`, `vad`, `asr`, `diar`, `json`). Тайминги стадий
(`stage.start/end`) пишутся на `DEBUG` target `run`: добавьте `run=debug` в
`MR_LOG`, чтобы видеть `elapsed_ms` каждой стадии.

### Карта событий

| Target | События | Смысл |
|---|---|---|
| `run` | `start`, `input`, `done`, `error`, `memory.check`, `stage.start`, `stage.end` | границы запуска и стадий, входные характеристики, итог |
| `audio` | `read`, `decode`, `channel`, `mix`, `bleed`, `capture` | что декодировали, какой канал выбрали, почему подавили эхо, проблемы захвата |
| `vad` | `start`, `summary`, `hist`, `window`, `window.suppressed`, `segment`, `group`, `group.cut`, `merge_quiet` | параметры, распределение вероятностей, найденные/склеенные сегменты |
| `asr` | `engine`, `seg.start`, `seg.done`, `loop`, `empty`, `ctc`, `ctc.skip`, `ctc.hotwords_skipped`, `blank_penalty`, `infer_error`, `text` | вход/выход сегмента, петли, второй проход CTC, ошибки ONNX |
| `diar` | `start`, `units`, `segmentation`, `cluster`, `merge_tiny`, `max_speakers`, `assign`, `embed_batch`, `split`, `reconcile`, `dedupe`, `merge_utts`, `profile.load`, `verify_profiles`, `stats` | почему столько спикеров и как распределены голоса |
| `cluster` | `auto_k`, `auto_k_fallback`, `merge_centroid` | выбор k по разрывам дерева слияний и финальное слияние близких центроидов |
| `json` | `load`, `build`, `utt`, `check`, `invalid`, `roundtrip`, `write`, `edit`, `response` | чтение/запись `.transcript.json`, инварианты, правки UI |
| `model` | `search`, `load`, `warmup`, `load_failed`, `infer_error`, `gpu_fallback` | что искали/загрузили, прогрев сессии, почему модели нет |
| `diag` | `init`, `dropped`, `artifact.truncated`, `artifact.error` | состояние самого dev-sink |

Поля `p50/p95/p99`, `silhouette`, `margin` и `blank_ratio` — числа из самого
конвейера; `iters` в `diar.cluster` не пишется, потому что кластеризатор
не возвращает число итераций.

Уровни: `ERROR` — операция не выполнена; `WARN` — деградация или нарушение
инварианта; `INFO` — одна строка на стадию; `DEBUG` — на сегмент/спикера;
`TRACE` — на окно/слово, с семплированием и лимитом.

## Артефакты запуска

```text
dev-runs/<run_id>/
  manifest.json          версия, commit, ОС, вход, опции, модели, repro
  vad/chN.probs.f32      вероятности по окнам 32 мс (f32 LE)
  vad/chN.segments.json  сегменты до/после группировки
  asr/segments.jsonl     по сегменту: границы, rms, mel, гипотеза, CTC, тайминги
  diar/summary.json      спикеры, silhouette, top-кандидаты (эмбеддинги при full)
  transcript/pre.json    документ до записи
  transcript/post.json   прочитанный обратно после записи
  transcript/invalid.json — только при нарушении инвариантов
  errors.jsonl           события run.error
  README.txt             предупреждение о приватности
```

`manifest.json` — точка воспроизведения: в нём есть полный путь к входу и
команда `repro`. Он лежит только локально и не попадает в экспорт диагностики.

## Как читать

```bash
LOG=<app_data>/logs/dev/dev-*.log

# VAD не увидел речь
rg 'ev=vad\.(summary|hist)|ev=audio\.(channel|mix)' "$LOG" | tail -20

# Фраза потерялась/разрезана
rg 'ev=(vad\.group|vad\.merge_quiet|asr\.seg|asr\.loop|asr\.empty|diar\.merge_utts)' "$LOG"

# Пустая расшифровка в UI
rg 'ev=json\.(load|response|check|invalid)' "$LOG" | tail -30

# Спикеры перепутаны
rg 'ev=diar\.(cluster|assign|merge_tiny|reconcile|stats)' "$LOG"

# Сравнить два запуска
rg 'ev=(vad\.summary|asr\.seg\.done|diar\.cluster|run\.done)' run-a.log > a.txt
rg 'ev=(vad\.summary|asr\.seg\.done|diar\.cluster|run\.done)' run-b.log > b.txt
diff -u a.txt b.txt
```

Зависание: последний `stage.start` без `stage.end` и `asr.seg.start` без
`asr.seg.done`. Для `MR_LOG_FORMAT=jsonl` удобны `jq -c 'select(.ev=="...")'`.

## Приватность (P12)

- Полный текст — только `MR_LOG_TEXT=full`; по умолчанию preview ≤ 80 символов.
- Имена спикеров — `S<hash>`; полные имена только при `MR_LOG_TEXT=full`.
- Имена записей — `recording-<hash>.<ext>`; домашний каталог → `~`.
  Полные пути — только при `MR_LOG_PATHS=full`.
- Аудио, сырые сэмплы, эмбеддинги и `people.json` в события не попадают;
  эмбеддинги пишутся только в локальный `diar/summary.json` при
  `MR_DIAG_DUMP=full`.
- `dev-runs` и `dev` не входят в git (они в папке данных).
- Отчёт диагностики **не** включает dev-логи; `podtekst --diagnostics --with-dev-logs`
  добавляет только последние 200 строк `dev.log` после редакции.

## Отказ от dev-логов в release

`mr-app` включает `log/release_max_level_off`, а `mod dev` существует только
под `#[cfg(debug_assertions)]`. В обычном `cargo build --release` макросы
`log` не порождают кода и строк. Release-CI проверяет бинарник:

```bash
./scripts/check-no-dev-logs.sh src-tauri/target/release/podtekst
# находит PODTEKST_DEV_LOGS_V1 — падает
```

Маркер `PODTEKST_DEV_LOGS_V1` есть только в dev-бинарнике.
