# Changelog

Все заметные изменения проекта. Формат — [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/),
версии — [Semantic Versioning](https://semver.org/lang/ru/).

## [0.1.0] — 2026-10-25

Первый публичный релиз.

### Добавлено

* Запись встреч с раздельными каналами микрофона и системного звука на Linux.
* Локальная расшифровка: GigaAM-v3 e2e RNNT int8 (русский) и Whisper large-v3-turbo int8.
* Диаризация и голосовые отпечатки ERes2NetV2 192-dim.
* Голосовые пакеты (`.voicepack.zip` / `.voicepack.json`) и база знаний из Markdown.
* Tauri-окно, трей, глобальный хоткей, автозапуск.
* Импорт и расшифровка готовых аудио/видеофайлов.
* Android-сборка (экспериментальная), см. `ANDROID.md`.

### Известные ограничения

* Живой захват системного звука на macOS и Windows пока в разработке.
* Android: только микрофон, расшифровка экспериментальная.
* Релизные бинарники не подписаны; macOS Gatekeeper и Windows SmartScreen
  могут показать предупреждение.

[0.1.0]: https://github.com/guest363/meeting-record/releases/tag/v0.1.0
