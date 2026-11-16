# Подтекст (Podtekst) — Web UI

Svelte 5 + TypeScript + Vite. Исходники - `src/`, прод-сборка - `../../web/static/`
(имена детерминированы: `css/app.css`, `js/app.js`; её раздаёт Rust-ядро).

```bash
corepack enable
yarn install --immutable
yarn lint    # Biome: формат, линт, импорты
yarn check   # svelte-check + tsc
yarn build   # сборка в ../static
```

Архитектура проекта — `docs/architecture.md`, правила — `docs/ai/code-style.md`, карта для
агентов — корневой `AGENTS.md`.
