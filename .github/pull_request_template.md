## Что изменено

<!-- Кратко: что и зачем. -->

## Проверки

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy -p mr-app --features tauri-ui --all-targets --locked -- -D warnings` (если трогал Tauri)
- [ ] `cd ui/web/frontend && yarn check && yarn build` (если трогал UI)
- [ ] Пересобранная статика `ui/web/static` закоммичена
- [ ] В diff нет `people.json`, `voiceprints.json`, `config.json`, логов, записей и моделей
- [ ] В описании PR и коммитах нет личных данных (имен, названий файлов, тем бесед - только технические параметры: моно/стерео, длительность, N спикеров, см. P12 в AGENTS.md)
- [ ] Лицензия вклада - MIT (см. `CONTRIBUTING.md`)

## Связанные issue

<!-- Fixes #... -->
