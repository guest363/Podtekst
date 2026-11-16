# Участие в разработке

Спасибо за интерес к проекту! PR приветствуются: исправления, документация, поддержка
платформ. Крупные предложения - сначала создайте issue для обсуждения.

## Проверки перед PR (все гоняет CI)

Единая точка входа — `./scripts/verify.sh`; её же запускают `lefthook` и CI, поэтому
команды не расходятся:

```bash
./scripts/verify.sh --fast   # P11 и инструкции: секунды, без сборки
./scripts/verify.sh --rust   # формат, clippy, тесты
./scripts/verify.sh --ui     # lint, check, build, штамп статики
./scripts/verify.sh --tauri  # clippy с tauri-ui (нужны системные WebKit-пакеты, см. README)
./scripts/verify.sh --all    # всё перечисленное перед PR
```

После изменения исходников фронта `--ui` пересобирает статику и пересчитывает
`ui/web/static/.mr-stamp`; закоммитьте их вместе - так чистый клон запускается без Node.
Перед первым `--ui` один раз установите зависимости:
`cd ui/web/frontend && corepack enable && yarn install --immutable`.

## Git hooks (lefthook)

`lefthook.yml` запускает те же режимы `verify.sh` перед `git push`. Установка один раз:

```bash
go install github.com/evilmartians/lefthook@latest   # или npm i -g lefthook / brew install lefthook
lefthook install
```

Осознанный обход без проверок - `git push --no-verify`.

## Правила

* Отправляя PR, вы соглашаетесь распространять свой вклад на условиях MIT
  (`LICENSE`). Отдельное соглашение о передаче прав (CLA) не требуется.
* Соблюдайте `CODE_OF_CONDUCT.md`.
* Не коммитьте `people.json`, `voiceprints.json`, `config.json`, аудиозаписи,
  логи и скачанные модели - это личные данные или артефакты.
* Ноль предупреждений clippy, форматирование cargo fmt - обязательно.
* `tauri_shell.rs` и мобильная точка входа компилируются только с `--features tauri-ui` -
  проверяйте, если трогали их.
* Правила архитектуры (P1-P12) - в `docs/ai/code-style.md`.
* Сообщения коммитов и PR - Conventional Commits: `<type>(<scope>): <subject>`, типы `feat|fix|refactor|perf|test|docs|chore`. В описаниях и коммитах - только обезличенные технические параметры записей, без личных названий файлов и контекста (P12).
* Документация и комментарии - на русском (P11).

## Перевод UI

Тексты интерфейса - на русском. Если добавляете язык, держите оригинальные строки
и прикладывайте снимки экрана в отдельном PR.
