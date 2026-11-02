# Hotwords через CTC + уверенность слов — план реализации (Track 2)

> Статус: **на согласовании, кода нет**. Изоляция: worktree
> `/home/moiseevov/Code/audio-record-ctc-hotwords`, ветка `feat/ctc-hotwords-confidence`
> (база `fa2fe16`). Основная копия `/home/moiseevov/Code/audio-record` не тронута.
> Это Track 2 по `docs/ai/workflow.md`: новая модель + изменение схемы
> `.transcript.json` + REST. Старт кода — только после Human Approval.

## 1. Цель (из ROADMAP P1)

1. **Hotwords:** второй движок GigaAM v3 CTC рядом с основным RNNT.
   CTC выдаёт logprobs букв/подслов на кадр → beam search даёт бонус словам
   из словаря (аббревиатуры, имена, продукты). RNNT-путь не меняется.
2. **Уверенность:** показатель уверенности каждого слова из CTC-logprobs,
   пометки «проверить вручную» в `.transcript.json`, `.md` и UI.

## 2. Что уже выяснено в коде

- ASR сейчас: `src-tauri/crates/mr-ml/src/onnx/rnnt.rs:99-181` — жадный RNNT
  (encoder + decoder + joint, `BLANK_ID=1024`, vocab 1024 + `▁`-декодинг).
  Вызывается из `src-tauri/crates/mr-app/src/transcribe/mod.rs:341-356`.
  Слов, таймстемпов и logprobs нет — только строка на ASR-окно.
- Координатор `transcribe/mod.rs:214-565` — единственный мост к ML (P3, P8).
  `ModelSet::load()` лениво грузит GigaAM/Whisper через `.ok()`.
- Манифест `src-tauri/crates/mr-models/models-manifest.json` — SSOT (P1):
  сейчас `asr-gigaam-v3-rnnt-int8` (enc/dec/joint + `gigaam_vocab.txt`).
- CTC-веса существуют в том же апстриме `istupakov/gigaam-v3-onnx`
  (проверено через HF API, 2026-02-18):
  `v3_e2e_ctc.int8.onnx` + `v3_e2e_ctc_vocab.txt` (257 токенов, blank=256,
  с пунктуацией — паритет с текущим e2e-RNNT) и `v3_ctc.int8.onnx` +
  `v3_vocab.txt` (34 символа, без пунктуации). Берём **e2e_ctc**.
- UI: карточка «Модели ИИ» — `SettingsModal.svelte:207-305`, DTO —
  `ui/web/frontend/src/lib/api.ts:20-31` (`ModelInfo`), рендер транскрипта —
  `TranscriptDoc.svelte` (сейчас рисует только `u.text`, уверенности нет).
- `AGENTS.md` mr-ml запрещает переписывать Whisper-greedy на beam без задачи,
  но новый CTC beam — отдельная задача, инвариант не нарушаем.

## 3. Два варианта (требование Track 2)

### Вариант A — выбран: отдельный CTC-декодер с три-бонусом (shallow fusion)

- Новый `mr-ml/src/onnx/ctc.rs`: один encoder-ONNX + префиксный beam search
  по CTC-logprobs + бонус за hotwords через префиксное дерево (trie) на
  токенизированных фразах словаря + уверенность как среднее по кадрам слова.
- RNNT-код не трогается. CTC — опциональный второй проход по тем же
  ASR-окнам (до 20 с, `MAX_SEGMENT_SEC`), включается только если модель
  скачана, иначе поведение байт-в-байт как сейчас.
- Плюсы: изоляция риска, P10 (чиним/добавляем на базовом слое `mr-ml`),
  легко откатить (удалил модель — вернулся RNNT), CPU-приемлемо
  (beam 8–16 + словарь ≤200 фраз).
- Минусы: +~110–160 МБ на диске (int8 encoder), второй проход = +время
  на длинных записях (компенсируем кэшем и пропуском при отсутствии словаря).

### Вариант B — отклонён: shallow fusion внутрь RNNT-joint / LM-рескоринг

- Бонус hotwords прямо в `rnnt.rs:164-177` (joint logits) или внешний KenLM.
- Плюсы: один проход.
- Минусы: ломает детерминированный RNNT-путь, риск регрессии русской SOTA,
  нужен LM-файл и его манифест, сложнее тестировать, нарушает «RNNT не меняется».
  Отклоняем.

## 4. Обратная совместимость (P9)

- CLI-флаги, `engine=auto|whisper`, коды HTTP — без изменений.
- `models-manifest.json`: только **добавление** новой записи
  `asr-gigaam-v3-e2e-ctc-int8`, существующие id/файлы не меняем.
- `.transcript.json`: только **аддитивные** поля (старые файлы читаются):
  `utterances[].words[]: {w, conf, needs_review}`, `utterances[].ctc_used: bool`,
  `meta.hotwords_applied: string[]`, `meta.confidence: {engine, threshold}`.
  Старые клиенты игнорируют неизвестные поля.
- REST: только аддитивно — `POST /transcribe` принимает опциональные
  `hotwords: string[]`, `hotwords_weight`, `confidence_threshold`;
  `GET /transcript` отдаёт новые поля как есть. Контракт-тест
  `restored_endpoints_roundtrip` дополняем, не ломаем.
- `.md`: пометка сомнительных слов аддитивна (например `` `слово`⚠ ``
  или суффикс ` [проверить]`), парсер `TranscriptDoc.svelte:269` не ломаем.

## 5. Детальный план по файлам

### Этап 0. Манифест и веса (mr-models)

- `src-tauri/crates/mr-models/models-manifest.json`: новая запись
  `asr-gigaam-v3-e2e-ctc-int8`, файлы `gigaam_ctc_enc_q.onnx`
  (из `v3_e2e_ctc.int8.onnx`), `gigaam_ctc_vocab.txt`
  (из `v3_e2e_ctc_vocab.txt`), `size_bytes`/`sha256` из HF API,
  `optional: true`, описание «второй проход: hotwords + уверенность».
- Тест: `cargo test -p mr-models` + ручная проверка `download_or_update_model`
  (`.part` + SHA-256, докачка `Range` — существующий механизм).

### Этап 1. Ядро CTC (mr-ml, новый файл ≤350 строк)

- Новый `src-tauri/crates/mr-ml/src/onnx/ctc.rs` (~300 строк):
  - `GigaCTC::load(dir)` через существующий `OrtSession::load`
    (`session.rs:38`), инпут как у RNNT (`audio_signal [1,64,T]`,
    `features::nemo_mel` — переиспользуем, без нового ресемплинга).
  - `decode_beam(logprobs [T,V], beam=12, hotwords: &Trie, weight) ->
    Vec<WordHyp {text, conf, start_frame, end_frame}>`:
    префиксный beam по CTC (blank-collapse, `log_softmax` на месте,
    без `ndarray` на горячем пути сверх существующего).
  - Hotwords: нормализация (lowercase, `ё→е` опционально), токенизация
    тем же vocab, trie на id-последовательностях, бонус `+weight`
    за завершённую фразу, штраф за частичное совпадение не начисляем
    (защита от false accept).
  - Confidence: `conf(word) = exp(mean(log p токенов слова))`,
    blank-кадры исключаем; пустые/короткие (<3 кадров) → `conf=0`.
  - Публичные типы в `lib.rs`: `CtcWord {text, confidence}`,
    `CtcResult {text, words}` — математика без диска (инвариант mr-ml).
- Юнит-тесты без моделей (`--lib`): синтетические logprobs
  (argmax-путь, collapse повторов/blank, бонус hotwords перевешивает,
  conf в [0,1], порог `needs_review`).
- Живой тест `onnx_real` (за `MR_MODELS_DIR` + `MR_TEST_WAV`): smoke
  e2e_ctc на коротком окне, сравнение WER RNNT vs CTC не требуем,
  только «не пусто и детерминировано».

### Этап 2. Интеграция в конвейер (mr-app, P3/P8)

- `transcribe/mod.rs`: `ModelSet { ctc: Option<GigaCTC> }`,
  загрузка `.ok()` как у GigaAM (`mod.rs:71-73`) — отсутствие модели
  не ошибка.
- `TranscribeOptions { hotwords: Vec<String>, hotwords_weight: f32,
  confidence_threshold: f32 }` + defaults (`weight=3.0`, `threshold=0.55`,
  пустой словарь = CTC только для уверенности).
- Политика второго прохода (там же, `mod.rs:341-356`): если `ctc.is_some()`
  и (`hotwords` не пуст или нужна уверенность) → после RNNT-текста
  прогоняем то же `chunk` через CTC, берём `words/conf`, текст оставляем
  RNNT (источник истины), hotwords-подмены применяем только при
  `conf_ctc > conf_rnnt_proxy` и совпадении границ окна (иначе только
  помечаем). Whisper-путь не трогаем.
- Память: CTC-сессия в `spawn_blocking`, encoder-кэш как у RNNT
  (`MODELS_CACHE`), лимит `memory::check` без изменений.

### Этап 3. Артефакты (report.rs + mod.rs:536-548)

- `Utterance` += `words: Vec<WordConf {w, conf, needs_review}>`,
  `ctc_used: bool`. `report.rs`: `build_markdown` помечает слова
  с `conf < threshold` (вид пометки — на согласование, дефолт
  `` `слово` [проверить] ``), `build_text` без изменений (чистый текст для LLM).
- JSON пишем новые поля всегда, когда CTC был (даже при пустом словаре —
  уверенность всё равно есть).

### Этап 4. REST + конфиг

- `http/transcribe_api.rs:handle_transcribe_start`: пробросить
  `hotwords/hotwords_weight/confidence_threshold` из body в `TranscribeOptions`
  (валидация: ≤200 фраз, ≤64 символов каждая, как в gigastt `MAX_HOTWORDS`).
- `config.json`: персистентный словарь `hotwords: string[]`
  (редактируется из UI, P9 — новое поле, старый конфиг читается).
- Тест `restored_endpoints_roundtrip`: новые поля в roundtrip.

### Этап 5. UI (Svelte 5, без новых зависимостей)

- Карточка моделей: CTC появляется автоматически из манифеста
  (тот же `SettingsModal.svelte:265-299`, отдельный текст
  «второй проход: hotwords + уверенность, ~150 МБ, опционально»).
- Настройки/старт расшифровки: поле «Словарь терминов» (textarea,
  по одному на строку) + слайдеры веса/порога (дефолты из Этапа 2).
- `TranscriptDoc.svelte`: бейдж `проверить` на словах
  с `needs_review` (подчёркивание + `title="уверенность 0.42"`),
  тултипы на русском, светлая тема, без эмодзи (правила `ui/AGENTS.md`).
  `api.ts`: `Utterance.words?: {w, conf, needs_review}[]`.
- `yarn check` 0/0 + `yarn build`, маркеры `transcript-grid` живы.

### Этап 6. Верификация (чек-лист workflow)

```bash
cargo test --workspace --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --workspace --all-targets --locked --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test -p mr-ml --features onnx --test onnx_real -- --nocapture  # при моделях
cd ui/web/frontend && yarn check && yarn build
```

P12: в тестах/логах только обезличенное («моно, 5 мин, 2 спикера»),
реальные имена/файлы запрещены.

## 6. Открытые вопросы к человеку (ответить до кода)

1. Словарь: глобальный в `config.json` + override на запуск, или только
   на запуск? (предлагаю глобальный + override).
2. Порог «проверить вручную» по умолчанию 0.55 — ок? Диапазон слайдера?
3. Вид пометки в `.md`: `` `слово` [проверить] `` — ок или другой маркер?
4. Выбор CTC в UI: авто (если скачан) или явный чекбокс «второй проход CTC»?
5. Объём словаря: лимит 200 фраз ок?

## 7. Следующий шаг

После ответов — реализую Этапы 0–1 в этом worktree малыми коммитами
(`feat(ml): …`, Conventional Commits), затем 2–5, каждый с прогоном
чек-листа. RNNT-путь и основная ветка не затрагиваются до MR.
