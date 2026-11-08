# Локальные итоги встреч (summary v1)

Назначение: после локальной расшифровки собрать короткий протокол встречи — что решили,
кто что делает, что осталось — без сети, облака и телеметрии. Полная спецификация:
[`SPEC.md`](SPEC.md). Baseline: [`G0-BASELINE.md`](G0-BASELINE.md); проверка домена:
[`G1-REPORT.md`](G1-REPORT.md); runtime/HTTP/SSE: [`G2-G3-REPORT.md`](G2-G3-REPORT.md);
E2E-фикс: [`E2E-QUALITY-FIX.md`](E2E-QUALITY-FIX.md); матрица промптов:
[`PROMPT_MATRIX.md`](PROMPT_MATRIX.md); упаковка и CI:
[`G6-REPORT.md`](G6-REPORT.md).

## Область v1

- Read-only: `<stem>.summary.json` (канон) и `<stem>.summary.md` (копия) рядом с
  `.transcript.json`. Расшифровка не изменяется.
- Одна модель: `Qwen3-4B-Instruct-2507 Q4_K_M` (2,33 ГиБ, Apache-2.0). Других моделей,
  tier-ов и fallback-ов нет.
- Runtime: `llama.cpp llama-server`, pin `b12182` (`fd27b2034`), отдельный каталог ресурсов
  (не static executable). В релизный desktop-бандл runtime входит pruned-каталогом;
  модель скачивается только по явному действию с проверкой SHA-256. Веса не входят
  в репозиторий и APK.
- Классификатор — детерминированная эвристика (`classify-spec.v1.json`), это
  **предположение** с всегда доступным ручным выбором шаблона (D15). Точность не заявляется.
  `manual_only` — операторский fallback, не дефолт.
- Чувствительные шаблоны `legal` и `psychology` требуют подтверждения и никогда не
  генерируются автоматически.

## Требования к машине

- Минимум: 12 ГиБ ОЗУ всего и не меньше 6,5 ГиБ свободно (MemAvailable).
- Рекомендуется: 16 ГиБ ОЗУ.
- 8 ГиБ — actionable-отказ `low_memory`, без частичного артефакта.
- 4 logical cores — минимум с предупреждением; 8 recommended.

## Деградация

- **Android:** все `/api/summary*` возвращают `501 summary_unsupported`, UI скрыт;
  runtime и модель в APK не входят.
- **Headless `--web` без runtime:** `GET /api/summary` отвечает `200` с
  `capabilities.runtime="missing"`, генерация — `503 runtime_missing`; запуск приложения
  не падает.
- **Модель не скачана:** доступна только генерация после явного скачивания, UI не создаёт
  тупик.
- **Низкая память / busy / stale / timeout / cancel:** предсказуемые ошибки и recovery,
  старый артефакт не повреждается.

## Упаковка runtime (G6)

Базовый `src-tauri/crates/mr-app/tauri.conf.json` собирается без runtime и без
весов. Только релизная сборка добавляет `tauri.summary.conf.json`:

```bash
# 1. Скачать, проверить SHA-256, обрезать и разложить runtime своей ОС
.github/scripts/fetch_summary_runtime.sh --target linux-x86_64

# 2. Собрать бандл с ресурсами (macOS/Windows — свои target_key)
cd src-tauri/crates/mr-app
cargo tauri build --features tauri-ui --config tauri.summary.conf.json --bundles appimage
```

Каталог ресурсов: `src-tauri/binaries/runtime/<triple>/` (в git не попадает).
Внутри — только `llama-server`, нужные `.so`/`.dylib`/`.dll` и тексты лицензий;
символы версий разворачиваются в имена SONAME, чтобы упаковщик Tauri не копировал
symlink-цели повторно. Бюджет — 60 МБ установленного runtime.

Пути поиска (`summary/runtime_locator.rs`): `PODTEKST_SUMMARY_RUNTIME` →
рядом с исполняемым файлом `runtime/<triple>` → macOS `Contents/Resources` →
Linux `usr/lib/Podtekst/runtime/<triple>` (AppImage/deb/rpm) →
dev `src-tauri/binaries/runtime/<triple>`. Запуск `--web` ищет runtime рядом с
бинарником или по env; без него `GET /api/summary` отвечает `200` с
`capabilities.runtime="missing"`, генерация — `503 runtime_missing`.

Проверка релизного бандла:

```bash
python3 .github/scripts/smoke_http.py --exe <bundle>/podtekst \
  --expected-version 0.6.3 --expected-os linux --expected-arch x86_64 \
  --static-dir ui/web/static --expect-summary-runtime bundled
```

Android APK не содержит runtime/`.gguf`; все `/api/summary*` отвечают `501`.
CI проверяет гигиену git (модели, `.gguf`, runtime-архивы и `.so`/`.dll` не
коммитятся) и содержимое APK.

## Проверка

```bash
cargo test -p mr-summarize --locked --manifest-path src-tauri/Cargo.toml
cargo clippy -p mr-summarize --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check
cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml
cargo test -p mr-app --test summary_api_contract --locked --manifest-path src-tauri/Cargo.toml
MR_SUMMARY_RUNTIME=<dir>/llama-server MR_SUMMARY_MODEL=<dir>/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
  MR_TEST_TRANSCRIPT=<file>.transcript.json SUMMARY_REQUIRE_REAL=1 \
  cargo test -p mr-app --test summary_real --locked --manifest-path src-tauri/Cargo.toml -- --nocapture
```

Домен `mr-summarize` не делает файлового ввода-вывода и не обращается к сети. Сетевые
запросы возможны только при явном скачивании модели/runtime и проверке релиза.
