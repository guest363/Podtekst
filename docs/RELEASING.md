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
git tag -a v0.1.0 -m "Meeting Recorder 0.1.0"
git push origin main
git push origin v0.1.0
```

После push:

1. Откройте вкладку **Actions** → workflow **release**.
2. Дождитесь завершения `build-rust-core`, `build-tauri`, `publish`.
3. Проверьте **Releases**: тег, заметки, артефакты и `SHA256SUMS`.
4. Скачайте по одному артефакту на платформу и проверьте запуск на чистой машине.
5. Проверка обновлений: на предыдущей версии откройте «Настройки» →
   «Проверить обновления» — должен появиться баннер с новым тегом и ссылкой
   на релиз. Автоустановки нет: пользователь скачивает пакет вручную.

Если workflow упал, исправьте причину, удалите тег и создайте заново:

```bash
git tag -d v0.1.0
git push origin :refs/tags/v0.1.0
# после исправления и merge в main
git tag -a v0.1.0 -m "Meeting Recorder 0.1.0"
git push origin v0.1.0
```

## 4. Подпись

Сейчас релизы не подписаны. Для macOS нужен Apple Developer ID и notarization,
для Windows — code signing certificate. До этого в release notes указывайте,
что Gatekeeper/SmartScreen могут показать предупреждение. Ключи и сертификаты
в репозиторий не коммитятся.

## 5. Проверка после релиза

```bash
gh release view v0.1.0
gh release download v0.1.0 --pattern SHA256SUMS
sha256sum -c SHA256SUMS
```

Ссылка на релиз: https://github.com/guest363/meeting-record/releases
