# Фронт (Svelte 5 + Vite + TypeScript) - AGENTS.md

Одностраничное локальное окно (Tauri WebView) + тот же UI в браузере (`--web`).
Бэкенд не трогаем: единственный мост - HTTP REST (`mr-app`), контракт сторожит
Rust-тест `restored_endpoints_roundtrip`. Офлайн: ни CDN, ни веб-шрифтов, ни аналитики.

## Команды (в `ui/web/frontend`)

```bash
yarn install --immutable   # зависимости (Yarn 4, lock в git)
yarn dev                   # dev-сервер :5173, /api и /audio проксируются на :8765 (нужен запущенный бэкенд)
yarn lint                  # Biome: формат, линт, organize imports; 0 диагностик - обязательно
yarn lint:fix              # то же с автоисправлениями
yarn format                # только форматирование
yarn check                 # svelte-check: 0 ошибок и 0 предупреждений - обязательно
yarn build                 # прод-сборка в ../static (index.html + css/app.css + js/app.js, имена без хешей)
```

`ui/web/static/` - только dist, руками не править (перезаписывается сборкой).
`run.sh` сам пересобирает фронт при изменении исходников. После `yarn build` нужно
пересчитать `ui/web/static/.mr-stamp` (команда в `docs/ai/workflow.md`).

## Структура `src/`

- `main.ts`, `app.svelte` - точка входа, шапка, двухколоночный макет, футер, модалки.
- `lib/api.ts` - типы DTO бэкенда (`RecordingItem`, `TranscriptData`, `SpeakerInfo`, …) + `fetch`-обёртки.
  DTO меняются только вместе с Rust-структурами; сначала править Rust + его тесты.
- `lib/guards.ts` - `isRecord`, `errText`, `isAbortError` для безопасной работы с `unknown`.
- `lib/*.svelte.ts` - разделяемое состояние на рунах (`$state`): `ui` (модалки + текущий файл),
  `people`, `recordings`, `support`, `transcript`, `settings`, события `on/emit` (там же).
  Импорт рун-модулей - всегда с полным именем файла: `from './ui.svelte.ts'`
  (голый `.svelte` резолвится в компонент, а не в модуль - проверено кровью).
- `components/` - по одному компоненту на файл, имена kebab-case: `recording-deck.svelte`,
  `people.svelte`, `recordings.svelte`, `slim-player.svelte` (свой slim-плеер вместо нативных
  `<audio>`), `transcript-page.svelte`, `transcript-doc.svelte`, `speakers-pane.svelte`,
  `settings-modal.svelte`, `system-settings-tab.svelte`, `update-modal.svelte`,
  `crash-banner.svelte`, `donate-modal.svelte`, `toasts.svelte`.
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
  рантайма кроме `svelte`. Зависимости dev - только toolchain (vite, svelte-check, TS, Biome).
- **Biome** - единственный форматтер и линтер. `yarn lint` должен быть зелёным до коммита.
  Biome форматирует только `<script>` в `.svelte`; разметку и `<style>` проверяет человек,
  типы и a11y - `svelte-check`. Не включать `html.experimentalFullSupportEnabled`:
  в Biome 2.4.4 он ломает форматирование JS в `<script>`.
- **Типы:** только `type`, `interface` - лишь для declaration merging. `import type` для
  тип-only импортов. `any` запрещён; `as` - только на границе с внешними данными и с
  комментарием `// SAFETY: причина`.
- **Функции:** только `const name = (...) => {}`. Объявления `function` в новом коде запрещены.
- **Svelte 5:** только runes (`$props`, `$state`, `$derived`, `$effect`), сниппеты вместо слотов.
  `export let`, `$:` и `on:click` в новом коде запрещены.
- **Имена файлов:** kebab-case (проверяется `style/useFilenamingConvention`).
- Перед коммитом: `yarn lint` + `yarn check` + `yarn build` + пересчёт `.mr-stamp` + `cargo test`
  (Rust-тесты сверяют маркеры `transcript-grid`, `packsListSettings` в бандле и отдают всю статику с 200).
