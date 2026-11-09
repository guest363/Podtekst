# Уведомления о сторонних компонентах (Third-Party Notices)

Код проекта лицензируется по MIT (`LICENSE`). Ниже - лицензии и источники
сторонних материалов, которые использует или распространяет проект. Полный
инвентарь Rust-зависимостей - в `THIRD_PARTY_LICENSES.md`, стандартные тексты
лицензий - в `LICENSES/`.

## ИИ-модели (скачиваются приложением, в репозиторий не входят)

Источники и SHA-256 - в `src-tauri/crates/mr-models/models-manifest.json`.

| Модель | Лицензия | Источник |
|--------|----------|----------|
| Silero VAD v6.2.3 | MIT | https://github.com/snakers4/silero-vad |
| GigaAM-v3 e2e RNNT (int8, ONNX) | MIT | https://github.com/salute-developers/GigaAM , конвертация https://huggingface.co/istupakov/gigaam-v3-onnx |
| Whisper large-v3-turbo (int8, ONNX) | MIT (базовая модель OpenAI) | https://github.com/openai/whisper , конвертация https://huggingface.co/onnx-community/whisper-large-v3-turbo |
| ERes2NetV2 192-dim (speech_eres2netv2_sv_zh-cn_16k-common) | Apache-2.0 | https://github.com/modelscope/3D-Speaker , конвертация https://huggingface.co/protocolvoice/asr-models |
| Qwen3-4B-Instruct-2507 Q4_K_M (локальные итоги) | Apache-2.0 (базовая модель Qwen) | https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507 , веса https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF |

Модели не входят в репозиторий и не распространяются вместе с бинарником:
приложение скачивает их с указанных источников по HTTPS и проверяет SHA-256.
Названия моделей и компаний принадлежат их правообладателям; проект не
аффилирован с Sber, OpenAI, Alibaba, Silero Team и Qwen/Alibaba Cloud.

## Runtime локальных итогов (в git не входит)

`llama.cpp llama-server` pin `b12182` (commit `fd27b2034`) распространяется по
лицензии MIT: https://github.com/ggml-org/llama.cpp/blob/master/LICENSE ,
сборка https://github.com/ggml-org/llama.cpp/releases/tag/b12182 . Каталог
ресурсов (`.so`/`.dylib`/`.dll`) не входит в git; SHA архивов зафиксирован в
`src-tauri/crates/mr-app/summary-runtime-manifest.json`. Релизные сборки
включают только pruned runtime и текст лицензии
(`LICENSES/llama.cpp-LICENSE.txt`); веса модели остаются отдельной загрузкой.
Android не содержит ни runtime, ни весов.

## Тестовые фикстуры

WAV-файлы `src-tauri/crates/mr-ml/tests/fixtures/speaker*.wav` - официальные
примеры датасета/тулкита 3D-Speaker (Alibaba, Apache-2.0); происхождение и
производные - см. `src-tauri/crates/mr-ml/tests/fixtures/README.md`.

## Встроенные компоненты

* **Rust-крейты** (`Cargo.lock`) - MIT, Apache-2.0, MPL-2.0, Unicode-3.0,
  CDLA-Permissive-2.0, BSD, ISC, Zlib и 0BSD. Полный список - `THIRD_PARTY_LICENSES.md`.
  MPL-2.0-крейты (`symphonia`) используются без изменений; исходники доступны
  на crates.io.
* **screencapturekit** (macOS, системный звук через ScreenCaptureKit) - MIT OR Apache-2.0,
  https://github.com/doom-fish/screencapturekit-rs.
* **ONNX Runtime** (бинарники `ort` для CPU-инференса) - MIT,
  https://github.com/microsoft/onnxruntime/blob/main/LICENSE.
* **Web UI** - собственный код проекта; Svelte runtime (MIT) входит в
  собранный `ui/web/static/js/app.js`, исходники Svelte - https://github.com/sveltejs/svelte.
* **Android-сборка** - AndroidX, Material Components и Kotlin (Apache-2.0),
  https://developer.android.com/jetpack/androidx .
* **Gradle wrapper** - Apache-2.0, https://gradle.org/license/ .

## Иконки

`icon.png`, `icon.svg`, `icon.ico`, `icon.icns` - собственные материалы проекта.
