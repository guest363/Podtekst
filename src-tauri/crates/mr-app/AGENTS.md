# mr-app - AGENTS.md

Главный крейт приложения: координация процессов, локальный HTTP REST API, Tauri-оболочка и конвейер транскрибации.

## 1. Архитектура и строгие инварианты

- **Изоляция UI от ML-ядра:** HTTP-слой (`src/http/`) служит единственным мостом между веб-клиентом и бэкендом. Прямые вызовы и импорты из `mr-ml` в HTTP-хендлерах запрещены (P3).
- **Потоковые события (SSE):** прогресс распознавания и диаризации передаётся в веб-клиент в реальном времени через Server-Sent Events (`/api/transcribe/events`). Буферизация событий запрещена: UI должен сразу видеть текущую фазу.
- **Атомарная запись артефактов:** файлы `.transcript.json`, `.md` и `.txt` записываются атомарно через временные файлы с последующим переименованием.
- **Корректное завершение:** при остановке приложения IPC-сервер, HTTP-сервер и сессия записи финализируют заголовки WAV-файлов без потери данных (P7). Во время записи `recording.rs` дописывает WAV и обновляет его RIFF-заголовок каждые 30 секунд.

## 2. Ключевые файлы

- `src/main.rs` - точка входа CLI, разбор флагов (`--web`, `--tray`, `--toggle`), отдача встроенных статических ассетов.
- `src/lib.rs` - общая инициализация сервисов, мобильная точка входа Tauri v2.
- `src/http/` - модули HTTP REST API (`media`, `files`, `people`, `transcribe_api`, `update`).
- `src/transcribe/` - сквозной конвейер транскрибации: `mod.rs` (координатор), `channel_mix.rs` (обнаружение дублей каналов и VAD-гейт пролезания системного звука в микрофон), `echo.rs` (задержка эха и классы кадров: только эхо / только локальная речь / двойная речь / неясно; гейт и фильтр сегментов опираются на них), `diarize.rs` (чистые субсекундные окна эмбеддингов, взвешенная кластеризация с cannot-link), `speaker_assignment.rs` (Венгерский алгоритм для локальных спикеров PyAnnote), `word_speakers.rs` (интервальное дерево и пословная атрибуция), `word_dedupe.rs` (пословная дедупликация эха в микрофонном канале), `events.rs` (SSE-события), `perf.rs` (замеры `MR_PERF=1`, sidecar `.perf.json`), `report.rs` (генерация Markdown и JSON).
- `src/recording.rs` - управление активной сессией записи и таймерами памяти.
- `src/summary/` - локальные итоги (G2/G3): `mod.rs` (флаги/ошибки/SSE-активность), `job.rs` (один job, cancel, preflight), `runner.rs` (worker и RAII-сброс), `view.rs` (`GET /api/summary`, classify), `catalog.rs` (модель/настройки/presets), `sidecar.rs` + `sidecar/helpers.rs` (pinned `llama-server`, токен 0600, stderr без текста, idle 120 с), `process.rs` (PID-файл и orphan cleanup по per-OS start_time), `runtime_locator.rs` (env/adjacent/Resources/Tauri/Linux `usr/lib/Podtekst`/dev + `--version`/`--help`; упаковка — `tauri.summary.conf.json`, `summary-runtime-manifest.json`, `.github/scripts/fetch_summary_runtime.sh`), `gpu.rs` + `gpu/archive.rs` (необязательный Vulkan-пакет: SHA-256, докачка, безопасная распаковка, `GGML_BACKEND_PATH`, CPU fallback; манифест — `summary-gpu-manifest.json`), `memory.rs` (лимиты 12 ГиБ/6,5 ГиБ), `sysmem.rs` (VmHWM / rusage_info_v4 lifetime max / GetProcessMemoryInfo), `store.rs` (`<stem>.summary.json/.md`, 0600, stale/corrupt/unsupported), `pipeline.rs` (prompt → parse → ground → artifact), `config.rs` (D15: heuristic по умолчанию, manual_only — fallback; `gpu=auto|off`). HTTP-адаптер — `src/http/summary_api.rs` (64 КиБ, коды §11); все summary-роуты безусловны, Android — 501, headless без runtime — 503.
- `src/media_registry.rs` - реестр записанных и внешних импортированных файлов.
- `src/metrics.rs` - личная базовая линия метрик (`<app_data>/metrics-baseline.json`, 0600): ключ по голосовому пакету и каналу, версия формата, сброс.
- `src/http/metrics_api.rs` - `GET /api/transcript/metrics` (+ `record=1` для базовой линии),
  `DELETE /api/metrics/baseline`; кэш по mtime расшифровки и WAV.
- `src/desktop.rs` - управление системным треем, глобальным хоткеем и автозапуском.
- `src/ipc.rs` - локальный сокет IPC для команд управления записью из терминала.
- `src/journal.rs` - JSONL-журнал последних сессий записи без аудио и текста.
- `src/crash.rs` + `src/crash/store.rs` - panic hook, маркер запуска и локальные отчёты о падении.
- `src/system/` - снимок системы и проверка настроек; `src/privacy.rs` - единая редакция путей и устройств.
- `src/http/system_api.rs`, `src/http/journal_api.rs` - тонкие адаптеры `/api/system`, `/api/journal`, `/api/crashes`.

## 3. Локальная верификация

```bash
# Юнит-тесты координатора и HTTP API
cargo test -p mr-app --lib

# Строгая проверка clippy (headless сборка)
cargo clippy -p mr-app --all-targets --locked -- -D warnings

# Проверка со сборкой нативного окна Tauri (требуются системные webkit-пакеты)
cargo clippy -p mr-app --features tauri-ui --all-targets --locked -- -D warnings

# Summary: контракт на fake backend и живой smoke на модели (env gate)
cargo test -p mr-app --test summary_api_contract --locked
MR_SUMMARY_RUNTIME=<dir> MR_SUMMARY_MODEL=<file>.gguf \
  MR_TEST_TRANSCRIPT=<file>.transcript.json SUMMARY_REQUIRE_REAL=1 \
  cargo test -p mr-app --test summary_real --locked -- --nocapture
```
`MR_SUMMARY_FAKE_BACKEND` (только debug) — тестовый шов: `1|model_load|invalid|thin|slow`.
Секреты итогов пишутся через `write_atomic_private` (0600/ACL); sidecar-токен — только в
файле, не в argv. Реальные транскрипты и `.gguf` в тесты/репозиторий не попадают (P12).

При выборе канала цифрового дубля сохраняется его исходный индекс в репликах и образцах. Назначение имени во всех режимах требует 2 с чистого аудио; окна не учитываются повторно. Исключение — кластер 0,9–2 с при почти точном совпадении с профилем (неокруглённые similarity ≥ 0,95, margin ≥ 0,05): так узнаётся образец, снятый с короткой реплики. Длинные кластеры матчатся первыми и не конкурируют с коротким; при 1 профиле порог повышен до 0,55, при 2–3 — до 0,50. Неоднозначный источник стереофрагмента при обучении отклоняется до записи профиля; обучение использует тот же AGC-фронтенд, что и диаризация.

Границы `min_speakers`/`max_speakers` (0..=32, `min <= max`) проверяются единой функцией в CLI и HTTP и в активном стерео не дробят `ch0`, передавая `ch1` остаток (`- 1`). Порядок мьютексов в `http/people`: всегда `lock_people` до `lock_voiceprints`.

`transcribe/profile_verification.rs` проверяет именованные реплики после слияний по исходному каналу, исключая перекрытия. После сверки при `min_speakers = 0` в пределах доминирующего канала хвост не больше 25% речи группы крепится по косинусу ≥ 0,70 к самой крупной карточке, а более крупные карточки — только по полной связи; единственное согласованное имя сохраняется при уверенности ≥ 0,78 или если анонимная речь меньше 2 с либо 25% общей речи группы, при нескольких именах единственное уверенное забирает группу, все слабые дают одну анонимную карточку, два и более уверенных блокируют слияние. Эхо проверяется как голос источника, после изменения меток повторяется дедупликация. Достаточность отдельной реплики — от 1,5 с; не подменять этим двухсекундный допуск к имени кластера. Противоречивые реплики не наследуют уверенность общей карточки. Если лучший кандидат прошёл неокруглённые пороги `similarity >= 0,45` и `margin >= 0,015`, но отсеян глобальным `cohort z-score`, он подтверждается только при наличии в этой же записи `matched`-карточки этого профиля с косинусом эмбеддинга `>= 0,55`.

Локальные абляции диаризации задаются `MR_DIAR_EXPERIMENT` только вместе с
`MR_DIAR_BENCH=1`: `anchors=turns|windows`, `centroids=frozen|online`,
`embedding=frontend|raw|unitnorm` (`unitnorm` — RMS-нормализация слайса юнита
к цели AGC перед embed), `guards=on|off` через запятую. По умолчанию —
`turns,frozen,frontend,on`. Конфигурация проверяется до загрузки моделей и
фиксируется на процесс; недопустимые или повторные опции отклоняются.
`guards=off` отключает только отбраковку по покрытию и coverage split/protection;
cannot-link и явные границы числа спикеров сохраняются. Режим записывается
в `ev=diar.experiment` и dev-артефакт `diar/experiment.json`.
