# G0 baseline — локальные итоги (2026-11-07)

Worktree: `/home/moiseevov/.local/share/opencode/worktree/d473c4/local-summary`
Branch: `feat/local-summary`, base `9d234d0`.

## Toolchain

- `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- `cargo 1.99.1 (5f94df478 2026-08-27)`
- `rust-toolchain.toml`: channel `1.99`, MSRV по `Cargo.lock` — 1.92.

## Baseline до добавления крейта

| Команда | Результат |
|---|---|
| `cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml` | pass, 218 tests, exit 0 |
| `cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings` | pass, exit 0 |

Логи: `/tmp/opencode/podtekst-summary/verification-g0-baseline-test.log`,
`/tmp/opencode/podtekst-summary/verification-g0-baseline-clippy.log`.

## Зафиксированные решения

- **Модель:** `Qwen/Qwen3-4B-Instruct-2507`, GGUF `Qwen3-4B-Instruct-2507-Q4_K_M.gguf`
  из `unsloth/Qwen3-4B-Instruct-2507-GGUF`; размер `2 497 281 120` байт;
  SHA-256 `3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597`;
  Apache-2.0; контекст v1 `8192`.
- **RAM:** measured peak `5.424 ГиБ` (8k), `estimate_peak_mb=5530`,
  `required_free_mb=6656`, минимум `12288 MiB` total, рекомендуется 16 ГиБ.
- **Runtime:** `llama.cpp llama-server`, pin `b12182` (`fd27b2034`), MIT; каталог
  ресурсов, не static executable; pruned Linux dir 43 МБ, бюджет `<=60 МБ installed`.
  SHA архивов: Linux x64 `c1dbfdb9…`, macOS arm64 `71cfee07…`, Windows cpu x64
  `e068c483…`.
- **Классификатор:** эвристика `classify-spec.v1`; D15 — v1 показывает
  `classifier="heuristic"` как предположение с ручным picker, `manual_only` остаётся
  операторским fallback; accuracy не заявляется.
- **Сеть:** генерация только локальная; скачивание модели/runtime — по явному действию
  с SHA-256 и resume. Веса и runtime в репозиторий не скачивались и не коммитились.

## G0 exit

Pin/SHA/бюджеты зафиксированы, baseline зелёный, блокеров G0 нет. Открытые вопросы
пина runtime/упаковки/held-out относятся к G2/G5/G6 (см. §23 спецификации).
