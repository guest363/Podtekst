# E2E quality fix — flat output и русская морфология

Статус: исправлено, проверки зелёные. Полный real-model smoke по договорённости
не запускался — E2E повторит заказчик.

## Что изменилось

- **Deterministic fallback (`extract.rs`):** скан всех реплик по централизованным
  cue-таблицам; решения/задачи/вопросы собираются как точные source-предложения
  с ref, speaker/assignee, deadline из того же экстрактора. «Нужно» без
  действия задачей не считается. Fallback добавляется в `ground()` только для
  отсутствующих пунктов; модель имеет приоритет, дубли отсекаются по ref и
  stem-overlap. Warning `fallback_extracted`; cap — `detail.max_items()`.
- **Pipeline policy:** плоский ответ с пунктами больше не вызывает repair —
  parser раскладывает его сам, fallback добирает задачи/решения/вопросы.
  Repair вызывается только при нуле пунктов/invalid output. Это убирает
  дополнительный вызов модели (60–140 с в E2E).
- **Prompt:** в `build`/`build_reduce` добавлены «Начни с `## <first>`, не
  пропускай заголовки», компактный one-shot пример структуры и ограничение
  «Кратко — 2–4 пункта, только главное; не дублируй задачи/решения». Новый
  `build_repair` переписывает плоский/пустой ответ в точный контракт секций
  (те же system-инварианты, DATA в nonce) и используется только на invalid output.
- **Cap:** приоритет `tasks/next_steps/todo` → `decisions` → `questions` →
  `brief` → остальные по evidence; brief принудительно обрезается до 4 пунктов
  до глобального cap, чтобы 13 brief-пунктов не вытесняли задачи.
- **Parser:** плоский список без заголовков раскладывается по cue —
  решения/задачи/вопросы/остальное → brief; `parser_recovered` фиксируется.
  Распознаются bare `Кратко:`, `Открытые вопросы:` и другие алиасы каталога.
- **Grounding:** support/dedupe используют стем-токены (5 кириллических /
  4 латинских символа), пороги `intersection>=2`, `ev>=0.25` сохранены.
  «обновить/обновлю», «доделать/доделаю» теперь поддерживаются.
- **Tasks:** cue и дедлайн — по `refs[0]`; дедлайн не берётся из соседней
  реплики (иначе `deadline_not_stated`). Расширены формы обязательств,
  assignee — по speaker name/тексту/«Имя + глагол».
- **Brief:** при пустом brief деривация идёт из decisions/tasks, не questions.
- Убран ложный ownership-drop: `доделает` больше не матчится как `делает`.

## Регрессии

- `parse::tests::flat_output_infers_decisions_tasks_questions`
- `parse::tests::bare_colon_headings_are_sections`
- `extract::tests::extracts_tasks_assignees_and_deadlines`
- `extract::tests::task_requires_action_verb_not_just_need`
- `extract::tests::first_person_uses_speaker_unless_placeholder`
- `grounding::tests::structured_fallback_fills_missing_tasks_without_duplicates`
- `grounding::tests::tasks_and_decisions_survive_brief_flood_at_normal`
- `grounding::tests::brief_is_truncated_to_four_without_warning`
- `pipeline::tests::fake_flat_output_is_recovered_without_repair` (1 вызов, без repair)
- `prompt::tests::prompt_starts_with_first_title_instruction`
- `prompt::tests::repair_prompt_wraps_flat_output`
- `grounding::tests::russian_morphology_supports_task_stems`
- `grounding::tests::task_assignee_from_speaker_and_text`
- `grounding::tests::task_assignee_from_inverted_name_and_verb`
- `grounding::tests::adjacent_unrelated_deadline_is_not_attached`
- `deadline::tests::supplied_ref_order_wins_over_utterance_index`

## Проверки

| Команда | Результат |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked ... -D warnings` | pass |
| `cargo clippy -p mr-app --features tauri-ui --all-targets --locked ... -D warnings` | pass |
| `cargo test --workspace --locked ...` | pass: 344 passed, 2 ignored |
| Android `cargo check ... aarch64-linux-android --features tauri-ui --locked` | pass |
| `git diff --check -- src-tauri` | pass |

Коммит: `fix(summary): prioritize tasks and cap brief` (этот коммит).
