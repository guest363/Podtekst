# Участие в разработке

Спасибо за интерес! PR приветствуются: исправления, документация, поддержка
платформ. Большие фичи — сначала issue для обсуждения.

## Проверки перед PR (все гоняет CI)

```bash
# Rust (из корня репозитория; тулчейн закреплён rust-toolchain.toml)
cargo fmt --all --manifest-path src-tauri/Cargo.toml
cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml
# Нативное окно/трей — отдельная проверка (нужны системные WebKit-пакеты, см. README):
cargo clippy -p mr-app --features tauri-ui --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings

# Web UI
cd ui/web/frontend && corepack enable && yarn install --immutable && yarn check && yarn build
```

После изменения исходников фронта пересоберите статику (`yarn build`) и
закоммитьте её вместе с `ui/web/static/.mr-stamp` — так чистый клон запускается без Node.

## Правила

* Отправляя PR, вы соглашаетесь распространять свой вклад на условиях MIT
  (`LICENSE`). Отдельный CLA не требуется.
* Соблюдайте `CODE_OF_CONDUCT.md`.
* Не коммитьте `people.json`, `voiceprints.json`, `config.json`, аудиозаписи,
  логи и скачанные модели — это личные данные или артефакты.
* Ноль варнингов clippy, форматирование cargo fmt — обязательно.
* `tauri_shell.rs` и мобильная точка входа компилируются только с `--features tauri-ui` —
  проверяйте, если трогали их.
* Правила архитектуры (P1–P12) — в `AGENTS.md`.
* Сообщения коммитов — Conventional Commits: `<type>(<scope>): <subject>`, типы `feat|fix|refactor|perf|test|docs|chore`.
* Документация и комментарии — на русском (P11).

## Перевод UI

Тексты интерфейса — на русском. Если добавляете язык, держите оригинальные строки
и прикладывайте скриншоты в отдельном PR.
