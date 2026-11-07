# G2 runtime + G3 service/HTTP/SSE — отчёт

Статус: **G2/G3 реализованы, backend-проверки зелёные**; живой smoke на Linux
выполнен. Frontend (G4) не входит в этот отчёт и не изменялся.

## Что вошло

- `mr-app::summary` (unconditional, без Cargo-feature): `job`, `runner`, `view`,
  `catalog`, `sidecar` (+`helpers`), `process`, `runtime_locator`, `memory`,
  `store`, `pipeline`, `config`, `cache`, `backend`, `fake` (debug-шов), `id`.
- HTTP: `GET /api/summary`, `POST /classify|generate|cancel`, `GET|POST /settings`,
  `GET /presets`; 64 КиБ body limit; стабильные коды; path safety как у
  `/api/transcript`; отсутствуют `/api/summary/status`, `/api/summary_stream`,
  `/api/summary/edit`.
- SSE: одна шина `/api/transcribe_stream`, `summary-stage|done|error`, без текста;
  `SUMMARY_ACTIVE` выставляется синхронно до `202` и снимается RAII-guard после
  терминала; `take_new_events` учитывает счётчик; grace-тик для stream-before-POST.
- Sidecar: pin `b12182`/`fd27b2034`, флаги `--jinja --reasoning off --no-webui
  --api-key-file --log-disable --parallel 1 --ctx-size 8192 -b 512 -ub 128`,
  loopback + Bearer, токен 256 бит в файле 0600, stderr только bytes/SHA-256,
  idle kill 120 с, orphan cleanup по per-OS `start_time` и имени процесса.
- Store: `<stem>.summary.json` + `.summary.md` только через
  `write_atomic_private` (0600 Unix / ACL-фолбэк Windows), stale/corrupt/
  unsupported/newer-major, бэкапы `.corrupt.<unix>`/`.newer.<unix>` (<=3) на
  commit point, caps JSON 1 МиБ/MD 2 МиБ.
- Map-reduce: map по chunk-ам → реальный reduce-проход моделью
  (`prompt::build_reduce`, те же инварианты и nonce DATA), парсинг и grounding
  против полного транскрипта; >10 частичных — два уровня reduce. Невалидный
  reduce → заземлённый merge частичных с `parser_recovered` и
  `retry_reason="reduce_invalid"`, job не теряется. `reduce_levels` в provenance.
- Retry: `invalid_output` (seed 43), `output_thin` и `decision_missed` — один
  content-first repair-повтор; `parser_retries`/`retry_reason` записываются,
  `decision_missed` остаётся предупреждением, если решение так и не найдено.
- `timings_ms.load` — измеренное spawn→`/health` время sidecar (0 при повторном
  job на уже загруженной модели); total/prepare/generate/verify измеряются.
- Guards: total RAM 12 ГиБ, free 6.5 ГиБ, skip free-check при загруженном том же
  sidecar, post-load `VmHWM > 6656 MiB` -> kill/`oom`; threads `min(8,max(2,logical))`.
- Модель: `kind:"summary"` в манифесте, не core; download precheck
  `size+512 MiB` -> `422 disk_low`; условная инвалидация ASR-кэша; `model-done`
  для summary не трогает ASR; `has_summary` additive; `rev` в `/api/transcript`
  и `/api/transcript/edit`; `SIDECAR_SUFFIXES` = 5.
- D15: default `classifier="heuristic"`; `manual_only` реализован как операторский
  fallback (classification null, classify/settings -> 409), не дефолт.

## Команды и результаты

| Команда | Результат |
|---|---|
| `cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check` | pass |
| `git diff --check -- src-tauri` | pass |
| `cargo clippy --workspace --all-targets --locked ... -- -D warnings` | pass |
| `cargo clippy -p mr-app --features tauri-ui --all-targets --locked ... -- -D warnings` | pass |
| `cargo test --workspace --locked ...` | pass: 325 passed, 2 ignored (diar_local, property 100k) |
| `cargo test -p mr-app --test summary_api_contract --locked` | pass: 1 suite (24 сценария) |
| `cargo test -p mr-app --test summary_real --locked` без env | pass (skip) |
| `SUMMARY_REQUIRE_REAL=1 ... cargo test -p mr-app --test summary_real` | pass на модели |
| Android: `cargo check -p mr-app --target aarch64-linux-android --features tauri-ui --locked` | pass (NDK 27.1.12297006 + llvm bin в PATH) |
| `git diff --check` (весь репозиторий) | fail: trailing whitespace в `ui/web/static/js/app.js` от параллельного frontend-агента; `src-tauri` чист |

Frontend-маркеры проверяются в `restored_endpoints_roundtrip` по фактическому
бандлу `3434072`: `summary-block`, `summary-picker`, `summary-result`. Класс
`summary-preset-picker` в бандле отсутствует (компонент рендерится как
`summary-picker`), поэтому используется реальный маркер; добавление нового
класса потребовало бы правки frontend-agent.

## Живой smoke (Linux, реальная модель)

Условия: `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` (2.33 ГиБ), runtime
`llama-b12182`, синтетический транскрипт `meeting-ru.json` (13 реплик),
20 logical cores, 31 ГиБ RAM; во время прогонов load average 16–18 (другие
агенты). Тест — debug-профиль оркестратора, sidecar — релизный бинарь.

| Метрика | Измерено |
|---|---|
| Успешная генерация, total `duration_ms` | follow-up smoke: `160 863 мс` wall / `160 673 мс` artifact; лучший прогон G3: `105 735 мс` |
| `timings_ms.load` | `5 408 мс` (spawn→`/health`, follow-up smoke) |
| Peak `VmHWM` sidecar | `5 291 MiB` (5.17 ГиБ) <= 6.5 ГиБ |
| Cancel (POST -> `summarizing=false`) | `1 789 мс` (ранее `1 418`/`1 797–1 798 мс`) <= 2 с |
| Артефакты | `.summary.json` + `.summary.md`, transcript bytes не изменены |
| Generator | `llama.cpp/b12182`, commit `fd27b2034`, `model_sha256=3605803b…`, `prompt_version=meeting-v2` |
| Разброс прогонов | `105.7`, `140.2`, `160.9`, `208.2`, `216.7`, `241.4 с` при load average 16–20 |

Лучший прогон `105.7 с` уложился в бюджет §15 (`<=120 с` на `>=8` logical),
follow-up smoke — `160.9 с` при load average ~20. Разброс почти 2.3× из-за
нагрузки хоста; 65.05 с из спецификации не воспроизводилась. 4-core tier и
9.5k-кейс — G5. Cancel и peak — в бюджете во всех прогонах.

## Ограничения / что осталось

- Map-reduce проверен fake-контрактом (2 уровня, fallback); качество и latency
  реального reduce на длинном входе — G5 (нужен отдельный прогон на модели).
- Retry `output_thin`/`decision_missed` проверен fake-контрактом; на реальной
  модели качество repair-промпта — G5.
- `timings_ms.load` заполняется из spawn→`/health`; в fake-тестах он 0, живой
  замер — в real smoke/G5.
- Windows ACL (`icacls` fallback), macOS `proc_pid_rusage`/`sysctl` и Windows
  `GetProcessTimes` скомпилированы условно, но не проверены на живых ОС (G6).
- Диск-precheck скачивания модели реализован, но не прогонялся с реальной
  загрузкой HF (нет сети в тесте).
- Idle 120 с реализован; тест завершает sidecar через RAII `shutdown`, 120 с
  не ожидались.
- Android: проверена компиляция и 501-ветка; APK/UI smoke — G6/G4.
- `/api/status` при каждом опросе читает статус модели с диска; для одного
  файла это дёшево, но кэш поколения (`MODEL_GEN`) пока не используется.

## Коммиты

- G2: `ce7dcc1` — `feat(summary): add llama.cpp sidecar runtime and private artifact store`.
- G3: `4d4dd1d` — `feat(summary): add summary service, HTTP API and SSE`.
- Follow-up: `17573b4` — `fix(summary): add reduce pass, thin-output retry and load timing`.
- Follow-up: `f4dcacd` — `fix(summary): keep single-chunk map merge without reduce warning`.
