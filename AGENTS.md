# Meeting Recorder (Запись встреч) — AGENTS.md

Нативное кроссплатформенное приложение для записи встреч и локальной ИИ-расшифровки
(Linux PipeWire/PulseAudio — живой захват; macOS/Windows — импорт файлов, живой захват в разработке).
Стек: **Rust + Tauri v2 + ONNX Runtime** (zero-dependency бинарник ~20–30 МБ).
Модели: **GigaAM-v3 e2e RNNT int8** (русский, SOTA) + **Whisper-turbo** (30+ языков) +
**Silero VAD v6** + **ERes2NetV2 192-dim** (3D-Speaker) — всё локально, без облаков.

---

## Команды

```bash
# Запуск (первый запуск сам соберёт релизный бинарник)
./run.sh                  # Нативное окно Tauri (WebView) + HTTP API http://127.0.0.1:8765
./run.sh --web            # Только HTTP API + автооткрытие браузера (без окна, лёгкая сборка)
./run.sh --tray           # Нативное окно скрыто, только трей (Tauri)
./run.sh --toggle         # Переключить запись (старт / стоп через IPC)
./run.sh --start          # Начать запись через IPC
./run.sh --stop           # Остановить запись через IPC
./run.sh --transcribe встреча.mp3 [--engine auto|gigaam|whisper]  # Расшифровка в CLI

# Нативное окно требует системный WebView (один раз):
# sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
# (macOS: WKWebView из системы; Windows: WebView2 из системы)

# Разработка
cargo test --workspace --manifest-path src-tauri/Cargo.toml   # Все unit + мат. тесты
cargo test -p mr-ml --features onnx --test onnx_real -- --nocapture  # Живой инференс (нужны модели + WAV, иначе skip)
cargo clippy --workspace --all-targets --manifest-path src-tauri/Cargo.toml  # Ноль варнингов — обязательно
cargo clippy -p mr-app --features tauri-ui --all-targets --manifest-path src-tauri/Cargo.toml  # Нативное окно/трей (таури_shell.rs компилируется только так)
cargo build --release -p mr-app --manifest-path src-tauri/Cargo.toml
cd ui/web/frontend && yarn check && yarn build  # Фронт: типы (svelte-check) + прод-сборка в ui/web/static
```

Переменные окружения для тестов/запуска:
`MEETING_RECORDER_DATA_DIR` (переопределение папки данных),
`MR_MODELS_DIR` + `MR_TEST_WAV` (живые ONNX-тесты).

---

## Локальное окружение

- **Rust**: точная версия закреплена в `rust-toolchain.toml` (последний стабильный);
  минимум для сборки — 1.92 (MSRV по `Cargo.lock`: ort 2.0.0-rc.15 требует 1.92,
  Tauri — 1.90): всё собирается Cargo, внешних рантаймов нет.
- **Node.js ≥ 20.19 / ≥ 22.12 + Yarn 4** (`corepack enable`; минимум диктует Vite 8):
  только для пересборки фронта при изменении исходников (`ui/web/frontend` →
  `yarn install --immutable && yarn build`). В рантайме Node нет. Штамп с хешем исходников
  коммитится внутрь `ui/web/static/.mr-stamp`: чистый клон с неизменёнными исходниками
  запускается без Node.
- **Модели** скачиваются один раз через карточку «Модели ИИ» в UI
  (`POST /api/models/update`, докачка `.part` + проверка `SHA-256` из `core/models-manifest.json`
  — точнее, `src-tauri/crates/mr-models/models-manifest.json`),
  лежат в `<app_data>/models/<id>/`.
- **Голосовые пакеты**: `<app_data>/packs/<id>/` (`people.json`, `voiceprints.json` v3, `snippets/*.wav`).
  Личные `people.json` / `voiceprints.json` / `config.json` в корне репозитория —
  локальные данные владельца, в git не коммитятся.

---

## Основные правила разработки (P1–P12)

Правила обязательны для всех агентов и разработчиков:

- **[P1] DRY & Single Source of Truth.** Форматы (`voiceprints.json` v3, `.transcript.json`,
  `models-manifest.json`) имеют одно каноническое описание; Rust и тесты — единственная реализация.
- **[P2] YAGNI & Простота (Ponytail).** Стандартная библиотека и текущие зависимости вместо новых крейтов.
  Фичи за cargo-фичами (`onnx`, `tauri-ui`); дефолтная сборка — без системных зависимостей.
- **[P3] Изоляция UI от ядра.** HTTP REST API (`mr-app`) — единственный мост между Web UI и ядром.
  Никаких ML-импортов в HTTP-слое, никакой файловой логики в математике.
- **[P4] Статические ассеты вне кода.** Web UI — только `ui/web/static/` (прод-сборка Svelte);
  исходники — `ui/web/frontend/src/` (Svelte 5 + TS + CSS Cascade Layers); CSS/JS не инлайнятся в Rust.
  Имена сборки детерминированы (`css/app.css`, `js/app.js`, без хешей) — их знает манифест
  вшитых ассетов в `mr-app/src/main.rs` (`static_body`), при добавлении файлов дописать туда же.
- **[P5] Кроссплатформенность без исключений.** Пути — `PathBuf`; захват аудио — через `mr-audio`
  (WASAPI Loopback / CoreAudio Tap / PipeWire); OS-специфика — за `#[cfg(target_os)]`.
- **[P6] Минимум внешних процессов.** Основной путь — нативные библиотеки (libpulse через dlopen, symphonia). Осознанные fallback-исключения: `ffprobe`/`ffmpeg` (длительность не-WAML медиа и контейнеры без symphonia-демуксера), `pactl`/`pw-record`/`parec` (если libpulse отсутствует), `gsettings` (GNOME-хоткей), открытие браузера/папок.
- **[P7] Корректное завершение.** IPC-сервер и HTTP останавливаются вместе с процессом; запись финализирует WAV-заголовки.
- **[P8] Модули до 300–400 строк.** Роутер HTTP (`http/mod.rs`) и пайплайн (`transcribe/mod.rs`) — координаторы, чьи ветки разнесены в соседние файлы; сами решения держим компактными.
- **[P9] Обратная совместимость.** CLI-флаги (`--toggle/--start/--stop/--tray/--web/--port`),
  REST-контракты и форматы файлов не ломаются без мажорной версии.
- **[P10] Root Cause.** Чинить причину на базовом слое; верификация — живым прогоном на реальной записи.
- **[P11] Строгая типизация и документация на русском.** Публичные API — с doc-комментариями.
- **[P12] Безопасность пользовательских данных.** Аудио, голосовые отпечатки и статистика — только локально.
  Единственный сетевой исходящий трафик: скачивание открытых весов моделей (автообновлений нет).

---

## Карта контекста

| Задача | Куда смотреть |
|--------|---------------|
| HTTP REST, CLI, IPC, автозапуск | `mr-app/src/main.rs`, `config.rs`, `ipc.rs`, `recording.rs`, `desktop.rs` |
| HTTP-хендлеры (медиа/люди/расшифровка) | `mr-app/src/http/{mod,media,files,people,transcribe_api}.rs` |
| Сквозная транскрибация (VAD→ASR→диаризация→md/txt/json) | `mr-app/src/transcribe/{mod,events,diarize,report}.rs` |
| ONNX-инференс: Silero, GigaAM RNNT, Whisper, эмбеддинги | `src-tauri/crates/mr-ml/src/onnx/` |
| Математика диаризации (прототипы, Венгрия, кластеризация) | `src-tauri/crates/mr-ml/src/{lib.rs,cluster.rs}` |
| Голосовые пакеты, Markdown-коннектор базы знаний | `mr-packs/src/{lib,manifest,transfer,directory}.rs` |
| Реестр и обновление моделей (манифест, зеркала, SHA-256) | `src-tauri/crates/mr-models/` + `models-manifest.json` рядом |
| Аудио: устройства, сплит, WAV, symphonia-декодер | `src-tauri/crates/mr-audio/src/` |
| Web UI (Svelte 5 + Vite + TS, собирает `run.sh`) | `ui/web/frontend/src/` (исходники), `ui/web/static/` (только dist, руками не править) |
| Конфиг Tauri (окно, трей, хоткеи) | `src-tauri/crates/mr-app/tauri.conf.json`, `capabilities/` рядом |
| CI: тесты + бинарники + Tauri-бандлы | `.github/workflows/release.yml` |

---

## Сообщения коммитов

Формат — Conventional Commits: `<type>(<scope>): <subject>`. Типы: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `chore`.
