# E2E quality fix — flat output и русская морфология

Статус: исправлено, проверки зелёные. Полный real-model smoke по договорённости
не запускался — E2E повторит заказчик.

## Что изменилось

- **Deterministic fallback (`extract.rs`):** скан всех реплик по централизованным
  cue-таблицам; решения/задачи/вопросы собираются как точные source-предложения
  с ref, speaker/assignee, deadline. `task_line` требует реального
  обязательства/потребности: explicit «я/имя + будущее», `нужно/надо/задача +
  инфинитив`, `беру/отвечаю`; вступления/мнения/предложения (`коллеги`,
  `начинаем`, `я считаю`, `можно`, `цель`, `предлагаю`, `обсудим`) отсекаются.
  Assignee — только известный speaker name или явный `Кто:`; «Цель» и прочие
  заголовочные существительные не становятся исполнителями. Пересекающиеся
  задачи дедуплицируются по stem/noun overlap с приоритетом explicit
  commitment + assignee + deadline (Anna phone > Vera need).
- **Fallback merge:** пункты модели приоритетны, но при совпадении задачи
  fallback с assignee+deadline заменяет более слабый model item; warning
  `fallback_extracted`.
- **Decision/question:** `decision_cue` отвергает `не решили/не согласовали/
  не утвердили/пока не решили`; `question_line` видит `вопрос`, `нужен ли`,
  `стоит ли`, `надо ли`, `?` в любом месте.
- **Prompt:** в `build`/`build_reduce` добавлены «Начни с `## <first>`, не
  пропускай заголовки», one-shot пример и «Кратко — 2–4 пункта, только главное».
  `build_repair` используется только на invalid output.
- **Cap:** сначала по одному пункту из каждой непустой секции, затем
  tasks/decisions extras, questions, brief (не более 4), остальное по evidence.
- **Pipeline policy:** плоский ответ с пунктами не вызывает repair — parser
  раскладывает его сам, fallback добирает задачи/решения/вопросы. Repair
  вызывается только при нуле пунктов/invalid output.
- **Parser:** плоский список без заголовков раскладывается по cue —
  решения/задачи/вопросы/остальное → brief; `parser_recovered` фиксируется.
  Распознаются bare `Кратко:`, `Открытые вопросы:` и другие алиасы каталога.
- **Grounding:** support/dedupe используют стем-токены (5 кириллических /
  4 латинских символа), пороги `intersection>=2`, `ev>=0.25` сохранены.
  «обновить/обновлю», «доделать/доделаю» теперь поддерживаются.
- **Tasks:** cue и дедлайн — по `refs[0]`; дедлайн не берётся из соседней
  реплики. Assignee — только известный speaker name или явный `Кто:`.
- **Brief:** при пустом brief деривация идёт из decisions/tasks, не questions.
- Убран ложный ownership-drop: `доделает` больше не матчится как `делает`.

## Регрессии

- `parse::tests::flat_output_infers_decisions_tasks_questions`
- `parse::tests::bare_colon_headings_are_sections`
- `extract::tests::synthetic_meeting_tightening`
- `extract::tests::task_requires_action_verb_not_just_need`
- `extract::tests::first_person_uses_speaker_unless_placeholder`
- `grounding::tests::synthetic_meeting_fallback_has_no_false_tasks`
- `grounding::tests::explicit_phone_commitment_replaces_need_task`
- `grounding::tests::structured_fallback_fills_missing_tasks_without_duplicates`
- `grounding::tests::tasks_and_decisions_survive_brief_flood_at_normal`
- `grounding::tests::brief_is_truncated_to_four_without_warning`
- `cues::tests::negated_decisions_are_not_decisions`
- `cues::tests::questions_without_trailing_mark_are_questions`
- `cues::tests::requirement_and_risk_lines_are_rejected`
- `extract::tests::fallback_task_text_strips_discourse_markers`
- `extract::tests::object_key_deduplicates_hints_tasks`
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
| `cargo test --workspace --locked ...` | pass: 352 passed, 2 ignored |
| Android `cargo check ... aarch64-linux-android --features tauri-ui --locked` | pass |
| `git diff --check -- src-tauri` | pass |

Коммит: `fix(summary): improve task precision and brief` (этот коммит).
