# G6 packaging, runtime distribution, CI — отчёт

Статус: **G6 реализован; Linux-бандл собран локально, runtime и headless-деградация
проверены. macOS/Windows/Android — CI-only (кроме проверки содержимого APK, если
локальная сборка доступна).** Продуктовая логика не менялась: правка локатора
добавляет только путь к ресурсам Linux-бандла.

## Что вошло

- `src-tauri/crates/mr-app/summary-runtime-manifest.json` — pin `b12182`
  (`fd27b2034`), MIT, per-target URL/SHA-256, triple, prune-список, сжатый и
  установленный размер, файлы лицензии, `layout=resources_dir`,
  `launcher=llama-server`, `nightly=true`.
- `.github/scripts/fetch_summary_runtime.sh` — скачивание/`--archive`, проверка
  SHA-256, распаковка tar.gz/zip, prune по манифесту, разворачивание symlink-имён
  в SONAME/install name (Tauri `fs::copy` иначе копирует цель symlink повторно),
  проверка `llama-server --version`, обязательных флагов `--help`, `ldd`/`otool`,
  лимит 60 МБ. Каталог по умолчанию — `src-tauri/binaries/runtime/<triple>/`
  (в git не попадает).
- `src-tauri/crates/mr-app/tauri.summary.conf.json` — release-only
  `bundle.resources: {"../../binaries/runtime": "runtime"}`. Базовый
  `tauri.conf.json` не получил `externalBin` и собирается без runtime.
- `runtime_locator.rs` — добавлены Linux-пути `usr/lib/Podtekst/runtime/<triple>`
  (AppImage/deb/rpm) при сохранении env/adjacent/`Resources`/dev-путей.
  `--web` в установленном бандле находит runtime без env.
- `ci.yml` — гигиена git отклоняет `.gguf`, runtime-архивы, `llama-server`,
  `.so/.dylib/.dll` и каталоги runtime; `frontend` выполняется до Rust-тестов
  (маркеры Svelte); явный `summary_api_contract`; headless `--web` smoke без
  runtime (`missing`); Android check сохранён; модель не скачивается.
- `release.yml` — per-target fetch/prune, merge `tauri.summary.conf.json`,
  проверка runtime в AppImage/.app/NSIS, `--expect-summary-runtime bundled`,
  APK без `llama-server`/`libllama`/`libggml`/`.gguf`/`assets/runtime`.
- `smoke_http.py` — presets `200` без модели, summary `404 no_transcript`,
  поля `/api/status`, генерация `503` без модели/runtime, `source_rev` по
  контракту digest, флаг `--expect-summary-runtime bundled|missing`.
- `LICENSES/llama.cpp-LICENSE.txt`, `THIRD_PARTY_NOTICES.md`, feature README
  (runbook упаковки), этот отчёт.
- `tests/summary_packaging_guard.rs` — контракт: базовый конфиг без runtime,
  release-конфиг с runtime, полнота манифеста и бюджеты.

## Проверки и результаты

| Команда / проверка | Результат |
|---|---|
| `fetch_summary_runtime.sh --target linux-x86_64 --archive …` | pass: 23 файла, `44 254 356` байт |
| prune macOS (cross, без запуска) | 13 файлов, `31 249 604` байт (оценка) |
| prune Windows (cross, без запуска) | 25 файлов, `46 889 299` байт (оценка) |
| `llama-server --version` (pruned Linux) | `0.9.0-dev (build 12182, commit fd27b2034)` |
| `llama-server --help` required flags | pass (все 11 флагов) |
| `ldd` всех `.so` + `llama-server` | pass, `not found` нет |
| Tauri AppImage build с `tauri.summary.conf.json` | pass: `113.16 MiB` (118 659 576 байт); ресурсы в `usr/lib/Podtekst/runtime/x86_64-unknown-linux-gnu` |
| runtime в AppDir | `44 254 356` байт, symlink-файлов `0` |
| `smoke_http.py --expect-summary-runtime bundled` (headless, runtime рядом) | pass |
| `smoke_http.py --expect-summary-runtime missing` (runtime убран) | pass: startup OK, presets `200`, generate `503` |
| Базовый `tauri.conf.json` без runtime | pass: `cargo tauri build --no-bundle --features tauri-ui` |
| Android APK (локально, debug-подпись) | `35 036 193` байт; runtime/весов нет |
| Рост native-кода Android vs релиз v0.6.3 | `+133 000` байт (`31 516 936` vs `31 383 936`), `<1 МБ` |
| `cargo fmt --all -- --check` | pass |
| `cargo test --workspace --locked` | pass: 353 passed, 2 ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo clippy -p mr-app --features tauri-ui --all-targets --locked -- -D warnings` | pass |
| `yarn check` + `yarn build` (frontend) | pass: 0 ошибок, 0 предупреждений |

SHA-256 архивов (перепроверены локально):

| Target | Архив | SHA-256 |
|---|---|---|
| Linux x86_64 | `llama-b12182-bin-ubuntu-x64.tar.gz` | `c1dbfdb9cbef7ee50b6ab08142d6424184d68b242906837770439ed118c57d99` |
| macOS arm64 | `llama-b12182-bin-macos-arm64.tar.gz` | `71cfee075e0c54852e225dafdc8dc79c32b26e8d45f052369b7d4f78d2ccf1a6` |
| Windows x86_64 | `llama-b12182-bin-win-cpu-x64.zip` | `e068c483e2ea97bf09121ec0bbe085bd43470bb67d9062c7c66162164aea5834` |

## Чего нет локально (CI-only)

- macOS `.app`/dmg: нужен Apple Silicon; `otool -L`, codesign/quarantine,
  запуск pruned runtime и smoke — только в `release.yml` на `macos-26`.
- Windows NSIS: нужен Windows; `llama-server.exe --version`, DLL-набор,
  Defender/AV и smoke — только в `release.yml` на `windows-latest`.
- Android APK: локально собрана universal-сборка (debug-подпись) и проверено
  отсутствие runtime/весов; запуск на устройстве и CI-артефакт — только CI.
  Рост native-кода измерен против опубликованного APK v0.6.3; G6 не добавляет
  Android-кода или ресурсов.
- AVX2/baseline CPU, Defender и чистые VM не проверялись локально.

## Ограничения

- Manifest `installed_bytes` для macOS/Windows — сумма файлов из архива, а не
  замер после упаковки; CI печатает фактический размер.
- `--allow-cross` в fetch-скрипте предназначен только для осмотра размера:
  каталог чужой ОС не проверяется запуском и может содержать symlink-имена.
- Runtime `b12182` — nightly; смена пина требует повторного `--help`/smoke.

## Коммиты

- G6: `feat(summary): package llama.cpp runtime and smoke release bundles`.
