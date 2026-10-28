# Web UI Meeting Recorder

Svelte 5 + TypeScript + Vite. Исходники - `src/`, прод-сборка - `../../web/static/`
(имена детерминированы: `css/app.css`, `js/app.js`; её раздаёт Rust-ядро).

```bash
corepack enable
yarn install --immutable
yarn check   # svelte-check + tsc
yarn build   # сборка в ../static
```

Архитектура проекта и правила - в корневом `AGENTS.md`.
