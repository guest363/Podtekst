# Релизы через GitHub

Версии выпускаются тегами `vMAJOR.MINOR.PATCH`. Push тега запускает
`.github/workflows/release.yml`: он собирает standalone-бинарники и Tauri-бандлы
для Linux/macOS/Windows, APK для Android, проверяет пакеты смоуками и создаёт
GitHub Release.

## 1. Подготовка версии

Обновите версию в:

* `src-tauri/crates/*/Cargo.toml` (5 крейтов);
* `src-tauri/crates/mr-app/tauri.conf.json`;
* `ui/web/frontend/package.json`;
* `CHANGELOG.md`;
* `.github/release-notes/vX.Y.Z.md` - описание релиза.

Описание пишется руками: 8-15 строк по-русски, что изменилось для человека
и как обновиться. Workflow публикует его как тело релиза, кладёт в `notes`
файла `latest.json` (до 16 КБ) и падает, если файла нет или в нём нет ссылки
на `docs/UNSIGNED-INSTALL.md`. Список коммитов вместо вычитанного текста
не выпускаем.

Версия в теге обязана совпадать с версией в `mr-app/Cargo.toml`,
`tauri.conf.json` и `ui/web/frontend/package.json`. Workflow проверяет все три
до сборки.

Затем:

```bash
cd src-tauri
cargo check --workspace
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p mr-app --features tauri-ui --all-targets --locked -- -D warnings
cargo test --workspace --locked

cd ../ui/web/frontend
yarn install --immutable
yarn check
yarn build

cd ../..
git status --short
```

Если менялся фронт, закоммитьте `ui/web/static` и `.mr-stamp`.

## 2. Проверка перед тегом

- [ ] `git status --porcelain` пуст;
- [ ] CI на `main` зелёный;
- [ ] ссылки в `README.md` и `SECURITY.md` ведут на `guest363/podtekst`;
- [ ] в `git ls-files` нет личных данных, логов, APK и моделей;
- [ ] `THIRD_PARTY_LICENSES.md` обновлён;
- [ ] `CHANGELOG.md` содержит секцию версии;
- [ ] в `.github/release-notes/` лежит описание для тега версии со ссылкой
      на `docs/UNSIGNED-INSTALL.md`.

## 3. Тег и релиз

```bash
git tag -a v0.1.1 -m "Podtekst 0.1.1"
git push origin main
git push origin v0.1.1
```

После push:

1. Откройте **Actions** → workflow **release**.
2. Дождитесь завершения `meta`, `build-tauri`, `build-android`, четырёх
   смоуков `smoke-*`, `manifest`, `publish` и `verify-published`.
3. Проверьте **Releases**: тег, заметки, артефакты, `latest.json` и
   `SHA256SUMS`.
4. Сверьте тело релиза с `.github/release-notes/<тег>.md`: оно публикуется
   из этого файла без правок.
5. Проверьте обновление на предыдущей версии: «Настройки» → «Проверить
   обновления» - должен появиться баннер с новым тегом. В установленной
   сборке кнопка «Обновить и перезапустить» скачивает пакет и перезапускает
   приложение; при запуске из исходников остаётся ссылка на релиз.

`latest.json` содержит ровно три платформы: `linux-x86_64`,
`darwin-aarch64`, `windows-x86_64`. В нём есть `version`, `notes`,
`pub_date` в RFC3339 и для каждой платформы `url` и `signature`. Поля `size`
в схеме нет: размер при необходимости берётся отдельно. `SHA256SUMS`
считается после создания `latest.json`, поэтому манифест входит в контрольные
суммы.

Если workflow упал, исправьте причину, удалите тег и создайте заново:

```bash
git tag -d v0.1.1
git push origin :refs/tags/v0.1.1
# после исправления и merge в main
git tag -a v0.1.1 -m "Podtekst 0.1.1"
git push origin v0.1.1
```

## 4. Подпись и предупреждения ОС

Релизы сознательно не подписываются Apple Developer ID и Windows
Authenticode. Платные сертификаты не окупаются, поэтому macOS Gatekeeper и
Windows SmartScreen показывают предупреждение. Это политика проекта, а не
недоделка.

Пользовательская инструкция: `docs/UNSIGNED-INSTALL.md` (проверка SHA-256,
снятие карантина на macOS, SmartScreen и Smart App Control на Windows).
Не отключайте защиту операционной системы глобально и не добавляйте
сертификаты в репозиторий.

### Ключ самообновления (обязателен)

Штатный апдейтер Tauri ставит только пакеты, подписанные ключом, публичная
часть которого лежит в `src-tauri/crates/mr-app/tauri.conf.json`
(`plugins.updater.pubkey`). Это единственная криптографическая проверка
обновления. Один раз сгенерируйте пару:

```bash
cargo tauri signer generate -w ~/.tauri/podtekst.key
```

Приватный ключ сохраните в надёжном месте и добавьте в GitHub Actions
секретом `TAURI_SIGNING_PRIVATE_KEY` (содержимое файла). Если у ключа есть
пароль - ещё и `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Не генерируйте второй
ключ: старые копии не примут подписи нового.

Секрет обязателен и для тега, и для dry run: без него `meta` завершает
workflow с `::error::`, а не собирает релиз без автообновления. Потеря ключа
означает, что уже установленные копии больше не смогут обновляться: им
придётся один раз скачать пакет вручную.

### Android release keystore

Для тега обязательны четыре секрета:

* `ANDROID_KEYSTORE_BASE64` - base64 от release keystore;
* `ANDROID_KEYSTORE_PASSWORD` - пароль хранилища;
* `ANDROID_KEY_ALIAS` - alias ключа;
* `ANDROID_KEY_PASSWORD` - пароль ключа.

Workflow декодирует keystore в `$RUNNER_TEMP` и передаёт Gradle пути через
`ANDROID_KEYSTORE_PATH`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`,
`ANDROID_KEY_PASSWORD`. Без них на теге сборка падает. Локально переменные
можно не задавать: release APK соберётся с debug-подписью и предупреждением
в логе. Debug-сборка не меняется.

## 5. Сухой прогон

```bash
gh workflow run release.yml -f dry_run=true
```

Можно запускать на ветке: если ref не тег, тег берётся из версии
`mr-app/Cargo.toml` (`v0.4.1`). Dry run собирает пакеты, прогоняет четыре
смоука, собирает и проверяет `latest.json`, `SHA256SUMS` и подписи, но не
публикует релиз. Для `workflow_dispatch` отсутствие ссылки на
`docs/UNSIGNED-INSTALL.md` в исторических заметках допускается; при push
тега ссылка обязательна. Публикация выполняется только для push тега.
`workflow_dispatch` не публикует никогда, независимо от `dry_run`.

## 6. Проверка после релиза

После `publish` job `verify-published` сам скачивает опубликованный
`latest.json` и `SHA256SUMS`, проверяет версию, URL, хэши и minisign-подписи
каждой платформы. Отдельный workflow:

```bash
gh workflow run verify-release.yml -f tag=v0.1.1
# без tag - последний релиз
gh workflow run verify-release.yml
```

Локально:

```bash
gh release download v0.1.1 --pattern SHA256SUMS
gh release download v0.1.1 --pattern latest.json
sha256sum -c SHA256SUMS --ignore-missing
```

Ссылка на релиз: https://github.com/guest363/podtekst/releases
