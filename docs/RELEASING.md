# Релизы через GitHub

Версии выпускаются тегами `vMAJOR.MINOR.PATCH`. Push тега запускает
`.github/workflows/release.yml`: он собирает standalone-бинарники и Tauri-бандлы
для Linux/macOS/Windows, затем создаёт GitHub Release и прикладывает артефакты.

## 1. Подготовка версии

Обновите версию в:

* `src-tauri/crates/*/Cargo.toml` (5 крейтов);
* `src-tauri/crates/mr-app/tauri.conf.json`;
* `ui/web/frontend/package.json`;
* `CHANGELOG.md`.

Версия в теге обязана совпадать с версией в `mr-app/Cargo.toml`: приложение
сравнивает её с тегом последнего GitHub Release и предлагает обновиться.
Workflow `release` проверяет это перед публикацией и падает при расхождении.

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
- [ ] ссылки в `README.md` и `SECURITY.md` ведут на `guest363/meeting-record`;
- [ ] в `git ls-files` нет личных данных, логов, APK и моделей;
- [ ] `THIRD_PARTY_LICENSES.md` обновлён;
- [ ] `CHANGELOG.md` содержит секцию версии.

## 3. Тег и релиз

```bash
git tag -a v0.1.1 -m "Meeting Recorder 0.1.1"
git push origin main
git push origin v0.1.1
```

После push:

1. Откройте вкладку **Actions** → workflow **release**.
2. Дождитесь завершения `build-rust-core`, `build-tauri`, `publish`.
3. Проверьте **Releases**: тег, заметки, артефакты и `SHA256SUMS`.
4. Скачайте по одному артефакту на платформу и проверьте запуск на чистой машине.
5. Проверка обновлений: на предыдущей версии откройте «Настройки» →
   «Проверить обновления» — должен появиться баннер с новым тегом. В
   установленной сборке (AppImage / `.app` / NSIS) кнопка «Обновить и
   перезапустить» скачивает подписанный пакет и перезапускает приложение;
   при запуске из исходников (`run.sh`) остаётся ссылка на релиз.
6. Проверьте, что в релизе есть `latest.json` и файлы `*.sig` — по ним
   приложение находит и проверяет обновление.

Если workflow упал, исправьте причину, удалите тег и создайте заново:

```bash
git tag -d v0.1.1
git push origin :refs/tags/v0.1.1
# после исправления и merge в main
git tag -a v0.1.1 -m "Meeting Recorder 0.1.1"
git push origin v0.1.1
```

## 4. Подпись

Сейчас релизы не подписаны. Для macOS нужен Apple Developer ID и notarization,
для Windows — code signing certificate. До этого в release notes указывайте,
что Gatekeeper/SmartScreen могут показать предупреждение. Ключи и сертификаты
в репозиторий не коммитятся.

### Ключ самообновления

Штатный апдейтер Tauri ставит только пакеты, подписанные ключом, публичная
часть которого лежит в `src-tauri/crates/mr-app/tauri.conf.json`
(`plugins.updater.pubkey`). Один раз сгенерируйте пару:

```bash
cargo tauri signer generate -w ~/.tauri/meeting-recorder.key
```

Приватный ключ (`~/.tauri/meeting-recorder.key`) сохраните в надёжном месте и
добавьте в GitHub Actions секретом `TAURI_SIGNING_PRIVATE_KEY` (содержимое
файла). Если у ключа есть пароль — ещё и `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
Потеря ключа означает, что уже установленные копии больше не смогут
обновляться: им придётся один раз скачать пакет вручную.

Без секрета workflow соберёт релиз без автообновления (предупреждение в
логе): `latest.json` и `*.sig` не появятся, у пользователей останется баннер
со ссылкой на релиз.

Подпись macOS включается без правки workflow: задайте в GitHub Actions секреты

* `APPLE_CERTIFICATE` — base64 от `.p12` сертификата Developer ID Application;
* `APPLE_CERTIFICATE_PASSWORD` — пароль `.p12`;
* `APPLE_SIGNING_IDENTITY` — например `Developer ID Application: ООО «…» (TEAMID)`;
* `APPLE_ID`, `APPLE_PASSWORD` — Apple ID и app-specific password для нотаризации;
* `APPLE_TEAM_ID` — Team ID.

Если `APPLE_CERTIFICATE` не задан, сборка остаётся неподписанной, а в README
остаётся инструкция `xattr -dr com.apple.quarantine /Applications/MeetingRecorder.app`.

## 5. Проверка после релиза

```bash
gh release view v0.1.1
gh release download v0.1.1 --pattern SHA256SUMS
sha256sum -c SHA256SUMS
```

Ссылка на релиз: https://github.com/guest363/meeting-record/releases
