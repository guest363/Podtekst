# E2E quality fix — flat output и русская морфология

Статус: исправлено, проверки зелёные. Полный real-model smoke по договорённости
не запускался — E2E повторит заказчик.

## Что изменилось

- **Prompt:** в `build` добавлены «Начни с `## <first>`, не пропускай заголовки»
  и компактный one-shot пример структуры. Новый `build_repair` переписывает
  плоский ответ в точный контракт секций (те же system-инварианты, DATA в nonce).
  Pipeline вызывает repair один раз, если в ответе нет распознанных заголовков.
- **Parser:** плоский список без заголовков раскладывается по cue —
  решения/задачи/вопросы/остальное → brief; `parser_recovered` фиксируется.
  Распознаются bare `Кратко:`, `Открытые вопросы:` и другие алиасы каталога.
- **Grounding:** support/dedupe используют стем-токены (5 кириллических /
  4 латинских символа), пороги `intersection>=2`, `ev>=0.25` сохранены.
  «обновить/обновлю», «доделать/доделаю» теперь поддерживаются.
- **Tasks:** cue и дедлайн — по `refs[0]`; дедлайн не берётся из соседней
  реплики (иначе `deadline_not_stated`). Расширены формы обязательств
  (`доделает, обновит, проведёт, проверит, отправит, подготовит, сделает,
  добавит, назначит, выдаст, позвонит, напишет, задача, это моя задача`).
  Assignee принимается по speaker name/тексту поддержки и извлекается из
  «Имя + глагол» в тексте задачи.
- **Brief:** при пустом brief деривация идёт из decisions/tasks (не questions),
  как и раньше; плоские tasks/questions не попадают в brief при наличии секций.
- Убран ложный ownership-drop: `доделает` больше не матчится как `делает`.

## Регрессии

- `parse::tests::flat_output_infers_decisions_tasks_questions`
- `parse::tests::bare_colon_headings_are_sections`
- `prompt::tests::prompt_starts_with_first_title_instruction`
- `prompt::tests::repair_prompt_wraps_flat_output`
- `grounding::tests::russian_morphology_supports_task_stems`
- `grounding::tests::task_assignee_from_speaker_and_text`
- `grounding::tests::task_assignee_from_inverted_name_and_verb`
- `grounding::tests::adjacent_unrelated_deadline_is_not_attached`
- `deadline::tests::supplied_ref_order_wins_over_utterance_index`
- `pipeline::tests::fake_flat_output_is_repaired_and_recovered`

## Проверки

| Команда | Результат |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked ... -D warnings` | pass |
| `cargo clippy -p mr-app --features tauri-ui --all-targets --locked ... -D warnings` | pass |
| `cargo test --workspace --locked ...` | pass: 338 passed, 2 ignored |
| Android `cargo check ... aarch64-linux-android --features tauri-ui --locked` | pass |
| `git diff --check -- src-tauri` | pass |

Коммит: `fix(summary): recover flat output and Russian morphology`.
