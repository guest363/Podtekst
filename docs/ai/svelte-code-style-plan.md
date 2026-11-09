# План миграции Svelte-кода на Biome и стиль `ads-players-new`

> Статус: реализовано в ветке `chore/frontend-biome-style` (этапы 1–7, включая
> kebab-case). Документ оставлен как обоснование решений и карта проверок.

Статус: исследование и план. Исходный код не изменён; все замеры и эксперименты
выполнены на копиях в `/tmp/opencode` с Biome 2.4.4 из
`~/Code/ads-players-new/node_modules/.bin/biome`.

Область: `ui/web/frontend` (Svelte 5 + TypeScript + Vite). Rust-ядро и контракты
HTTP REST не затрагиваются.

---

## 1. Цель, границы, принципы

### 1.1 Цель

Перенести в текущий репозиторий применимые конвенции `ads-players-new`:

- Biome как единый форматтер, линтер и организатор импортов;
- `type` вместо `interface`;
- `const`-стрелки вместо объявлений `function`;
- `import type` для типов;
- запрет `any` и жёсткий бюджет `as`;
- Svelte 5 только на рунах (уже выполнено);
- русские комментарии «зачем» (уже выполнено);
- опционально — kebab-case имён файлов.

Итог — серия независимых коммитов без изменения поведения UI. Каждый коммит,
меняющий `src`, пересобирает `ui/web/static` и обновляет `.mr-stamp`.

### 1.2 Входит

- `ui/web/frontend/biome.json`, `package.json`, `yarn.lock`.
- `ui/web/frontend/src/**/*.ts`, `src/**/*.svelte` (только `<script>`),
  `src/styles/*.css`, `src/app.css`.
- `ui/web/frontend/scripts/*.mjs`, конфиги `vite.config.ts`, `svelte.config.js`,
  `tsconfig*.json`, `index.html` — если Biome их форматирует.
- `ui/web/static` (пересборка) и `ui/web/static/.mr-stamp`.
- CI-шаг `yarn lint`, документация `AGENTS.md`, `docs/ai/workflow.md`,
  `CONTRIBUTING.md`, `README.md`, шаблон PR.

### 1.3 Не входит

- Rust-код, HTTP REST-контракты, форматы `.transcript.json`, `voiceprints.json`.
- Переписывание логики компонентов, изменение разметки и стилей «по вкусу».
- SvelteKit/SSR/роутеры/Tailwind/Storybook, новые рантайм-зависимости.
- `prettier-plugin-svelte` и любой второй форматтер.
- Обязательный pre-commit-хук (решение — отдельный вопрос).
- Массовое добавление JSDoc ради JSDoc (противоречит P11 текущего репозитория).

### 1.4 Принципы

1. **Без изменения поведения.** Форматирование, `type`, `const`-стрелки,
   `import type`, guards не меняют наблюдаемое поведение. Исключения — только
   исправление реальных ошибок Biome (`forEach` с возвратом, присваивание в
   выражении, присваивание параметру), которые и сейчас являются дефектами стиля.
2. **Один этап — один коммит.** Форматирование не смешивается с типами,
   типами не смешивается с функциями.
3. **P1/P2 текущего репозитория.** Не плодить новые сущности; для `errText`,
   `isRecord` и response-типов использовать существующие DTO и модули.
4. **P9.** CLI-флаги, REST-контракты и форматы не ломаются. Переименования файлов
   (этап 7) — только по отдельному решению.
5. **P12.** В commit/PR-текстах и тестовых описаниях — только обезличенные
   технические параметры, без имён людей и названий записей.
6. **Один источник правды.** Правила стиля живут в `biome.json` и
   `ui/web/frontend/AGENTS.md`; не дублировать их в нескольких документах.

### 1.5 Что не переносим из `ads-players-new`

| Конвенция vpaid | Почему не переносим |
|---|---|
| P14 (VPAID/VAST-события) | Доменная специфика плеера, к текущему UI неприменима |
| SCSS, `svelte-preprocess`, `@df/scss` | Здесь чистый CSS + Cascade Layers; препроцессор не нужен |
| Монорепо-алиасы `@df/*`, `workspace:*`, Turbo | Один пакет; импорты относительные, рун-модули — с `.svelte.ts` |
| Запрет `export default` в библиотечном TS | `src/main.ts` — точка входа Vite/Svelte, `export default app` остаётся |
| `docs/ai/package-registry.md`, `tech-debt.md` | Слишком тяжёлый процесс для одного фронтенд-пакета |
| Обязательный JSDoc для каждой функции (P13) | Текущий P11 запрещает пересказ кода; JSDoc — только «зачем»/контракт |
| `html.experimentalFullSupportEnabled` | В 2.4.4 отключает форматирование JS в `.svelte` и добавляет a11y-шум (см. 3.4) |
| `prettier-plugin-svelte` | Второй форматтер; Biome форматирует `<script>`, разметку оставляем человеку |
| Обязательный kebab-case | Опциональный этап 7; PascalCase компонентов — текущая конвенция Svelte |
| `console.*` в коде пакетов | В `src` уже 0; отдельное правило не добавляем, чтобы не ломать dev-скрипт |

---

## 2. Что переносим: соответствие

> Номера P… в этой таблице — из `ads-players-new/docs/ai/code-style.md`.
> Не путать с P1–P12 текущего `AGENTS.md`: у них другая нумерация.

| Из `ads-players-new` | Что делаем у нас | Чем проверяем |
|---|---|---|
| Корневой `biome.json` + пакетные `extends` | Один `ui/web/frontend/biome.json`, без `extends`/`root:false` | `yarn biome --version`, `yarn lint` |
| `formatter` 2 пробела, organize imports | То же; кавычки/`;`/`lineWidth` — по решению (вопрос а) | `yarn lint`, diff |
| `package.json`: `lint`/`format`/`pre-commit` | `lint`, `lint:fix`, `format`; `check` не переименовываем | `yarn lint`, `yarn check` |
| Lefthook pre-commit | CI-шаг `yarn lint`; хук — опционально | CI, ручной прогон |
| P8 `type` > `interface` | 77 `interface` → `type`; 1 `extends` вручную | `grep`, `yarn check` |
| P7 `const`-стрелки | 336 top-level + 2 вложенные функции | codemod, `yarn check`, smoke |
| P11 `import type` | Уже почти везде; 1 нарушение | Biome `useImportType`, `tsc` |
| P5 `any` | 4 явных `any` → `unknown`/DTO | `noExplicitAny`, `grep` |
| P6 `as` | ≈180 `as` → guards/generic/`instanceof`; ≤10 с `// SAFETY:` | `yarn check`, ревью |
| P9 runes-only | Уже выполнено: 0 `export let`, `$:`, `on:click` | `grep`, `svelte-check` |
| P13 русские комментарии | Уже русские; JSDoc — только «зачем» | `./scripts/p11-diff.sh` |
| Kebab-case файлов | Опциональный этап 7 | `useFilenamingConvention` |

---

## 3. Конфигурация

### 3.1 `ui/web/frontend/biome.json` — стартовый вариант (этап 1)

Рекомендуемый вариант — **минимальный diff**: сохранить текущие одинарные
кавычки и отсутствие точек с запятой, `lineWidth: 100`. Полный паритет с vpaid
(двойные кавычки, `;`, `lineWidth: 80`) — альтернатива, см. вопрос (а).

```json
{
  "$schema": "./node_modules/@biomejs/biome/configuration_schema.json",
  "files": {
    "includes": [
      "**",
      "!**/node_modules/**",
      "!**/.yarn/**",
      "!**/.svelte-check/**",
      "!**/public/**",
      "!../static/**"
    ]
  },
  "formatter": {
    "enabled": true,
    "indentStyle": "space",
    "indentWidth": 2,
    "lineWidth": 100
  },
  "javascript": {
    "formatter": {
      "quoteStyle": "single",
      "semicolons": "asNeeded"
    }
  },
  "assist": {
    "actions": {
      "source": {
        "organizeImports": "on"
      }
    }
  },
  "linter": {
    "rules": {
      "recommended": true,
      "style": {
        "noNamespace": "error",
        "noParameterAssign": "error",
        "useAsConstAssertion": "error",
        "useDefaultParameterLast": "error",
        "useEnumInitializers": "error",
        "useSelfClosingElements": "error",
        "useSingleVarDeclarator": "error",
        "noUnusedTemplateLiteral": "error",
        "useNumberNamespace": "error",
        "noInferrableTypes": "error",
        "noUselessElse": "error"
      },
      "complexity": {
        "noImportantStyles": "off"
      }
    }
  },
  "json": {
    "parser": {
      "allowComments": true
    },
    "linter": {
      "enabled": true
    }
  },
  "css": {
    "formatter": {
      "enabled": true,
      "quoteStyle": "single",
      "lineWidth": 100
    },
    "linter": {
      "enabled": true
    }
  },
  "vcs": {
    "enabled": true,
    "clientKind": "git"
  },
  "overrides": [
    {
      "includes": [
        "**/*.config.js",
        "**/*.config.cjs",
        "**/*.config.mjs",
        "**/*.config.ts",
        "**/*.d.ts"
      ],
      "linter": {
        "rules": {
          "style": {
            "noDefaultExport": "off"
          }
        }
      }
    },
    {
      "includes": ["**/*.svelte", "**/*.astro", "**/*.vue"],
      "linter": {
        "rules": {
          "style": {
            "useConst": "off",
            "useImportType": "off"
          },
          "correctness": {
            "noUnusedVariables": "off",
            "noUnusedImports": "off"
          }
        }
      }
    }
  ]
}
```

Почему `noImportantStyles: "off"`: в `src/styles/*.css` есть осознанные
`!important` для `[hidden]`, print-режима и перекрытия модалок. Это не «грязный
стиль», а инструмент каскада; 8 предупреждений иначе останутся навсегда.

`json.parser.allowComments` нужен из-за комментариев в `tsconfig.app.json`.
`files.includes` не включает `../static`: сборка лежит вне пакета и не линтуется.

### 3.2 Целевые добавки (этапы 3 и 4)

На этапе 3 в `linter.rules.style` добавляется:

```json
"useConsistentTypeDefinitions": {
  "level": "error",
  "options": { "style": "type" }
}
```

На этапе 4, после удаления `any`, в `linter.rules.suspicious` добавляется:

```json
"noExplicitAny": "error"
```

До этапа 3 правило типов не включаем: иначе `yarn lint` падает на 77
`interface` и каждый промежуточный коммит красный. До этапа 4 `noExplicitAny`
остаётся рекомендованным предупреждением (4 штуки).

### 3.3 `package.json`

Фрагмент; остальные поля (`version`, `packageManager`, порядок зависимостей)
не меняются.

```json
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview",
    "check": "svelte-check --tsconfig ./tsconfig.app.json --tsgo && tsc -p tsconfig.node.json && node scripts/check-summary-fixtures.mjs",
    "lint": "biome check .",
    "lint:fix": "biome check --write .",
    "format": "biome format --write ."
  },
  "devDependencies": {
    "@biomejs/biome": "^2.4.4",
    "@sveltejs/vite-plugin-svelte": "^7.3.1",
    "@tsconfig/svelte": "^5.0.8",
    "@types/node": "^26.6.6",
    "@typescript/native": "npm:typescript@^7.0.2",
    "svelte": "^5.57.3",
    "svelte-check": "^4.7.8",
    "typescript": "~6.0.3",
    "vite": "^8.3.5"
  }
}
```

`lint` — полный `biome check` (формат + линт + organize imports), без записи.
`lint:fix` — то же с записью. `format` — только форматирование. Существующий
`check` (svelte-check + tsc + fixtures) не переименовываем.

Установка:

```bash
cd ui/web/frontend
yarn add -D @biomejs/biome@^2.4.4
yarn biome --version   # 2.4.4
```

`@biomejs/biome` — dev-зависимость, а не рантайм: в бандл не попадает, Node в
рантайме по-прежнему не нужен. Версия `^2.4.4` совпадает с `ads-players-new`.

### 3.4 Влияние на `yarn.lock` и `.mr-stamp`

- `yarn add` меняет `package.json` и `yarn.lock`; оба входят в хеш `.mr-stamp`.
- `biome.json` в хеш **не входит** — и не должен: он не влияет на бандл.
- CI считает stamp по списку: `src`, `index.html`, `package.json`, `yarn.lock`,
  `vite.config.ts` (`.github/workflows/ci.yml`, job `frontend`, шаг
  «Static build is committed»). Точная команда:

```bash
root="$(git rev-parse --show-toplevel)"
expected="$(cd "$root" && find ui/web/frontend/src ui/web/frontend/index.html ui/web/frontend/package.json ui/web/frontend/yarn.lock ui/web/frontend/vite.config.ts -type f -print | LC_ALL=C sort | while IFS= read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)"
printf '%s\n' "$expected" > "$root/ui/web/static/.mr-stamp"
```

- `yarn build` очищает `../static` и удаляет `.mr-stamp`; после каждой сборки
  stamp нужно записать заново этой командой.
- Этап 1 меняет stamp, даже если `app.js` побайтово тот же: хешируются
  `package.json`/`yarn.lock`. Это ожидаемо.

### 3.5 Ограничения Biome 2.4.4 для `.svelte` (проверено эмпирически)

Эксперимент: копия `src` + `scripts` в `/tmp/opencode`, конфиг по образцу vpaid,
`biome check`/`format` без записи и с записью.

| Проверка | Результат |
|---|---|
| Parse errors | **0** на 61 файле; все 24 `.svelte` разбираются, включая `Recordings`, `SettingsModal`, `TranscriptDoc` |
| Что форматируется в `.svelte` | **Только содержимое `<script>`**: отступы, кавычки, `;`, переносы |
| Разметка | Не меняется (побайтовое сравнение сегментов вне `<script>`: 0 отличий) |
| `<style>` внутри `.svelte` | Не меняется (входит в «разметку» и не форматируется) |
| CSS-файлы | Форматируются и линтуются |
| `organizeImports` в `.svelte` | Работает: сортирует импорты внутри `<script>` |
| `html.experimentalFullSupportEnabled: true` | **Не включать**: в 2.4.4 перестаёт форматировать JS в `<script>`, разметку всё равно не переформатирует, зато добавляет 124 `a11y/useButtonType`, 4 `noSvgWithoutTitle`, 3 `useMediaCaption` и др. |
| `useConst` в `.svelte` | Отключён override; иначе возможны ложные срабатывания на props/`$bindable` |
| `useImportType` в `.svelte` | Отключён override; `import type` всё равно требует `verbatimModuleSyntax` через `svelte-check` |
| `noUnusedVariables`/`noUnusedImports` в `.svelte` | **Обязательно отключены**: без override Biome даёт 240 + 80 ложных срабатываний, потому что не видит использования переменных в разметке |
| `any` в `.svelte` | Ловится (`noExplicitAny`), если появится |
| `as` в разметке | Biome не видит; `as` в `<script>` видит, но правила «запрет `as`» у Biome нет |

Вывод: Biome — форматтер `<script>` + линтер TS/CSS/JSON. Разметка и `<style>`
остаются на человеке; `svelte-check` — единственный контроль типов и a11y.
Полный HTML-режим Biome не даёт выигрыша и ломает форматирование JS.

---

## 4. Фактическое состояние репозитория (baseline)

Замеры на `feat/diarization-v2-gates`, HEAD `6e0c9d0`; на момент замеров
рабочее дерево было чистым (кроме самого этого плана). К моменту финализации
плана в дереве появились незакоммиченные правки Rust в трёх файлах
(`mr-app/src/transcribe/mod.rs`, `mr-ml/src/onnx/ctc.rs`, `mr-ml/src/onnx/rnnt.rs`) —
это чужая/параллельная работа, её не трогать и не смешивать с миграцией.

| Метрика | Значение |
|---|---|
| `.svelte` | 24 (23 компонента + `App.svelte`) |
| `.ts` | 23 (21 в `src/lib` + `fixtures/index.ts` + `main.ts`) |
| `.css` | 8 глобальных + `app.css` |
| Строк `.svelte` + `.ts` + `.css` | 11 231 + 4 566 + 2 341 = **18 138** |
| `interface` | **77**: 64 в `.ts`, 13 в `.svelte`; 1 `extends` (`SummaryModelInfo extends ModelInfo`) |
| `function` (top-level) | **336**: 144 в `.svelte`, 192 в `.ts`; ещё 2 вложенные в `DatePickerPopover.svelte` |
| `function` (не-export, не-async) | 137 (98 + 39) — это подмножество, а не полный объём |
| Явные `any` | **4** |
| `as` | ≈**184**: 143 в `<script>` по AST + ≈41 в разметке; 2 из них `as const` |
| `as Error` | 49 |
| `import type` | 23 объявления; 1 нарушение (`summaryLoad.ts`) |
| `$props()` | 11 компонентов; 1 inline-тип (`UpdateModal.svelte:13`) |
| `export let` / `$:` / `on:click` | 0 / 0 / 0 |
| `console.*` / `@ts-ignore` / non-null `!` | 0 / 0 / 1 |
| JSDoc-блоки | 7 |
| `yarn check` | 0 ошибок, 0 предупреждений |
| Текущий `.mr-stamp` | совпадает с хешем CI |

Аномалии:

- `Recordings.svelte` уже отформатирован Biome с двойными кавычками, `;` и
  отступом 4 (следы прошлой ручной прогонки). Этап 2 приведёт его к общему
  стилю — это самый большой однострочный diff в одном компоненте.
- `UpdateModal.svelte:13` — единственный inline-тип props.
- `summaryLoad.ts:11` — единственный `useImportType`-нарушитель.
- `SlimPlayer.svelte` экспортирует `togglePlay`; `toggle` объявлена ниже.
- `summaryActions.ts` содержит самовызов `generateSummary`;
  `TranscriptDoc.svelte` — взаимный цикл `startEdit ↔ saveText`;
  `summaryLoad.ts` — цикл `startTicker → loadSummary → applyResponse → startTicker`.
  Все вызовы отложенные, TDZ при инициализации нет (см. 5.5).

Baseline Biome (конфиг vpaid, без записи): **91 error, 30 warning, 62 info**;
0 parse errors. Диагностики: 60 format, 59 `useTemplate`, 25 organizeImports,
8 `noImportantStyles`, 6 `noGlobalIsNan`, 5 `useOptionalChain`, 4 `noExplicitAny`,
3 `noGlobalIsFinite`, 2 `useIterableCallbackReturn` и по 1–2 прочих.
С `noImportantStyles: "off"` — 91 error, 22 warning (правило даёт только
предупреждения, ошибки не меняются).

После `biome check --write` (без `--unsafe`): **4 error, 28 warning, 59 info**;
с выключенным `noImportantStyles` — **4 error, 20 warning, 59 info**.

4 оставшихся error (их надо чинить руками, они не форматирование):

| Файл | Правило | Суть |
|---|---|---|
| `SpeakersPane.svelte` | `useIterableCallbackReturn` | `forEach`-колбэк возвращает значение |
| `TranscriptPage.svelte` | `useIterableCallbackReturn` | то же |
| `TranscriptDoc.svelte` | `noAssignInExpressions` | присваивание внутри выражения |
| `api.ts` (`fmtClock`) | `noParameterAssign` | `sec = 0` переприсваивает параметр |

20 warning: 6 `noGlobalIsNan`, 3 `noGlobalIsFinite`, 5 `useOptionalChain`,
4 `noExplicitAny`, 1 `noUnusedFunctionParameters`, 1 `noNonNullAssertion`.
59 info — почти все `useTemplate` (не блокируют exit code).

---

## 5. Поэтапный план

Порядок выбран так, чтобы после этапа 2 `yarn lint` был зелёным (0 error),
а этапы 3–5 не ломали проверку. `yarn lint` = `biome check .` включает
формат, линт и organize imports; поэтому organize imports выполняется в 2b,
а не в 4-м этапе. Это единственное отклонение от буквы задания; если нужен
строго отдельный коммит на импорты — вынесите 2b в 4a, но тогда до 4a
`yarn lint` будет красным.

### Этап 0. Baseline (0.5 ч)

Цель: зафиксировать исходное состояние, не смешать с текущей работой по
диаризации.

```bash
cd /home/moiseevov/Code/audio-record
git status --short
git switch -c chore/frontend-biome-style
corepack enable
cd ui/web/frontend
yarn check
cd ../..
cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
```

Важно: к моменту финализации плана в рабочем дереве есть незакоммиченные
правки Rust в трёх файлах (параллельная работа). Их не stash-ить, не
коммитить и не смешивать с миграцией. Миграцию делать в отдельном
worktree/ветке от согласованной базы: либо от текущего HEAD `6e0c9d0`
(если диаризация скоро вливается и стек допустим), либо от `main` после её
слияния.

Критерий: чистое дерево (или отдельный worktree), `yarn check` 0/0,
stamp = CI-хеш. Откат: удалить ветку.

### Этап 1. Toolchain без правок кода (1–2 ч)

Цель: Biome установлен, конфиг и скрипты есть, код не тронут.

Файлы:

- новый `ui/web/frontend/biome.json` (3.1, без `useConsistentTypeDefinitions`);
- `ui/web/frontend/package.json` — 3 новых скрипта (`lint`, `lint:fix`, `format`) + devDependency;
- `ui/web/frontend/yarn.lock`;
- `ui/web/static/.mr-stamp`.

Команды:

```bash
cd ui/web/frontend
yarn add -D @biomejs/biome@^2.4.4
yarn biome --version
yarn lint   # ориентировочно 91 error = 60 format + 25 organize + 6 lint error
```

Baseline-отчёт сохранить в задачу/PR (не коммитить): 91/30/62.

Затем пересобрать статику и stamp (source не менялся, `app.js` может быть тем же):

```bash
cd ui/web/frontend && yarn build
cd /home/moiseevov/Code/audio-record
root="$(git rev-parse --show-toplevel)"
expected="$(cd "$root" && find ui/web/frontend/src ui/web/frontend/index.html ui/web/frontend/package.json ui/web/frontend/yarn.lock ui/web/frontend/vite.config.ts -type f -print | LC_ALL=C sort | while IFS= read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)"
printf '%s\n' "$expected" > "$root/ui/web/static/.mr-stamp"
git status --short -- ui/web/static
```

Ожидаемый diff: `biome.json` ~120 строк, `package.json` +5/−0,
`yarn.lock` ~50–150 строк, `.mr-stamp` 1 строка. `app.js`/`app.css` — без
изменений или минимальные.

Риски: Yarn 4 может перегенерировать lock не той версией. Использовать
`corepack enable` и `packageManager: yarn@4.18.0`. Проверить
`yarn install --immutable` на чистом клоне.

Критерий: `yarn biome --version` = 2.4.4; `yarn check` 0/0; stamp совпадает;
`git diff --stat` не содержит `src/`.

Откат: revert коммита (lock/package/stamp вернутся вместе).

### Этап 2. Механическое форматирование (2–4 ч)

#### 2a. Только формат

```bash
cd ui/web/frontend
yarn format          # biome format --write .
git diff --stat
```

Затрагивает: 24 `.svelte` (только `<script>`, дедент с 2/4 пробелов до 0),
15 `.ts` из 23 (остальные уже отформатированы), 8 CSS-файлов (`app.css` не меняется),
`scripts/check-summary-fixtures.mjs`,
`vite.config.ts` (перенос продолжений), 2 golden JSON (схлопывание коротких
массивов). `package.json`, `tsconfig*`, `svelte.config.js`, `index.html` —
без изменений (проверено).

Ожидаемый diff (минимальный стиль, замер на копии):

| Тип | Файлов | Вставки | Удаления |
|---|---:|---:|---:|
| `.svelte` | 24 | 3 200 | 3 172 |
| `.ts` | 15 | 213 | 102 |
| `.css` | 8 | 1 202 | 323 |
| `.mjs` | 1 | 61 | 7 |
| `.json` | 2 | 5 | 88 |
| **Итого** | **50** | **≈4 681** | **≈3 692** |

Почти весь diff `.svelte` — дедент строк `<script>` на 2 пробела: Biome
форматирует скрипт с нулевого отступа. Разметка и `<style>` не меняются.
Для ревью полезен `git diff -w` (игнорировать whitespace), но дедент он тоже
скроет — лучше смотреть `git diff` по `.svelte` выборочно.

При полном паритете (двойные кавычки, `;`, `lineWidth: 80`) — 60 файлов,
≈8 000 вставок / 6 200 удалений, в основном за счёт `.ts` (3 079/2 612).

#### 2b. Organize imports + безопасные фиксы + 4 error

```bash
cd ui/web/frontend
yarn lint:fix        # biome check --write .: organizeImports + safe fixes
yarn lint            # всё ещё 4 error
```

`lint:fix` сортирует импорты в 25 файлах, чинит `useNumberNamespace`,
`noUnusedTemplateLiteral` и аналогичные safe-правила. После него вручную:

1. `SpeakersPane.svelte` — `forEach`-колбэк: обернуть тело в `{}`, не возвращать значение.
2. `TranscriptPage.svelte` — то же.
3. `TranscriptDoc.svelte` — `;(list ??= []).push(x)` → `if (!list) list = []` + `list.push(x)`.
4. `api.ts` `fmtClock` — `sec = 0` → локальная `const total = Number.isFinite(sec) && sec > 0 ? sec : 0`, дальше использовать `total` (имя `s` уже занято секундами).

Все четыре — без изменения поведения (возврат `forEach` игнорируется;
присваивание и переприсваивание параметра не наблюдаемы).

```bash
yarn lint    # 0 error, 20 warning, 59 info
yarn check   # 0/0
yarn build
# stamp по команде из 3.4
```

Ожидаемый diff 2b: 25 файлов с перестановкой импортов (строки не меняются,
кроме порядка), 4 файла с точечными правками, `app.js` может измениться из-за
порядка модулей; пересобрать и закоммитить.

Риски: перестановка импортов меняет порядок инициализации модулей. В текущем
коде модульные side-effect'ы — только `on(...)` в конце `summaryLoad.ts`;
конфликтов нет. Всё равно — smoke после сборки.

Критерий: `yarn lint` 0 error; `yarn check` 0/0; `yarn build`; stamp; в diff
нет логических правок, кроме четырёх перечисленных.

Откат: revert 2b; 2a можно откатить отдельно.

### Этап 3. `interface` → `type` (1–2 ч)

Цель: 77 `interface` → `type`, включая `SummaryModelInfo`.

```bash
cd ui/web/frontend
# добавить useConsistentTypeDefinitions в biome.json (3.2)
yarn biome lint --write --unsafe --only=style/useConsistentTypeDefinitions src
yarn format
yarn lint
yarn check
```

Biome автоматически конвертирует 76 из 77 (18 файлов). Вручную — только
`src/lib/api.ts:109`:

```ts
// было
export interface SummaryModelInfo extends ModelInfo {
  kind: 'summary'
}

// стало
export type SummaryModelInfo = ModelInfo & {
  kind: 'summary'
}
```

Проверка: `interface` = 0, `yarn check` 0/0, `yarn lint` 0 error.
`useConsistentTypeDefinitions` — unsafe-фикс, поэтому обязателен
`--only=style/useConsistentTypeDefinitions`; без `--only` `--unsafe` может
применить другие опасные правки.

Ожидаемый diff: 18 файлов, ~+400/−300 (развёрнутые тела типов), плюс
`api.ts` 1 файл. `app.js` не меняется (типы стираются), но stamp меняется.

Риски: `type` не поддерживает declaration merging — в коде его нет; один
`extends` заменён на `&`. `Props` в `.svelte` проверены svelte-check.

Критерий: `grep` по `interface` = 0; `yarn check`; `yarn lint`; stamp.

Откат: revert коммита и убрать правило из `biome.json`.

### Этап 4. Типовая гигиена: `import type`, `any`, `as` (6–10 ч)

Этап разбит на подкоммиты; каждый — `yarn check` + `yarn lint`.

#### 4a. `errText` и 49 `as Error`

Новый `src/lib/guards.ts`:

```ts
export const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v)

export const errText = (e: unknown, fallback = 'неизвестная ошибка'): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : fallback

export const isAbortError = (e: unknown): boolean =>
  (e instanceof Error || e instanceof DOMException) && e.name === 'AbortError'
```

Заменить 49 `(e as Error).message` на `errText(e)`, `(e as Error)?.name ===
'AbortError'` на `isAbortError(e)`. Это же убирает часть `useTemplate`-шума.
Проверка: `grep "as Error"` = 0; `yarn check`.

#### 4b. `readJsonSafe<T>` и 4 `any`

```ts
// api.ts (до этапа 5 — function; после — const-стрелка)
export async function readJsonSafe<T = Record<string, unknown>>(
  res: Response,
): Promise<T> {
  const ct = res.headers.get('content-type') || ''
  if (ct.includes('application/json')) return (await res.json()) as T // SAFETY: контракт endpoint
  const text = await res.text()
  throw new Error(`Сервер вернул HTTP ${res.status}: ${text.slice(0, 160)}`)
}
```

Всего 61 вызов `readJsonSafe`. Замер: при `T = Record<string, unknown>`
`svelte-check` даёт **57 ошибок в 11 файлах** (update 7, settings 13, RecordingDeck 8, people 8, transcript 6,
system 5, support 4, recordings 2, People 2, queue 1, Recordings 1). Их надо
закрыть response-типами (`readJsonSafe<UpdateState>(...)` и т.п.). При
`T = unknown` ошибок **253 в 13 файлах** — это отдельная, более строгая фаза;
для данной миграции достаточно `Record<string, unknown>`. Все текущие
JSON-ответы — объекты; если появится endpoint с массивом, указать
`readJsonSafe<unknown[]>` явно.

4 `any`:

| Файл | Было | Стало |
|---|---|---|
| `transcript.svelte.ts` | `let evt: any` | `let evt: unknown`, затем `isRecord`/`asString` |
| `api.ts` | `Promise<any>` | `Promise<T>` (см. выше) |
| `update.svelte.ts` | `failApply(d: any)` | `failApply(d: Record<string, unknown>)` (до этапа 5 — `function`) |
| `system.svelte.ts` | `.map((c: any) =>` | `.map((c: unknown) =>`, `isRecord` + `String(...)` |

#### 4c. DOM, union, оставшиеся `as`

- `as Error` — уже 0.
- `e.target as Node` → `e.target instanceof Node` (осторожно с `contains(null)`:
  условие `!(e.target instanceof Node && rootEl.contains(e.target))` сохраняет
  прежнее поведение).
- `closest('.x') as HTMLElement | null` → `closest<HTMLElement>('.x')`.
- `querySelectorAll('audio, video') as HTMLMediaElement[]` →
  `querySelectorAll<HTMLMediaElement>('audio, video')`.
- `document.getElementById('x') as HTMLMediaElement | null` →
  `querySelector<HTMLMediaElement>('#x')`.
- `(b as Block)` / `(b as Raw)` — после `type Block`/`type Raw` narrowing в
  `{#if b.kind === 'utt'}` работает; проверено на копии: удаление всех 31
  шаблонных каста даёт `svelte-check` 0/0.
- `bytes.buffer as ArrayBuffer` → передавать `Uint8Array` в `Blob`/`fetch`
  (проверено `tsc` 6.0.3: ошибок нет).
- `as SummaryApiError` → `e instanceof SummaryApiError` (тогда импорт должен
  остаться value-импортом; если 2b уже сделал `import type` — вернуть value).
- `as { code?: string }` → `isRecord(e) && typeof e.code === 'string'`.
- `as T` в `summaryApi.ts` — оставить 1–2 boundary-каста с
  `// SAFETY: ответ соответствует DTO, проверен Rust-тестом ...`.
- `as unknown as` в `export.ts` — либо `declare global { interface Window { ... } }`
  (единственное допустимое `interface` — declaration merging; правило
  `useConsistentTypeDefinitions` на нём придётся подавить
  `// biome-ignore lint/style/useConsistentTypeDefinitions: declaration merging`),
  либо локальный type guard с `// SAFETY:`.

Цель по `as`: ≤10 не-`as const` кастов, каждый с `// SAFETY:` на русском.
`as const` (2) не трогаем — это не каст, а const assertion.

#### 4d. Оставшиеся warning и правило `any`

- `noGlobalIsNan` → `Number.isNaN`, `noGlobalIsFinite` → `Number.isFinite`
  (аргументы уже числа; поведение для NaN/Infinity сохраняется).
- `useOptionalChain` → `?.` (5 мест; проверено, семантика та же).
- `noNonNullAssertion` в `main.ts` → явная проверка `#app`:
  `const target = document.getElementById('app'); if (!target) throw new Error(...)`.
- `noUnusedFunctionParameters` в `summaryResponse` (`summary.dev.ts`) → `_file`
  (Biome игнорирует `_`-префикс), вызов не меняется.
- `useImportType` в `summaryLoad.ts` — если 2b ещё не исправил: `import type`
  либо `instanceof SummaryApiError` (см. 4c).
- Добавить `noExplicitAny: "error"` в конфиг.
- Проверить: `yarn lint` → 0 error, 0 warning; остаются только info `useTemplate`.

#### 4e. Опционально: `useTemplate`

59 info не блокируют CI. Если нужен чистый вывод:

```bash
yarn biome lint --write --unsafe --only=style/useTemplate src
```

Проверить diff: правило меняет `'a' + b` на `` `a${b}` ``; для строк/чисел
безопасно, для объектов — потенциально иное приведение. После 4a–4c большая
часть уйдёт сама. Если сомневаетесь — оставить info.

Критерий этапа 4: `any` = 0; `as Error` = 0; `as` ≤10 с `SAFETY`; `yarn lint`
0 error/0 warning; `yarn check` 0/0; stamp.

Откат: подкоммиты откатываются по одному; `readJsonSafe` — самый рискованный,
его revert возвращает `any` и снимает 57 ошибок.

### Этап 5. `function` → `const`-стрелки (4–6 ч)

#### 5a. `.svelte` — 144 top-level + 2 вложенные (21 файл)

Цель: 143 не-export + 1 export (`SlimPlayer.togglePlay`) + 2 вложенные в
`DatePickerPopover.svelte` (`onPointerDown`, `onKey`).

Кодмод (временный, на TypeScript Compiler API, не коммитить):

- для каждого top-level `FunctionDeclaration` заменить
  `[export ][async ]function name<T>(params): Ret {` на
  `[export ]const name = [async ]<T>(params): Ret => {`;
- сохранить JSDoc/лидирующие комментарии (`node.getStart`, а не `node.pos`);
- применить правки с конца файла к началу;
- в `.svelte` — только внутри `<script>`;
- пропустить функции без тела/генераторы/overload (в коде их нет).

Прототип на копии: конвертированы все 336 top-level, `svelte-check` — **0
ошибок/0 предупреждений**, `tsc` — 0, `vite build` — успешно, маркеры
`transcript-grid`/`packsListSettings` в бандле есть. Это подтверждает, что
типовых проблем нет; остаётся runtime-TDZ и smoke.

TDZ-аудит:

- 21 текстовое «упоминание до объявления» — все внутри тел функций, то есть
  исполняются после инициализации модуля. Список — в приложении A.
- 3 цикла: `generateSummary` (самовызов), `startEdit ↔ saveText`,
  `startTicker → loadSummary → applyResponse → startTicker`. Все вызовы
  отложенные; для `const` это безопасно.
- Модульных eager-вызовов нет: `$state(...)` инициализируется литералами;
  `$derived` ленив; `on(...)` в `summaryLoad.ts` стоит после объявлений;
  `onMount`/`$effect` — колбэки.
- `export const togglePlay` в `.svelte` — валидный instance-export Svelte 5;
  проверено `svelte-check` и сборкой. Smoke: `bind:this` + `playerRef.togglePlay()`.

Также в 5a: props, которые не `$bindable`, перевести с `let` на `const`
(10 компонентов; `SlimPlayer` с `$bindable` оставить `let`).

#### 5b. `.ts` — 192 top-level (19 файлов)

Тот же кодмод по `src/lib/*.ts`, `main.ts`, `fixtures/index.ts`. После:
`grep` top-level `function` = 0; `yarn check` 0/0; `yarn build`; smoke.

Ожидаемый diff: 19 файлов, ~1.5k строк (в основном сигнатуры и `=>`).
`app.js` может стать немного меньше (стрелки), пересобрать и закоммитить.

Риски:

- `this`/`arguments`/`new`/`prototype`: в коде не используются (проверено).
- Имя функции в стеке меняется (`const`-стрелка может быть анонимной в
  минифицированном бандле) — на поведение не влияет, но ошибки в консоли
  могут выглядеть иначе. Приемлемо.
- Функции, вызываемые до объявления на этапе инициализации, отсутствуют; если
  появится новая — держать её `function` или поднять объявление выше.

Критерий: `function` = 0, включая 2 вложенные (они конвертируются в 5a);
`yarn check`; `yarn build`; smoke; stamp.

Откат: revert 5a/5b; вернуть `function` можно обратным кодмодом, но проще
revert.

### Этап 6. Документация и CI (1–2 ч)

Файлы:

- `ui/web/frontend/AGENTS.md`:
  - команды `yarn lint`, `yarn lint:fix`, `yarn format`;
  - правила: `type`, `const`-стрелки, `import type`, `any`/`as`, runes-only,
    kebab-case (если принят), Biome форматирует только `<script>`;
  - «перед коммитом: `yarn lint && yarn check && yarn build`».
- корневой `AGENTS.md`: в блок «Команды» добавить `yarn lint` перед `yarn check`;
  в правила — ссылку на фронтенд-AGENTS и краткую строку про `type`/`const`/`any`.
- `docs/ai/workflow.md`: шаг 4 → `yarn lint && yarn check && yarn build`.
- `CONTRIBUTING.md`, `README.md`, `ui/web/frontend/README.md`,
  `.github/pull_request_template.md` — добавить `yarn lint`.
- `.github/workflows/ci.yml`, job `frontend`:

```yaml
      - name: Lint frontend (Biome)
        working-directory: ui/web/frontend
        run: yarn lint
```

поставить после `yarn install --immutable` и перед `yarn check`.

- `docs/RELEASING.md` — добавить `yarn lint` в предполётный список (опционально).
- `release.yml` не трогать: релиз собирается после CI, лишний гейт замедлит хотфиксы.

Pre-commit: не добавляем. В репозитории нет ни lefthook, ни husky, ни
`core.hooksPath`; lefthook потребует новую зависимость и установку хука в каждом
клоне, а пути staged-файлов из корня не совпадут с cwd `ui/web/frontend`.
Достаточно CI + `yarn lint:fix` локально. Если хук нужен — простой
`.git/hooks/pre-commit` с `cd ui/web/frontend && yarn lint` (медленнее, но
без зависимостей), не lefthook.

Критерий: `yarn lint` в CI зелёный; документация не противоречит `biome.json`;
P11-аудит чистый. Откат: revert docs/CI-коммита; исходники не менялись.

### Этап 7. Kebab-case (опционально, 2–4 ч)

Решение пользователя. Если да:

- переименовать 24 `.svelte` (`SlimPlayer.svelte` → `slim-player.svelte`,
  `App.svelte` → `app.svelte` или оставить `App.svelte` как entry) и 6 модулей
  (`screenRecorder.svelte.ts` → `screen-recorder.svelte.ts`,
  `summaryActions.ts` → `summary-actions.ts`, `summaryApi.ts`, `summaryLoad.ts`,
  `summaryState.svelte.ts`, `summaryUi.ts`);
- обновить импорты (включая `main.ts`, `App.svelte`, все компоненты);
- включить `style/useFilenamingConvention` с `filenameCases: ["kebab-case"]`
  (правило шумит дублями диагностик — включать только после переименования);
- `yarn check`, `yarn build`, smoke; stamp.

Почему не сейчас: это отдельный шумный diff на 30+ файлов, не влияющий на
поведение; PascalCase компонентов — распространённая конвенция Svelte и текущая
документация репозитория. Kebab-case для `.ts`-модулей можно сделать раньше,
если хочется, но без обязательного правила в Biome. Откат: revert коммита
переименования (git корректно отследит rename при `git mv`).

---

## 6. Примеры «до/после» из текущего репозитория

### 6.1 Props: inline-тип → `type Props`

`src/components/UpdateModal.svelte`

```ts
// было
let { recording }: { recording?: boolean } = $props()

// стало
type Props = { recording?: boolean }
const { recording }: Props = $props()
```

### 6.2 `interface` → `type` с `extends`

`src/lib/api.ts`

```ts
// было
export interface SummaryModelInfo extends ModelInfo {
  kind: 'summary'
}

// стало
export type SummaryModelInfo = ModelInfo & {
  kind: 'summary'
}
```

### 6.3 `function` → `const`-стрелка

`src/lib/api.ts`

```ts
// было
export function fmtClock(sec: number): string {
  if (!isFinite(sec) || sec < 0) sec = 0
  const h = Math.floor(sec / 3600)
  // ...
}

// стало (после этапа 5; до него — function)
export const fmtClock = (sec: number): string => {
  const total = Number.isFinite(sec) && sec > 0 ? sec : 0
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = Math.floor(total % 60)
  // ...
}
```

`src/lib/summaryActions.ts`

```ts
// было
export async function generateSummary(
  presetId: string,
  opts: { force?: boolean; confirmSensitive?: boolean; replace?: boolean; allowDowngrade?: boolean } = {},
): Promise<void> { /* ... */ }

// стало
export const generateSummary = async (
  presetId: string,
  opts: { force?: boolean; confirmSensitive?: boolean; replace?: boolean; allowDowngrade?: boolean } = {},
): Promise<void> => { /* ... */ }
```

### 6.4 `import type`

`src/lib/summaryLoad.ts`

```ts
// было: SummaryApiError используется только в type-position
import { apiJson, isMockMode, SummaryApiError } from './summaryApi.ts'

// вариант A — оставить value-импорт, но использовать instanceof
if (e instanceof SummaryApiError) { /* e.code */ }

// вариант B — если остаётся только тип
import { apiJson, isMockMode } from './summaryApi.ts'
import type { SummaryApiError } from './summaryApi.ts'
```

### 6.5 `any` → `unknown` + generic

`src/lib/api.ts`

```ts
// было
export async function readJsonSafe(res: Response): Promise<any> { /* ... */ }

// стало
export const readJsonSafe = async <T = Record<string, unknown>>(
  res: Response,
): Promise<T> => {
  const ct = res.headers.get('content-type') || ''
  if (ct.includes('application/json')) return (await res.json()) as T // SAFETY: контракт endpoint
  const text = await res.text()
  throw new Error(`Сервер вернул HTTP ${res.status}: ${text.slice(0, 160)}`)
}
```

`src/lib/update.svelte.ts`

```ts
// было
function failApply(d: any): void { /* ... */ }

// стало
const failApply = (d: Record<string, unknown>): void => { /* ... */ }
```

### 6.6 `as Error` → `errText`

`src/components/People.svelte`

```ts
// было
toast('Не удалось сохранить участника: ' + (e as Error).message, 'error')

// стало
toast(`Не удалось сохранить участника: ${errText(e)}`, 'error')
```

### 6.7 `as Node` → `instanceof` с сохранением семантики

`src/components/DatePickerPopover.svelte`

```ts
// было: contains(null) === false, поэтому клик вне закрывает
if (rootEl && !rootEl.contains(e.target as Node)) { onClose() }

// стало: если target === null, условие по-прежнему истинно
if (rootEl && !(e.target instanceof Node && rootEl.contains(e.target))) { onClose() }
```

### 6.8 `as Block` → discriminated union

`src/components/TranscriptDoc.svelte`

```ts
// было
interface Block { kind: 'utt'; search: string; index: number; /* ... */ }
interface Raw { kind: 'h1' | 'h2' | 'h3' | 'p' | 'hr' | 'ul'; /* ... */ }
// ...
{#if b.kind === 'utt' && (b as Block).search.includes(q)}

// стало
type Block = { kind: 'utt'; search: string; index: number; /* ... */ }
type Raw = { kind: 'h1' | 'h2' | 'h3' | 'p' | 'hr' | 'ul'; /* ... */ }
// ...
{#if b.kind === 'utt' && b.search.includes(q)}
```

Проверено на копии: удаление всех `(b as Block)`/`(b as Raw)` (31 шаблонный
каст) даёт `svelte-check` 0/0.

### 6.9 `as ArrayBuffer` → `Uint8Array`

`src/lib/export.ts`

```ts
// было
new Blob([bytes.buffer as ArrayBuffer], { type: type.mime })
body: bytes.buffer as ArrayBuffer

// стало
new Blob([bytes], { type: type.mime })
body: bytes
```

`tsc` 6.0.3 принимает `Uint8Array` как `BodyInit`/`BlobPart`; проверено.

---

## 7. Верификационная матрица

| Проверка | Команда | Что ловит | Когда |
|---|---|---|---|
| Biome | `cd ui/web/frontend && yarn lint` | формат, линт, organize imports; 0 error | каждый этап после 2b |
| Типы | `yarn check` | svelte-check 0/0, `tsc`, fixtures | каждый этап |
| Сборка | `yarn build` | Vite, детерминированные имена | каждый этап, меняющий `src` |
| Статика | `git diff --exit-code -- ui/web/static` после build+stamp | незакоммиченный бандл | каждый этап |
| Stamp | команда из 3.4 | хеш `src/index.html/package.json/yarn.lock/vite.config.ts` | каждый этап |
| Rust-тесты | `cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml` | маркеры `transcript-grid`, `packsListSettings`, отдача static | после build |
| Clippy | `cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings` | 0 предупреждений | после build |
| Clippy Tauri | `cargo clippy -p mr-app --features tauri-ui --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings` | нативная оболочка | после build |
| P11 | `./scripts/p11-diff.sh` | комментарии только «зачем» | перед коммитом |
| Метрики | см. ниже | `interface`/`function`/`any`/`as` | этапы 3–5 |
| Smoke | `./run.sh --web` | вкладки, модалки, плеер, запись, расшифровка, итоги | этапы 2b, 4, 5 |

Grep-метрики (ожидаемые значения после этапов):

```bash
cd ui/web/frontend
# interface: 0 (кроме допустимого declaration merging для Window, если появится)
rg -n "^\s*(export\s+)?interface\s" src -g '*.{ts,svelte}'
# top-level function: 0
rg -n "^\s*(export\s+)?(async\s+)?function\s" src -g '*.{ts,svelte}'
# any: 0
rg -n ":\s*any\b|\bas\s+any\b|<any>" src -g '*.{ts,svelte}'
# as (приблизительно; исключить as const и {#each ... as x})
rg -n "\bas\s+[A-Z{]" src -g '*.{ts,svelte}' | rg -v "as const"
# legacy Svelte: 0
rg -n "export let|\$:|on:click" src -g '*.svelte'
# console: 0
rg -n "console\." src -g '*.{ts,svelte}'
```

`rg "\bas\s+[A-Z{]"` — приблизительная метрика: она ловит `as Block`,
`as HTMLElement`, `as { ... }`, но не ловит `as string` (lowercase). Для
точного аудита использовать AST-скрипт из исследования или ревью. Цель —
≤10 не-`as const` кастов, каждый с `// SAFETY:`.

Ручной smoke (обязателен после этапов 2b, 4, 5):

1. `./run.sh --web` (или `./run.sh --build`), открыть `http://127.0.0.1:8765`.
2. Вкладки «Запись», «Очередь», «Участники», «Словарь», «Настройки».
3. Запись 10–20 с (или загрузка файла), пауза, переименование, удаление.
4. Страница расшифровки: плеер, перемотка, спикеры, правка реплики, итоги.
5. Модалки: настройки, обновление, донат, выбор пресета итогов; Esc/клик по фону.
6. Тосты, `CrashBanner` (если есть тестовый сценарий), мобильная ширина.
7. DevTools: нет новых ошибок/предупреждений.

P12: в описании smoke — только «моно/стерео, минуты, число спикеров», без
имён и названий файлов.

---

## 8. Риски и меры

| Риск | Влияние | Мера |
|---|---|---|
| Большой diff форматирования | Ревью тонет, конфликты с текущей веткой | Отдельный коммит 2a; `git diff -w`; не смешивать с логикой; ветка от чистого HEAD |
| Biome не форматирует разметку/`<style>` | Единый стиль только в `<script>` | Зафиксировать в AGENTS; разметку проверяет svelte-check; не включать full-support |
| `useConst`/`useImportType`/`noUnused*` отключены в `.svelte` | Пропущенные `let`/импорты | `verbatimModuleSyntax` ловит `import type`; props `const` — в 5a; ревью |
| `noUnusedVariables` без override даёт 320 ложных срабатываний | Нельзя включить «для порядка» | Оставить override; использовать svelte-check |
| TDZ при `function` → `const` | Runtime `ReferenceError` | 21 раннее упоминание проверено; циклы отложенные; prototype check/build; smoke |
| `export const` в `.svelte` | Instance-export может не работать | Проверено svelte-check/сборкой; smoke `playerRef.togglePlay()` |
| Organize imports меняет порядок инициализации | Side-effect'ы модулей | В коде только `on(...)` в конце `summaryLoad.ts`; smoke; при сомнении — отдельный коммит 2b |
| `readJsonSafe<T>` вскрывает 57 ошибок | Этап 4b затягивается | Response-типы по endpoint'ам; `T = Record<string, unknown>`; строгий `unknown` — отдельная фаза |
| `as` → guards меняет поведение на `null` | Клики/фокус | Пример 6.7: сохранять `contains(null) === false`; svelte-check + smoke |
| `yarn.lock`/stamp | CI «Static build is committed» падает | `yarn install --immutable`; build+stamp по 3.4; коммитить вместе |
| `yarn build` удаляет `.mr-stamp` | Забытый stamp | Всегда писать stamp после build; проверять `git diff --exit-code -- ui/web/static` |
| Версия Yarn/Node | Lock перегенерирован не той версией | `corepack enable`; `packageManager: yarn@4.18.0`; Node 22 |
| Платформенные бинарники Biome | macOS/Windows dev | `@biomejs/biome` с optional platform packages; lock общий; CI ubuntu |
| `--unsafe` применяет лишнее | Случайные правки | Всегда `--only=<rule>`; иначе не использовать |
| `useTemplate` unsafe-фикс | Иное приведение `+` | Оставить info или отдельный 4e с ревью |
| `noImportantStyles` off | Потеря предупреждений о каскаде | Осознанные `!important`; зафиксировать в AGENTS |
| Kebab-case | 30+ переименований, конфликты | Этап 7, только по решению; без правила в Biome до переименования |
| Текущая ветка с Rust-правками | Смешение задач | Отдельный worktree/ветка; не stash-ить чужие изменения |

---

## 9. Трудоёмкость и стратегия

Оценка — для одного разработчика с кодмодом; в скобках — «механическая» часть,
которую можно делегировать агенту.

| Этап | Файлов | Ожидаемый diff | Часы |
|---|---:|---|---:|
| 0. Baseline | 0 | 0 | 0.5 |
| 1. Toolchain | 3–4 | ~150 строк | 1–2 |
| 2. Формат + импорты + 4 error | 55 | ~4.7k/3.7k (минимальный стиль) | 2–4 |
| 3. `interface` → `type` | 19 | ~400/300 | 1–2 |
| 4. Типы: `errText`, `readJsonSafe`, `as` | ~30 | ~500–800 | 6–10 |
| 5. `function` → `const` | 40 | ~1.5k | 4–6 |
| 6. Docs + CI | 8 | ~100 | 1–2 |
| 7. Kebab-case (опция) | 30+ | ~200 + переименования | 2–4 |
| **Итого** | | | **16–28 ч** |

Стратегия ветки: одна ветка `chore/frontend-biome-style` от согласованной базы
(текущий HEAD `6e0c9d0` или `main` после слияния диаризации), серия коммитов:

```text
chore(frontend): добавить Biome и скрипты проверки
style(frontend): форматирование Biome и organize imports
refactor(frontend): interface → type
refactor(frontend): убрать any и сократить as
refactor(frontend): function → const-стрелки
docs(ci): закрепить стиль фронтенда и добавить yarn lint
refactor(frontend): kebab-case имена (опционально)
```

Каждый коммит, меняющий `src`, сопровождается `yarn build` + stamp + `cargo test`
(или хотя бы `cargo test -p mr-app` для маркеров). Финальный PR — с описанием
«0 поведения, только стиль», ссылкой на этот план и отчётом `yarn lint`.

---

## 10. Принятые решения

- **(а) Стиль:** минимальный diff — одинарные кавычки, `semicolons: "asNeeded"`,
  `lineWidth: 100`. Полный паритет с vpaid переключается одной строкой в `biome.json`.
- **(б) Kebab-case:** выполнен для 24 компонентов и 6 модулей; правило
  `style/useFilenamingConvention` включено.
- **(в) Объём:** весь `src/**/*.{ts,svelte}`, `scripts/*.mjs`, CSS и конфиги.
- **(г) Pre-commit:** только CI (`yarn lint` в job `frontend`); локально — `yarn lint:fix`.
- **(д) `scripts/*.mjs`:** включены.
- **(е) CSS:** форматируется и линтуется; `noImportantStyles` выключен с обоснованием.
- **(ж) `useTemplate`:** исправлен unsafe-фиксом с ревью diff; все 59 info закрыты.

## 11. Чек-лист приёмки

- [ ] `cd ui/web/frontend && yarn lint` — exit 0, 0 error, 0 warning
      (допустимы 0 info или отдельно согласованные `useTemplate`).
- [ ] `yarn check` — 0 ошибок, 0 предупреждений.
- [ ] `yarn build` — успешно; `ui/web/static` закоммичен.
- [ ] `.mr-stamp` равен хешу CI (команда 3.4); `git diff --exit-code -- ui/web/static`
      после build+stamp — чисто.
- [ ] `cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml` — зелёный
      (маркеры `transcript-grid`, `packsListSettings`).
- [ ] `cargo clippy ... -- -D warnings` — 0; `mr-app --features tauri-ui` — 0.
- [ ] `./scripts/p11-diff.sh` — только «зачем»/`SAFETY:`/`shortcut:`.
- [ ] `interface` = 0 (кроме допустимого declaration merging, если появится).
- [ ] top-level `function` = 0 в `.ts` и `.svelte` (2 вложенные тоже).
- [ ] `any` = 0; `as Error` = 0; не-`as const` кастов ≤10, каждый с `// SAFETY:`.
- [ ] `export let`/`$:`/`on:click` = 0; `console.` = 0.
- [ ] Нет `@ts-ignore`/`@ts-expect-error`; non-null `!` = 0.
- [ ] Имена файлов не менялись (или этап 7 согласован отдельно).
- [ ] Smoke: вкладки, запись/загрузка, расшифровка, спикеры, итоги, модалки,
      плеер, тосты; в DevTools нет новых ошибок.
- [ ] В diff/PR нет личных данных (P12): только обезличенные параметры.
- [ ] `docs/ai/svelte-code-style-plan.md` и обновлённые AGENTS/CI в коммите.

---

## Приложение A. Ранние упоминания и циклы (для этапа 5)

Номера строк — до этапа 2 (после форматирования они сдвинутся; искать по имени
функции). Все ранние упоминания — внутри тел функций; исполняются после инициализации
модуля. После конверсии в `const` TDZ не возникает, но места стоит просмотреть
глазами:

| Файл | Функция | Объявлена | Первое упоминание |
|---|---|---:|---:|
| `People.svelte` | `stopVoice` | 139 | 92, в `closeForm` |
| `SlimPlayer.svelte` | `toggle` | 73 | 31, в `togglePlay` |
| `SummaryPromptModal.svelte` | `closeSelect` | 101 | 77, в `onWindowKey` |
| `SummarySettingsTab.svelte` | `startGpuPoll` | 123 | 108, в `reload` |
| `SummarySettingsTab.svelte` | `save` | 187 | 184, в `toggleGpu` |
| `TranscriptDoc.svelte` | `scrollActive` | 91 | 88, в `$effect` |
| `TranscriptDoc.svelte` | `saveText` | 149 | 115, в `startEdit` |
| `TranscriptDoc.svelte` | `openWordMenu` | 229 | 225, в `onUttTextClick` |
| `settings.svelte.ts` | `setRecordMode` | 104 | 91, в `loadRecordMode` |
| `summaryActions.ts` | `generateSummary` | 133 | 109, в `pickPreset` |
| `summaryActions.ts` | `generateSummary` | 133 | 118, в `confirmSensitive` |
| `summaryActions.ts` | `rebuildInOldFormat` | 295 | 264, в `retrySummary` |
| `summaryLoad.ts` | `loadSummary` | 165 | 32, в `setInterval` |
| `summaryLoad.ts` | `sameRecording` | 239 | 107, в `applyResponse` |
| `summaryLoad.ts` | `loadSummaryPresets` | 212 | 184, в `loadSummary` |
| `toast.svelte.ts` | `dismissToast` | 46 | 21/34/37, в `setTimeout` |
| `transcript.svelte.ts` | `handleEngineEvent` | 143 | 118, в `onmessage`-стрелке |
| `transcript.svelte.ts` | `finishEngine` | 227 | 203, в `handleEngineEvent` |
| `update.svelte.ts` | `pollProgress` | 183 | 169, в `setInterval` |

Циклы вызовов (все — runtime, не инициализация):

- `summaryActions.ts`: `generateSummary → generateSummary`.
- `TranscriptDoc.svelte`: `startEdit → saveText → startEdit`.
- `summaryLoad.ts`: `startTicker → loadSummary → applyResponse → startTicker`.

Модульных eager-вызовов нет: `$state` — литералы, `$derived` ленив,
`on(...)` в `summaryLoad.ts` — после объявлений.

## Приложение B. Распределение объёмов

`interface` по файлам: `api.ts` 35, `system.svelte.ts` 21,
`TranscriptDoc.svelte` 3, `toast.svelte.ts` 2, `queue.svelte.ts` 2,
`SpeakersPane.svelte` 2, остальные 12 файлов — по 1.

`function` в `.svelte`: `TranscriptDoc` 18, `SummarySettingsTab` 16,
`SpeakersPane` 12, `Recordings` 12, `SystemSettingsTab` 11, `TranscriptPage` 10,
`People` 10, `SummaryPromptModal` 9, `DatePickerPopover` 7, `UpdateModal` 6,
`SummaryPanel` 6, `SlimPlayer` 4, `RecordingDeck` 4, `SettingsModal` 3,
`QueueHub` 3, `App` 3, `SummaryStage` 2, `SummaryPresetPicker` 2, `DonateModal` 2,
`Dictionary` 2, `CrashBanner` 2.

`function` в `.ts`: `summaryActions` 23, `settings.svelte.ts` 20, `api.ts` 17,
`update.svelte.ts` 15, `summaryUi` 15, `system.svelte.ts` 14, `summaryLoad` 14,
`export` 12, `transcript.svelte.ts` 11, `summary.dev` 9, `people.svelte.ts` 9,
`ui.svelte.ts` 6, `queue.svelte.ts` 6, `toast.svelte.ts` 4,
`screenRecorder.svelte.ts` 4, `recordings.svelte.ts` 4, `markdown` 4,
`summaryApi` 3, `support.svelte.ts` 2.

## Приложение C. Быстрые команды

```bash
# toolchain
cd ui/web/frontend
yarn add -D @biomejs/biome@^2.4.4
yarn biome --version

# формат / линт
yarn format
yarn lint:fix
yarn lint

# типы и сборка
yarn check
yarn build

# stamp (из корня)
root="$(git rev-parse --show-toplevel)"
expected="$(cd "$root" && find ui/web/frontend/src ui/web/frontend/index.html ui/web/frontend/package.json ui/web/frontend/yarn.lock ui/web/frontend/vite.config.ts -type f -print | LC_ALL=C sort | while IFS= read -r f; do sha256sum "$f"; done | sha256sum | cut -d' ' -f1)"
printf '%s\n' "$expected" > "$root/ui/web/static/.mr-stamp"

# Rust после сборки
cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo clippy -p mr-app --features tauri-ui --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
```
