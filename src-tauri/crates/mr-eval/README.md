# mr-eval — метрики словаря

Отдельная Rust-утилита для проверки словаря и hotwords на локальном корпусе. В релизный гейт не входит: `small`/`full` и `baseline.json` не затрагиваются.

## Сборка

```bash
cargo build --release -p mr-eval --manifest-path src-tauri/Cargo.toml
```

Бинарник: `src-tauri/target/release/podtekst-eval` (`podtekst-eval.exe` на Windows).

## Манифест

```json
{
  "name": "hotwords-ru-synth",
  "language": "ru",
  "engine": "gigaam",
  "vocabulary": ["альфа", "бета", "юзер флоу => User Flow"],
  "records": [
    {
      "id": "term-01",
      "audio": "audio/term-01.wav",
      "ref_text": "ref/term-01.txt",
      "group": "term",
      "expected_terms": ["альфа"],
      "language": "ru",
      "engine": "gigaam"
    },
    {
      "id": "ordinary-01",
      "audio": "audio/ordinary-01.wav",
      "ref_text": "ref/ordinary-01.txt",
      "group": "ordinary",
      "expected_terms": []
    }
  ]
}
```

- `group` — `term`, `lookalike` или `ordinary`.
- `expected_terms` перечисляет канонические термины по одному разу на каждое ожидаемое вхождение и сверяется с `ref_text`; расхождение разметки — ошибка.
- `lookalike` и `ordinary` не должны содержать терминов в эталоне.
- Для `term` допускается `reference` вместо `ref_text`.

Аудио и эталоны — синтетические. Реальные записи и имена в репозиторий не попадают; запуски лежат в `runs/` (gitignore).

## Прогон

Модели должны быть в `<data-dir>/models`, как у приложения. `data-dir` — отдельный каталог с junction/symlink на модели: живой профиль приложения (с `people.json`/`voiceprints.json`) утилита не принимает. `config.json` сохраняется и восстанавливается при штатном завершении; Ctrl-C оставляет его изменённым, недостающие ключи не теряются.

```bash
src-tauri/target/release/podtekst-eval hotwords run \
  --manifest ~/.cache/podtekst-bench/runs/hotwords/manifest.json \
  --data-dir ~/.cache/podtekst-bench/runs/hotwords/appdata \
  --out ~/.cache/podtekst-bench/runs/hotwords/run \
  --variants off,on,ctc-off,on-b2.5,on-c0.4
```

Варианты:

- `off` — пустой словарь, CTC включён;
- `on` — словарь, CTC включён;
- `ctc-off` — словарь без CTC, работает только пост-замена;
- `on-bX` — свип бонуса CTC (0–10) при `MR_HOTWORD_BENCH=1`;
- `on-cX` — свип порога уверенности (0–1) при `MR_HOTWORD_BENCH=1`.

Каждый клип запускается отдельным процессом приложения с изолированным `PODTEKST_DATA_DIR` и `MEETING_RECORDER_DATA_DIR`; аудио копируется в каталог варианта. `raw.json` хранит гипотезу и эталон для повторного скоринга.

## Метрики

```bash
src-tauri/target/release/podtekst-eval hotwords score \
  --manifest ~/.cache/podtekst-bench/runs/hotwords/manifest.json \
  --run ~/.cache/podtekst-bench/runs/hotwords/run
```

Пишет `metrics.csv`, `summary.csv`, `failures.csv` и `report.md`.

- **Recall терминов** — доля ожидаемых вхождений, найденных в гипотезе.
- **Precision терминов** — найденные вхождения минус лишние.
- **False insertions** — термин в гипотезе сверх ожидаемого; отдельно для `ordinary` и `lookalike`.
- **WER** — обычный WER по нормализованным словам.
- **WER term-norm** — термины и алиасы сводятся к одному маркеру; метрика сравнима только сама с собой.
- **WER вне терминов** — термины и алиасы удалены; показывает сопутствующий ущерб.

Абсолютные значения на TTS-корпусе нельзя сравнивать с публичными наборами; смотрите на прирост `on` относительно `off`, false insertions и WER вне терминов.

CTC умеет только перерешивать слова, уже попавшие в гипотезу RNNT, — вставку новых слов он не делает. Это ограничение recall, а не ошибка утилиты.
