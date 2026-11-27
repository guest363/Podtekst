# Подтекст (Podtekst) — AGENTS.md

Нативное кроссплатформенное приложение для записи встреч и локальной ИИ-расшифровки
(Linux PipeWire/PulseAudio, macOS CoreAudio + ScreenCaptureKit, Windows WASAPI loopback).
Микрофон и система пишутся раздельно, в том числе при выводе на колонки; акустическое эхо
подавляется VAD-гейтом и дедупликацией, цифровые дубли сводятся к одному исходному каналу.
Стек: Rust + Tauri v2 + ONNX Runtime. Модели: GigaAM-v4 e2e RNNT int8, Whisper turbo,
Silero VAD v6, PyAnnote 3.0, ERes2NetV2 192-dim — всё локально, без облаков.

Это карта проекта, а не мануал: правила — `docs/ai/code-style.md`, процесс —
`docs/ai/workflow.md`. Документ, на который ссылается карта, — единственный источник правды
по своей теме; не пересказывайте его в других файлах.

## Запуск и гейт

```bash
./run.sh                  # окно Tauri + HTTP API 127.0.0.1:8765
./run.sh --web            # только HTTP API и браузер
./run.sh --transcribe meeting.mp3 [--engine auto|gigaam|whisper]

./scripts/verify.sh --fast  # P11 и доки: секунды, без сборки
./scripts/verify.sh --rust  # fmt, clippy, тесты
./scripts/verify.sh --ui    # lint, check, build, штамп статики
./scripts/verify.sh --tauri # clippy с tauri-ui
./scripts/verify.sh --all   # полный гейт перед PR
```

Охват режимов и критерий готовности — `docs/ai/workflow.md`. Первый запуск окна требует
системный WebView; пакеты для Linux — в `README.md`.

## Инварианты

- **[P12] Zero-Leak.** Аудио, отпечатки и статистика не покидают машину. В PR, коммитах,
  issue, коде и тестах нет имён людей, названий файлов, тем и фрагментов речи — только
  технические параметры: канал, хронометраж, число спикеров.
- **[P10] Root Cause.** Причина чинится на своём слое, а не маскируется в вызывающем коде.
- **Ноль предупреждений.** `clippy -D warnings`, `yarn lint`, `yarn check` — блокеры.
- **[P2] Минимальный diff.** Простейшее решение, закрывающее задачу; новые зависимости —
  только с критическим поводом.
- **[P9] Контракты.** CLI-флаги, REST, `voiceprints.json` v3 и `.transcript.json` не ломаются
  без мажорной версии.

Полный свод — `docs/ai/code-style.md` (P1–P12). Перед правкой прочтите разделы по
затронутому коду.

## Карта контекста

| Задача | Куда смотреть |
|---|---|
| Доменные термины (Turn, Voiceprint, Person, VAD…) | `CONTEXT.md` |
| Правила P1–P12, стиль, комментарии | `docs/ai/code-style.md` |
| Процесс, Track 1/2, верификация, критерий готовности | `docs/ai/workflow.md` |
| Архитектура, стадии конвейера, UI ↔ Rust ↔ файлы | `docs/architecture.md` |
| Тестирование на датасетах, метрики, приёмка | `docs/testing/SPEC.md`, `docs/testing/PERF.md`, `tools/diar_eval/baseline.json` |
| ML, ONNX, Silero, GigaAM, Whisper, эмбеддинги | `src-tauri/crates/mr-ml/AGENTS.md` |
| Захват аудио, устройства, WAV, symphonia | `src-tauri/crates/mr-audio/AGENTS.md` |
| HTTP API, Tauri IPC, конвейер транскрибации | `src-tauri/crates/mr-app/AGENTS.md` |
| Каталог и загрузка моделей, SHA-256 | `src-tauri/crates/mr-models/AGENTS.md` |
| Локальные итоги: шаблоны, парсер, grounding | `src-tauri/crates/mr-summarize/AGENTS.md` |
| Голосовые пакеты, экспорт и импорт | `src-tauri/crates/mr-packs/AGENTS.md` |
| Метрики речи, F0, качество записи | `src-tauri/crates/mr-metrics/AGENTS.md` |
| Метрики словаря и hotwords (Rust) | `src-tauri/crates/mr-eval/README.md` |
| Web UI (Svelte 5 + Vite + TS) | `ui/web/frontend/AGENTS.md` |
| Android (AAudio, Foreground Service) | `ANDROID.md` |
| Релиз и публикация версий | `docs/RELEASING.md` |
| CI: тесты, бинарники, Tauri-бандлы | `.github/workflows/release.yml` |

## Локальные AGENTS.md

Перед изменением крейта или фронта прочтите его `AGENTS.md`: инварианты, карта файлов,
локальная верификация. Файл создаётся только при существенных правилах, которых нет в общей
документации (`docs/ai/agents-template.md`); описывать очевидную структуру не нужно.

README пакета — конвенции, API и примеры (канон для людей и агентов). `AGENTS.md` — только
правила изменения и особенности, без дублей README. Уже прочитанные и неизменившиеся
инструкции повторно не загружайте.

## Роли

Основной процесс един для всех агентов — `docs/ai/workflow.md`. Antigravity входит через
`.agent/rules/audio-record.md`, Claude Code — через `CLAUDE.md` (импорт `AGENTS.md`).

## Сообщения коммитов и PR

`<type>(<scope>): <subject>`; типы `feat|fix|refactor|perf|test|docs|chore`.
В заголовках и описаниях — только обезличенные технические параметры [P12].
