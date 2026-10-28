# Фронт (Svelte 5 + Vite + TypeScript) - AGENTS.md

Одностраничное локальное окно (Tauri WebView) + тот же UI в браузере (`--web`).
Бэкенд не трогаем: единственный мост - HTTP REST (`mr-app`), контракт сторожит
Rust-тест `restored_endpoints_roundtrip`. Офлайн: ни CDN, ни веб-шрифтов, ни аналитики.

## Команды (в `ui/web/frontend`)

```bash
yarn install --immutable   # зависимости (Yarn 4, lock в git)
yarn dev                   # dev-сервер :5173, /api и /audio проксируются на :8765 (нужен запущенный бэкенд)
yarn check                 # svelte-check: 0 ошибок и 0 предупреждений - обязательно
yarn build                 # прод-сборка в ../static (index.html + css/app.css + js/app.js, имена без хешей)
```

`ui/web/static/` - только dist, руками не править (перезаписывается сборкой).
`run.sh` сам пересобирает фронт при изменении исходников.

## Структура `src/`

- `main.ts`, `App.svelte` - точка входа, шапка, двухколоночный макет, футер, модалки.
- `lib/api.ts` - типы DTO бэкенда (`RecordingItem`, `TranscriptData`, `SpeakerInfo`, …) + `fetch`-обёртки.
  DTO меняются только вместе с Rust-структурами; сначала править Rust + его тесты.
- `lib/*.svelte.ts` - разделяемое состояние на рунах (`$state`): `ui` (модалки + текущий файл),
  `people`, `recordings`, `support`, `transcript`, `settings`, события `on/emit` (там же).
  Импорт рун-модулей - всегда с полным именем файла: `from './ui.svelte.ts'`
  (голый `.svelte` резолвится в компонент, а не в модуль - проверено кровью).
- `components/` - по одному компоненту на файл: `RecordingDeck`, `People`, `Recordings`,
  `SlimPlayer` (свой slim-плеер вместо нативных `<audio>`), `TranscriptModal`, `TranscriptDoc`,
  `SpeakersPane`, `SettingsModal`, `GuestModal`, `DonateModal`, `Toasts`.
- `styles/*.css` - глобальные слои `@layer tokens, base, components, responsive`
  (импорт порядком каскада в `app.css`). Компонентные `<style>` - без слоёв:
  незалейеренное бьёт лейерное, компонент всегда поверх базы. Новые стили -
  в `<style>` компонента, в глобальные стили - только токены и кросс-компонентное.

## Правила

- Русский интерфейс - по правилам технического стиля (технический стиль): глаголы, термины едины
  («гость», «Текст и спикеры», «Модели ИИ»), без эмодзи в интерфейсе.
- Светлая тема: фон всегда белый, тёмные - только текст и акценты
  (кнопки `.btn-primary`, активный сегмент, play-кнопки). Серых плашек не добавлять.
- Нативные `<audio>`/`<video>`-контролы - только где нужна картинка видео;
  звук всегда через `SlimPlayer` (перемотка, `#t=`-фрагменты, пауза соседних треков).
- `role="dialog"` + `aria-modal` + `tabindex="-1"` у модалок; Esc и клик по фону закрывают.
- Никаких SvelteKit/SSR/роутеров/Tailwind/Storybook: одна страница, три модалки, ноль зависимостей
  рантайма кроме `svelte`. Зависимости dev - только toolchain (vite, svelte-check, TS).
- Перед коммитом: `yarn check` чисто + `yarn build` + `cargo test` (Rust-тесты сверяют маркеры
  `transcript-grid`, `packsListSettings` в бандле и отдают всю статику с 200).
