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
- `src/transcribe/` - сквозной конвейер транскрибации: `mod.rs` (координатор), `channel_mix.rs` (обнаружение дублей каналов и VAD-гейт пролезания системного звука в микрофон), `echo.rs` (задержка эха и классы кадров: только эхо / только локальная речь / двойная речь / неясно; гейт и фильтр сегментов опираются на них), `diarize.rs` (чистые субсекундные окна эмбеддингов, взвешенная кластеризация с cannot-link), `speaker_assignment.rs` (Венгерский алгоритм для локальных спикеров PyAnnote), `word_speakers.rs` (интервальное дерево и пословная атрибуция), `word_dedupe.rs` (пословная дедупликация эха в микрофонном канале), `events.rs` (SSE-события), `report.rs` (генерация Markdown и JSON).
- `src/recording.rs` - управление активной сессией записи и таймерами памяти.
- `src/summary/` - локальные итоги (G2/G3): `mod.rs` (флаги/ошибки/SSE-активность), `job.rs` (один job, cancel, preflight), `runner.rs` (worker и RAII-сброс), `view.rs` (`GET /api/summary`, classify), `catalog.rs` (модель/настройки/presets), `sidecar.rs` + `sidecar/helpers.rs` (pinned `llama-server`, токен 0600, stderr без текста, idle 120 с), `process.rs` (PID-файл и orphan cleanup по per-OS start_time), `runtime_locator.rs` (env/adjacent/Resources/Tauri/Linux `usr/lib/Podtekst`/dev + `--version`/`--help`; упаковка — `tauri.summary.conf.json`, `summary-runtime-manifest.json`, `.github/scripts/fetch_summary_runtime.sh`), `gpu.rs` + `gpu/archive.rs` (необязательный Vulkan-пакет: SHA-256, докачка, безопасная распаковка, `GGML_BACKEND_PATH`, CPU fallback; манифест — `summary-gpu-manifest.json`), `memory.rs` (12 ГиБ/6,5 ГиБ, VmHWM), `store.rs` (`<stem>.summary.json/.md`, 0600, stale/corrupt/unsupported), `pipeline.rs` (prompt → parse → ground → artifact), `config.rs` (D15: heuristic по умолчанию, manual_only — fallback; `gpu=auto|off`). HTTP-адаптер — `src/http/summary_api.rs` (64 КиБ, коды §11); все summary-роуты безусловны, Android — 501, headless без runtime — 503.
- `src/media_registry.rs` - реестр записанных и внешних импортированных файлов.
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

При выборе канала цифрового дубля сохраняется его исходный индекс в репликах и образцах. Назначение имени во всех режимах требует 2 с чистого аудио; окна не учитываются повторно. Неоднозначный источник стереофрагмента при обучении отклоняется до записи профиля.

`transcribe/profile_verification.rs` проверяет именованные реплики после слияний по исходному каналу, исключая перекрытия. Эхо проверяется как голос источника, после изменения меток повторяется дедупликация. Достаточность отдельной реплики — от 1,5 с; не подменять этим двухсекундный допуск к имени кластера. Противоречивые реплики не наследуют уверенность общей карточки.
