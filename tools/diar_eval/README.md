# Офлайн-оценка Podtekst на открытых датасетах

Dev-харнесс: запускает release-сборку на файлах из `manifest.json`, считает
WER/CER, DER, SCA, cpWER и attribution accuracy, собирает `metrics.csv` и
`report.md`. В рантайм приложения Python не попадает.

## 1. Окружение

```bash
python3.13 -m venv ~/.cache/podtekst-bench/venv
uv pip install --python ~/.cache/podtekst-bench/venv/bin/python -r tools/diar_eval/requirements-eval.txt

cargo build --release -p mr-app
```

Бинарник по умолчанию: `src-tauri/target/release/podtekst`. Другой путь - `--app`.

Проверка самого харнесса (из корня репозитория):

```bash
PY=~/.cache/podtekst-bench/venv/bin/python
$PY tools/diar_eval/transcript_to_rttm.py --self-test
$PY tools/diar_eval/evaluate.py --self-test
$PY tools/diar_eval/metrics.py --self-test
$PY tools/diar_eval/synth_meetings.py --self-test
$PY tools/diar_eval/normalize_wer.py --self-test
$PY tools/diar_eval/run_batch.py --self-test
$PY tools/diar_eval/report.py --self-test
```

## 2. RTTM из транскрипта

```bash
$PY tools/diar_eval/transcript_to_rttm.py \
  path/to/meeting.transcript.json path/to/meeting.rttm --id meeting
```

- `--id` должен совпадать с file id в эталонном RTTM. Если не задан, берётся
  имя файла без `.transcript.json`.
- Спикеры нумеруются `SPEAKER_01`, `SPEAKER_02`, ... по убыванию суммарного
  времени речи. Имена из UI для DER не важны.
- Некорректные сегменты (нет времени, `end <= start`, NaN) пропускаются,
  число пропущенных печатается в stderr.

## 3. DER

```bash
$PY tools/diar_eval/evaluate.py reference.rttm hypothesis.rttm --per-file
$PY tools/diar_eval/evaluate.py ref_dir/ hyp_dir/ --collar 0.25
$PY tools/diar_eval/evaluate.py ref.rttm hyp.rttm --skip-overlap
```

`reference` и `hypothesis` - файл или каталог с `*.rttm`. Считается
`DiarizationErrorRate` из `pyannote.metrics`; файлы сопоставляются по file id.
Режимы из ТЗ: `collar=0.0` (основной), `collar=0.25` (сравнение с
публикациями), `--skip-overlap` (ошибки на перекрытиях отдельно).

## 4. Manifest

`run_batch.py` читает JSON-объект с массивом `records` (допустим и просто
массив). Пути относительны `--data-root`; по умолчанию это каталог манифеста.

```json
{
  "records": [
    {
      "id": "ami-ihm/test/ES2002a",
      "audio": "ami-ihm/audio/ES2002a.wav",
      "dataset": "ami-ihm",
      "subset": "test",
      "ref_rttm": "ami-ihm/ref/ES2002a.rttm",
      "ref_text": "ami-ihm/ref/ES2002a.transcript.json",
      "language": "en",
      "speakers": 4,
      "duration": 3600.0
    }
  ]
}
```

Поля:

| Поле | Обязательно | Смысл |
|---|---|---|
| `id` | нет | Уникальный id; по умолчанию `dataset/subset/<stem>` |
| `audio` | да | WAV/медиафайл |
| `dataset`, `subset` | нет | Группировка в отчёте |
| `ref_rttm` | для DER | Эталонная разметка |
| `ref_text` | для WER/cpWER | `.transcript.json`, `.jsonl` или `.txt` |
| `language` | нет | Передаётся в `--language`, по умолчанию `auto` |
| `speakers` | нет | Число спикеров эталона; иначе считается по RTTM |
| `duration` | нет | Длительность, с; иначе берётся из события `done` или аудио |
| `engine` | нет | `gigaam`, `whisper`, `auto`; CLI `--engine` важнее |
| `transcript` | нет | Путь к JSON, если он не рядом с аудио |
| `rttm_id` | нет | File id для RTTM, если в эталоне несколько id |

Один `ref_rttm` на запись должен содержать один file id - тогда гипотеза
конвертируется с тем же id. Если `ref_rttm` или `ref_text` указаны, но файл
не найден, строка получает `error`; молча пропускать метрику нельзя.

`ref_text` с метками спикеров нужен для cpWER и attribution. Формат -
как у `.transcript.json` (`utterances` с `speaker`, `start`, `end`, `text`,
`words`), JSONL с теми же полями или JSON-массив. Обычный `.txt` считается
одним спикером: для него считаются только WER/CER.

Приложение пишет JSON-события в stdout. `run_batch.py` берёт путь к
транскрипту и метаданные из последнего события
`{"type":"done","json":...,"duration_sec":...,"speakers":[...]}`,
а текст ошибки - из `{"type":"error","message":...}`. Если события нет,
используется `<stem>.transcript.json` рядом с аудио. Старый транскрипт
(не изменился mtime) помечается ошибкой `stale`.

## 5. Пакетный прогон

```bash
$PY tools/diar_eval/run_batch.py \
  --manifest /data/open/manifest.json \
  --data-root /data/open \
  --out runs/2026-11-09-abc1234 \
  --engine gigaam --limit 20
```

- Один файл - один процесс, таймаут `--timeout` (по умолчанию 3600 с).
- `--dataset` можно повторять для фильтра.
- Release-бинарник обязателен; проверка наличия - до первого файла.
- Peak RSS снимается через `/usr/bin/time -v` (Linux). На других ОС колонка
  остаётся пустой.
- stderr упавших файлов, команда и tail stdout складываются в
  `failures/<id>.log`.
- `--out` по умолчанию `runs/<дата>-<короткий commit>`.

Результат: `metrics.csv` (строка на файл), `manifest.json` прогона,
`rttm/<id>.rttm` (гипотеза для DER), `failures/`. Если `--out` указывает
в каталог входного `manifest.json`, отчёт прогона пишется в
`run-manifest.json`, чтобы не перезаписать вход.

Обязательные колонки `metrics.csv`:

`dataset, subset, file_id, duration, speakers_ref, speakers_hyp, WER, CER,
DER, SCA, cpWER, attribution, wall_sec, rtf, peak_rss_kb, exit_code, error`

Дополнительно пишутся `language, engine, ref_words, wer_errors, ref_chars,
cer_errors, der_ref_sec, der_miss_sec, der_false_alarm_sec, der_confusion_sec,
cpwer_ref_words, cpwer_errors, attribution_matched, attribution_correct,
words_hyp, transcript, MAE`: первые из них нужны `report.py` для
micro-агрегатов, `transcript` - путь к JSON для отладки.

Код возврата `run_batch.py`: `0` - ошибок нет, `1` - есть строки с `error`,
`2` - не найден манифест/бинарник или сломан вход.

## 6. Метрики

```bash
$PY tools/diar_eval/metrics.py \
  --ref ref.transcript.json --hyp hyp.transcript.json
$PY tools/diar_eval/metrics.py --ref ref.txt --hyp hyp.transcript.json --json
```

- Нормализация: NFKC, нижний регистр, `ё` → `е`, пунктуация удаляется,
  пробелы схлопываются. Числа не разворачиваются.
- WER/CER - `jiwer`, micro-сумма ошибок на отчёт.
- SCA - доля файлов с точным числом спикеров; MAE есть в библиотеке.
- cpWER - группировка слов по спикерам, перебор перестановок до 8 спикеров,
  минимум WER. Больше 8 - `ValueError`, метрика не считается.
- attribution - выравнивание слов `jiwer`, матрица «спикер эталона × спикер
  гипотезы» на совпавших словах, Hungarian (`scipy`, есть запасная
  реализация), доля верных. Считается только при ≥2 спикерах в эталоне.

## 7. Нормализованный WER

```bash
$PY tools/diar_eval/normalize_wer.py \
  --metrics ~/.cache/podtekst-bench/runs/golos-crowd-100/metrics.csv \
  --manifest ~/.cache/podtekst-bench/data/manifest-run.json \
  --data-root ~/.cache/podtekst-bench/data \
  --out ~/.cache/podtekst-bench/runs/golos-crowd-100/metrics_norm.csv
```

Скрипт читает `metrics.csv`, заново собирает эталон и гипотезу по записям
манифеста (`ref_text` и `transcript`) и дописывает к каждой строке `WER_norm`,
`CER_norm`, `norm_method`. Исходные колонки, включая `WER` и `CER`, не
меняются: raw-метрика остаётся точкой отсчёта. Строки без `WER` копируются с
пустыми новыми полями. Если для строки не нашлась запись манифеста или файл, в
`norm_method` пишется `error: ...`, а код возврата становится `1`.

Конвейер (`norm_method = numbers+latin+vowel-fold`):

1. `normalize_text` из `metrics.py`: NFKC, нижний регистр, `ё` → `е`, без
   пунктуации.
2. `normalize_numbers`: цифры и числительные 0-1000, тысячи, проценты и слова
   десятичных дробей заменяются на `<num>`, подряд идущие `<num>`
   схлопываются в один. Границы слов проверяются по токенам, поэтому
   `одиннадцать` маскируется, а `одиннадцати` - нет.
3. `transliterate_latin`: посимвольная транслитерация латиницы в кириллицу
   (`joy` → `джой`).
4. `fold_vowels`: подряд идущие гласные стягиваются в одну, конечная гласная
   отбрасывается (`home` → `хоме` → `хом`, `хоум` → `хом`).

Шаги 3-4 - эвристика, а не лингвистическая транслитерация: она уравнивает
`joy`/`джой` и `home`/`хоум`, но бренды с нерегулярным чтением (`synergy` и
`синергия`, `youtube` и `ютуб`) остаются ошибками. `fold_vowels` сокращает и
часть настоящих различий: `была` и `был` сравниваются как `был` и `был`.
Поэтому `WER_norm` - метрика с точностью до формата чисел и записи латиницы, а
не «настоящий» WER.

Маскирование чисел проверяет только наличие числа, но не его значение:
`500` и `250` в `WER_norm` неразличимы. Числа нужно оценивать отдельно.

`CER_norm` считается на том же нормализованном тексте, поэтому символов в нём
меньше, а значение ниже raw CER. Сравнивать `CER_norm` можно только с
`CER_norm`.

Self-test на синтетике: `21:00` = `двадцать один ноль ноль`, `866 663` =
`восемьсот шестьдесят шесть шестьсот шестьдесят три`, `joy home` = `джой хоум`,
обычный текст не маскируется.

```bash
$PY tools/diar_eval/normalize_wer.py --self-test
```

## 8. Отчёт

```bash
$PY tools/diar_eval/report.py \
  --run runs/2026-11-09-abc1234 \
  --baseline tools/diar_eval/baseline.json
```

`--run` ищет `metrics.csv`, пишет `report.md` рядом. Можно `--csv path` и
`--out path`. Отчёт содержит micro WER/CER/DER, SCA, MAE, cpWER, attribution,
RTF p50/p95, peak RSS, список ошибок, 95% доверительные интервалы
(`bootstrap_ci`: ресэмплинг файлов с возвращением, 1000 повторов, seed
`20261109`) и сравнение с baseline.

Формат `baseline.json` (ключи - `dataset/subset`):

```json
{
  "schema": 1,
  "groups": {
    "ami-ihm/test": {
      "WER": 0.12,
      "DER": 0.18,
      "SCA": 0.81,
      "cpWER": 0.15,
      "attribution": 0.88,
      "RTF_p50": 0.32,
      "peak_rss_kb": 1200000
    }
  }
}
```

Порог регресса: WER/DER - больше `min(5% baseline, 1 п.п.)`; cpWER и
attribution - больше 3 п.п.; RTF - +15%; peak RSS - +20%. Это правила
раздела 9 ТЗ.

## 9. Ограничения

- `prepare.py` и скачивание наборов в этот PR не входят. `datasets.yaml` -
  шаблон: данные, SHA-256 и подготовка заполняются отдельно.
- `datasets.yaml` не используется кодом автоматически; manifest готовится
  вручную или будущим `prepare.py`.
- cpWER ограничен 8 спикерами, attribution требует эталонный текст с
  метками спикеров.
- SCA в отчёте общий; разбивка по группам 2-4 и 5-8 спикеров не делается.
- Доверительные интервалы - bootstrap по файлам (1000 ресэмплов, seed
  `20261109`); при одном файле в наборе или нулевом знаменателе пишется `n/a`.
- Первый файл каждого набора не исключается из RTF (прогрев не реализован).
- Peak RSS только Linux; для macOS/Windows нужен отдельный замер.
- `run_batch.py` не скачивает модели и не собирает приложение.
- Аудио, эталоны, транскрипты и каталог `runs/` не коммитятся.

## 10. Синтетические встречи

```bash
$PY tools/diar_eval/synth_meetings.py \
  --clips-dir ~/.cache/podtekst-bench/data/fleurs/ru_ru \
  --out ~/.cache/podtekst-bench/synth/ru-2spk-60s-ov0 \
  --speakers 2 --duration 60 --overlap 0 --seed 20261109 \
  --manifest-out ~/.cache/podtekst-bench/synth/manifest.json
```

Один клип FLEURS содержит одного диктора, но разные клипы могут принадлежать
одному диктору. Клип повторяется, но разные дикторы внутри
сегмента не смешиваются. Реплики чередуются с паузами 0,2-0,5 с, `--overlap`
- процент от длительности предыдущей реплики. Пишутся `meeting.wav`
(16 кГц моно), `meeting.rttm`, `meeting.ref.json` с текстом реплик и запись
manifest для `run_batch.py` (`ref_rttm`, `ref_text`, `language`, `engine`:
`whisper` для `en`, `gigaam` для `ru`). `--manifest-out` накапливает записи
нескольких сценариев в один общий манифест.

## P12

В репозиторий и self-tests попадают только обезличенные синтетические
фикстуры: слова «альфа/бета», спикеры `A/B/X/Y`. Реальные записи, имена и
фрагменты речи не используются.

### Регрессия громкости и числа спикеров

`--second-speaker-db -18` уменьшает уровень второго спикера на 18 дБ;
без флага генератор сохраняет прежний результат. Для одинакового seed состав
реплик и разметка не меняются. Файлы и модели остаются локальными.

Для gate нужны локальные FLEURS `test.tsv` и `audio/test.tar.gz` в
`$MR_DIAR_BENCH_ROOT/tmp/hf/fleurs/data/{en_us,ru_ru}`. Подготовка выбирает
четыре различимых голоса по метаданным: для каждой пары совпадает предложение
или различается указанный пол. Гарантия разных дикторов для одного предложения
приведена в [статье FLEURS](https://simran-khanuja.github.io/assets/fleurs_slt.pdf).
Проверка сверяет метаданные, хеши аудио и исходный архив; тексты и аудио
остаются в локальном кэше.

```bash
ROOT="$HOME/.cache/podtekst-bench"
PY="$ROOT/venv/bin/python"
"$PY" tools/diar_eval/prepare_fleurs_speakers.py "$ROOT"
MR_DIAR_BENCH=1 MR_DIAR_BENCH_ROOT="$ROOT" "$PY" tools/diar_eval/regression.py --prepare-only
```

Исходную сборку прогоняют через `run_batch.py` по
`$ROOT/regression-verified/manifest.json`; её `metrics.csv` закрепляет baseline:

```bash
MR_DIAR_BENCH=1 MR_DIAR_BENCH_ROOT="$ROOT" "$PY" tools/diar_eval/regression.py \
  --baseline-from-csv /путь/к/исходному/metrics.csv
MR_DIAR_BENCH=1 MR_DIAR_BENCH_ROOT="$ROOT" \
MR_DIAR_BENCH_APP="$PWD/src-tauri/target/release/podtekst" \
PODTEKST_DATA_DIR="$ROOT/appdata" \
  "$PY" tools/diar_eval/regression.py --baseline "$ROOT/regression-verified/baseline.json"
```

Проверяются обычные и тихие двухспикерные записи: DER английского ≤ 15%,
число русских спикеров равно двум. DER и ошибка числа спикеров для русских
записей с тремя и четырьмя спикерами не должны превышать baseline на той же
выборке (допуск DER 0,0001 п.п.). Исторические значения другой выборки не
используются. Seed 20261109, collar 0, перекрытия учитываются. Для CI нужны
self-hosted runner `diarization-bench`, переменные `MR_DIAR_BENCH=1`,
`MR_DIAR_BENCH_ROOT` и `MR_DIAR_BENCH_BASELINE`, локальные модели и venv.
Скачивания датасетов в CI нет.
