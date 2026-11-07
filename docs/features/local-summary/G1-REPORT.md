# G1 domain crate — проверка

Статус: **G1 green** для домена `mr-summarize`. Product wiring (HTTP/sidecar/UI)
намеренно отсутствует — это G3+.

## Команды и результаты

| Команда | Результат |
|---|---|
| `cargo test -p mr-summarize --locked --manifest-path src-tauri/Cargo.toml` | pass: 75 tests, 1 ignored |
| `cargo clippy -p mr-summarize --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings` | pass |
| `cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check` | pass |
| `MR_PERF=1 cargo test -p mr-summarize --release --locked ... classify_100k_p95 -- --ignored --nocapture` | pass: p50 `2.16 ms`, p95 `2.21–2.84 ms` на 100k chars, 100 прогонов |

## Golden-классификатор

Реализация `classify-spec.v1.json` воспроизводит все 8 замороженных строк
спецификации без подгонки весов:

| Case | meeting | scrum | legal | lecture | psychology | personal |
|---|---:|---:|---:|---:|---:|---:|
| meeting-ru | 9 | 3 | 5 | 0 | 2 | 4 |
| scrum-ru | 8 | 11 | 2 | 0 | 2 | 2 |
| legal-ru | 4 | 1 | 10 | 0 | 1 | 2 |
| lecture-ru | 4 | 3 | 1 | 9 | 2 | 1 |
| psychology-ru | 1 | 2 | 1 | 2 | 9 | 3 |
| personal-ru | 4 | 3 | 2 | 2 | 2 | 6 |
| english | 9 | 5 | 1 | 1 | 2 | 3 |
| injection-ru | 8 | 3 | 1 | 1 | 2 | 2 |

Дополнительная реплика без срока в `meeting-ru` не изменила итог (база упирается в
cap 6); расхождения с golden-таблицей нет. Cue-матч зафиксирован как нормализованная
подстрока: это единственная интерпретация, воспроизводящая замороженные `scrum-ru`
(legal=2) и `personal-ru` (personal=6) без изменения весов. Подробности — в
`/tmp/opencode/podtekst-summary/verification/blockers.md`.

## Что заморожено

- `classify-spec.v1.json` + SHA в `classify.rs`, `HEURISTIC_VERSION=1`.
- `stopwords-v1.txt` (212 строк, SHA в spec).
- Golden scores/signals `tests/golden/classify-golden.v1.json`.
- Digest vectors: `df83afb2…` и `103b92e4…`.
- `podtekst.summary/v1` + DTO projection `tests/golden/summary-{artifact,record}.json`.
- Секции/роли 7 шаблонов, task-разметка только `role=task`, tolerant parser,
  deadline recall, soft-claim drop, caps.

## Ограничения

- Prompt matrix §7 (3 формулировки × 4 фикстуры) — G5: в G1 нет модели, метрики не
  выдумываются; реализован победитель `hybrid-v2`.
- NFC предполагается выполненной: `unicode-normalization` не входит в разрешённый
  список зависимостей (см. blockers).
- Список EN stopwords в спецификации содержит 61 слово при заявленных 60; сохранены
  все перечисленные слова.
- `manual_only`, sidecar, HTTP/SSE, frontend — не этот gate.
