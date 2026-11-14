# Тесты и стенд качества

Стенд проверяет распознавание речи, разделение спикеров (диаризацию), привязку слов к спикерам, скорость и устойчивость на открытых данных. Аудио и тексты храним вне git. В репозитории лежат скрипты, манифесты, метаданные отбора и агрегированные метрики.

## Документы

- [SPEC.md](SPEC.md) - что проверяем, на каких данных, какие метрики и пороги.
- [../../tools/diar_eval/baseline.json](../../tools/diar_eval/baseline.json) - текущие метрики и известные разрывы.
- [EXTERNAL-BENCHMARKS.md](EXTERNAL-BENCHMARKS.md) - внешние ориентиры.
- [../tools/diar_eval/README.md](../../tools/diar_eval/README.md) - команды инструментов.

## Локальный корень данных

Все пути задаются через `MR_DIAR_BENCH_ROOT`. Пример:

```bash
ROOT="$HOME/.cache/podtekst-bench"
PY="$ROOT/venv/bin/python"
```

Ожидаемая раскладка:

```text
$MR_DIAR_BENCH_ROOT/
  data/                       Golos, FLEURS, VoxConverse, AMI
  tmp/hf/fleurs/data/         test.tsv и audio/test.tar.gz
  diar-regression-clips/      проверенные клипы + proof.json
  diar-main-clips/            основной EN/RU 2spk-набор
  diar-main-meetings/         normal.json и quiet.json
  regression-verified/        манифест и сгенерированные встречи small
  appdata/models/             модели приложения
  venv/                       Python-окружение
  runs/                       результаты прогонов
```

## Как получить данные

В репозитории нет аудио, текстов и готовых датасетов. Скачайте их в `$ROOT` один раз.

### FLEURS

Для `small` нужны `ru_ru` и `en_us`. Для `full` эти же файлы используются в матрице и ASR.

```bash
for lang in ru_ru en_us; do
  mkdir -p "$ROOT/tmp/hf/fleurs/data/$lang/audio"
  base="https://huggingface.co/datasets/google/fleurs/resolve/main/data/$lang"
  curl -L --fail -o "$ROOT/tmp/hf/fleurs/data/$lang/test.tsv" "$base/test.tsv"
  curl -L --fail -o "$ROOT/tmp/hf/fleurs/data/$lang/audio/test.tar.gz" "$base/audio/test.tar.gz"
done
```

Затем подготовьте проверенные клипы и основной набор:

```bash
"$PY" tools/diar_eval/prepare_fleurs_speakers.py "$ROOT"
MR_DIAR_BENCH=1 MR_DIAR_BENCH_ROOT="$ROOT" \
  "$PY" tools/diar_eval/regression.py --prepare-only
"$PY" tools/diar_eval/prepare_main_meetings.py "$ROOT"
```

`prepare_fleurs_speakers.py` проверит TSV, архив и SHA-256 клипов.

### Русская матрица 36

`full` ожидает `$ROOT/runs/matrix-m/manifest.json`. Соберите его из четырёх клипов `ru_ru`:

```bash
mkdir -p "$ROOT/runs/matrix-m"
for speakers in 2 3 4; do
  for duration in 60 120; do
    for overlap in 0 5 15; do
      for gain in 0 -18; do
        suffix=""
        [ "$gain" = "-18" ] && suffix="-spk2-18db-quieter"
        name="synth-ru-${speakers}spk-${duration}s-ov${overlap}${suffix}"
        "$PY" tools/diar_eval/synth_meetings.py \
          --clips-dir "$ROOT/diar-regression-clips/ru_ru" \
          --out "$ROOT/runs/matrix-m/$name" \
          --speakers "$speakers" --duration "$duration" --overlap "$overlap" \
          --second-speaker-db "$gain" --name "$name" \
          --manifest-out "$ROOT/runs/matrix-m/manifest.json"
      done
    done
  done
done
```

### Golos

Golos нужен для `full`. Скачайте два parquet-файла:

```bash
mkdir -p "$ROOT/tmp/hf/golos"
curl -L --fail -o "$ROOT/tmp/hf/golos/crowd.parquet" \
  "https://huggingface.co/datasets/bonlime/golos-test/resolve/main/crowd/test-00000-of-00003.parquet"
curl -L --fail -o "$ROOT/tmp/hf/golos/farfield.parquet" \
  "https://huggingface.co/datasets/bonlime/golos-test/resolve/main/farfield/test-00000-of-00001.parquet"
```

Подготовка Golos из parquet в WAV и `data/manifest-run.json` пока не входит в репозиторий. Если нужен полный ASR-прогон, подготовьте эти файлы локально и положите их в `$ROOT/data`.

### AMI и VoxConverse

Эти наборы нужны только для релизных проверок. Ссылки на parquet и лицензии есть в `tools/diar_eval/datasets.yaml`. В `small` и `full` они не входят.

## Наборы

| Набор | Когда | Состав | Время |
|---|---|---|---|
| `small` | перед каждым пул-реквестом | 6 синтетических встреч и 4 основных файла | минуты |
| `full` | перед релизной веткой | `small`, русская матрица 36 и 450 файлов русского распознавания речи | десятки минут или часы |
| release | перед релизом | `full`, VoxConverse, AMI, записи 60 и 120 минут, macOS и Windows | отдельное решение |

`small` ловит грубые поломки, но не заменяет `full`. Диагностика (абляции, разбор стадий) не входит в порог и не хранится в репозитории как отдельные документы.

## Быстрый старт

```bash
ROOT="${MR_DIAR_BENCH_ROOT:-$HOME/.cache/podtekst-bench}"
PY="$ROOT/venv/bin/python"

cargo build --release -p mr-app --locked --manifest-path src-tauri/Cargo.toml

"$PY" tools/diar_eval/synth_meetings.py --self-test
"$PY" -m unittest discover -s tools/diar_eval -p 'test_*.py'
"$PY" tools/diar_eval/prepare_fleurs_speakers.py "$ROOT"
MR_DIAR_BENCH=1 MR_DIAR_BENCH_ROOT="$ROOT" "$PY" tools/diar_eval/regression.py --prepare-only
"$PY" tools/diar_eval/prepare_main_meetings.py "$ROOT"

OUT="$ROOT/runs/small-$(date +%Y%m%d-%H%M%S)"
MR_DIAR_BENCH=1 PODTEKST_DATA_DIR="$ROOT/appdata" \
  "$PY" tools/diar_eval/run_suite.py --suite small --out "$OUT"
"$PY" tools/diar_eval/check.py --suite small --run "$OUT/metrics.csv"
```

`small` занимает минуты. `full` идёт долго, поэтому запускается перед релизной веткой:

```bash
OUT="$ROOT/runs/full-$(date +%Y%m%d-%H%M%S)"
MR_DIAR_BENCH=1 PODTEKST_DATA_DIR="$ROOT/appdata" \
  "$PY" tools/diar_eval/run_suite.py --suite full --out "$OUT"
"$PY" tools/diar_eval/check.py --suite full --run "$OUT/metrics.csv"
```

## Правила

- Базовая линия (baseline) - точка отсчёта для регресса, а не разрешение на релиз.
- Цель релиза, порог регресса и известный разрыв - разные вещи. Смотрите SPEC §8.5.
- Нельзя подбирать пороги или менять эталон по результатам теста.
- В git не попадают аудио, тексты, имена файлов записей и логи с путями.
