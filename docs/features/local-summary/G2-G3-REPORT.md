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
| `cargo test --workspace --locked ...` | pass: 321 passed, 2 ignored (diar_local, property 100k) |
| `cargo test -p mr-app --test summary_api_contract --locked` | pass: 1 suite (19 сценариев) |
| `cargo test -p mr-app --test summary_real --locked` без env | pass (skip) |
| `SUMMARY_REQUIRE_REAL=1 ... cargo test -p mr-app --test summary_real` | pass на модели |
| Android: `cargo check -p mr-app --target aarch64-linux-android --features tauri-ui --locked` | pass (NDK 27.1.12297006 + llvm bin в PATH) |
| `git diff --check` (весь репозиторий) | fail: trailing whitespace в `ui/web/static/js/app.js` от параллельного frontend-агента; `src-tauri` чист |

Frontend-маркеры (`summary-block`, `summary-preset-picker`, `summary-result`)
в `restored_endpoints_roundtrip` **не добавлялись** по coordination note:
roundtrip расширен только backend-контрактом (presets/settings/404/413/status/
`has_summary`). Интеграционная проверка маркеров — после коммита frontend.

## Живой smoke (Linux, реальная модель)

Условия: `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` (2.33 ГиБ), runtime
`llama-b12182`, синтетический транскрипт `meeting-ru.json` (13 реплик),
20 logical cores, 31 ГиБ RAM; во время прогонов load average 16–18 (другие
агенты). Тест — debug-профиль оркестратора, sidecar — релизный бинарь.

| Метрика | Измерено |
|---|---|
| Успешная генерация, total `duration_ms` | финальный прогон на HEAD: `105 735 мс` wall / `105 657 мс` artifact (items=8) |
| Peak `VmHWM` sidecar | `5 291 MiB` (5.17 ГиБ) <= 6.5 ГиБ |
| Cancel (POST -> `summarizing=false`) | `1 418 мс` (ранее `1 797–1 798 мс`) <= 2 с |
| Артефакты | `.summary.json` + `.summary.md`, transcript bytes не изменены |
| Generator | `llama.cpp/b12182`, commit `fd27b2034`, `model_sha256=3605803b…`, `prompt_version=meeting-v2` |
| Разброс прогонов | `105.7 с`, `140.2 с`, `208.2 с`, `216.7 с`, `241.4 с` при load average 16–20; конкуренция CPU |

Финальный прогон `105.7 с` уложился в бюджет §15 (`<=120 с` на `>=8` logical),
но разброс между прогонами почти 2.3× из-за нагрузки хоста (load average 16–20).
Цифра 65.05 с из спецификации не воспроизводилась; 4-core tier и 9.5k-кейс —
G5. Cancel и peak — в бюджете во всех прогонах.

## Ограничения / что осталось

- Map-reduce на длинном входе использует детерминированный merge частичных
  ответов (dedupe/grounding), без отдельного LLM-reduce; quality/coverage —
  G5. `provenance.mode="map_reduce"`, `reduce_levels=0`.
- Retry реализован только для `invalid_output` (seed 43); retry для
  `output_thin`/`decision_missed` — G5.
- `timings_ms.load` = 0 (загрузка модели не отделена от generate); total/prepare/
  generate/verify измерены.
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
- G3: этот коммит — `feat(summary): add summary service, HTTP API and SSE`.
