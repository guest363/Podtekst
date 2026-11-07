# mr-summarize — AGENTS.md

Доменный крейт локальных итогов. Чистые преобразования: без файлового ввода-вывода,
сети, `ort`, `ureq`, Tauri и системных зависимостей. HTTP, runtime и sidecar живут в
`mr-app` (G3+); этот крейт компилируется на Android и в headless-сборке.

## Инварианты

- **Один канон артефакта:** `podtekst.summary/v1`, `schema_minor=0`. Неизвестные поля
  читатель игнорирует; чужая major-версия — `SchemaUnsupported`.
- **Digest `source_rev`:** `schema=1\n` + для каждой реплики `f64.to_bits()` (LE),
  `u32_le` длины UTF-8 `speaker`, байты, `u32_le` длины UTF-8 `text`, байты. Поля
  `words`, `ctc_used`, `silhouette`, порядок ключей и `meta.speakers_info` не влияют.
- **Классификация:** единственный источник — `classify-spec.v1.json`, встроенный через
  `include_str!`; `SPEC_SHA256` и SHA стоп-слов проверяются тестом. Любое изменение
  групп, весов, структуры, cue, порогов или метрик требует bump `spec_version` и
  `HEURISTIC_VERSION`. `classifier` — предположение, не accuracy-claim (D15).
- **Секции:** роли `plain|decision|task|status` задаёт каталог. «Кто/Срок/не назначен/
  срок не назван» появляются только у `role=task`; `Суть`, `Факты`, `Примеры` не получают
  task-разметку.
- **Парсер:** принимает `-`/`*`/`+`/`•`, нумерованные списки, bold-label заголовки,
  h2–h4, `Задача:`/`Срок:`/`Кто:` блоки, хвостовые пробелы; неизвестный заголовок
  прикрепляет пункты к предыдущей секции; валидный пункт не теряется из-за формата.
- **Заземление:** пункт без >=1 ref не показывается; evidence `intersection / |item tokens|`,
  support при `intersection>=2 && ev>=0.25`, refs top-3. Жёсткие факты (числа, даты,
  имена, Latin-термины) обязаны быть в поддерживающей реплике, иначе drop.
- **Дедлайны:** детерминированные regex-паттерны, exact source substring; модель не
  придумывает срок. `deadline_source` — `model|extracted`.
- **Промпт:** системные инварианты — дословно `SYSTEM_PROMPT`; данные — JSON внутри
  nonce-маркеров `<<<DATA_<32hex>_BEGIN>>>`; `prompt_version=<id>-v2`, вариант
  `hybrid-v2`.
- **Размеры:** JSON <=1 МиБ, MD <=2 МиБ; optional-секции удаляются в фиксированном
  порядке, обязательные — никогда; иначе `OutputCapped`.

## Модули

| Файл | Ответственность |
|---|---|
| `src/lib.rs` | Публичный API и реэкспорты |
| `src/preset.rs` | 7 шаблонов, секции/роли, caution, `Detail`, версии |
| `src/classify.rs` + `classify/{spec,signals}.rs` | Эвристика, скоринг, сигналы |
| `src/text.rs` | Транскрипт, нормализация, токены, язык, стиль, окно |
| `src/chunk.rs` | Map-reduce chunking, overlap, coverage, `MAX_CHUNKS` |
| `src/prompt.rs` | Prompt contract, nonce, JSON DATA |
| `src/parse.rs` + `parse/aliases.rs` | Толерантный парсер и alias-таблица |
| `src/grounding.rs` + `grounding/rules.rs` | Evidence, hard facts, soft claims, tasks |
| `src/deadline.rs` | Детерминированное извлечение сроков |
| `src/artifact.rs` + `artifact/{dto,markdown}.rs` | Схема, digest, DTO, `.md`, caps |

## Проверка

```bash
cargo test -p mr-summarize --locked --manifest-path src-tauri/Cargo.toml
cargo clippy -p mr-summarize --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check
MR_PERF=1 cargo test -p mr-summarize --release --locked --manifest-path src-tauri/Cargo.toml \
  classify_100k_p95 -- --ignored --nocapture
```

Фикстуры синтетические; реальные имена, файлы и тексты запрещены (P12). Golden-файлы
`tests/golden/*` не редактируются вручную: изменение поведения требует bump версии и
нового замера.
