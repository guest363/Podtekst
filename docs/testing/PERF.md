# Скорость конвейера: локальные замеры и A/B

Документ описывает замеры скорости транскрибации (`MR_PERF=1`), базовую линию
и парный A/B. Качество (WER/DER) и пороги `report.py` здесь не меняются:
это отдельный слой, который отвечает на вопрос «стало ли медленнее».

## 1. Зачем

`wall_sec`/`rtf` из `run_batch.py` включают запуск процесса, загрузку моделей и
колебания машины. Для решения о регрессе нужны стадии и нормализация на
калибровку, иначе одна и та же сборка на загруженном ноутбуке выглядит хуже
сама себя. Сырой RTF остаётся справочной величиной и не заменяется.

## 2. Что пишет release-бинарник

При `MR_PERF=1` (или `MR_PERF=true`) рядом с `X.transcript.json` появляется
`X.perf.json`. Без флага не создаётся ни файла, ни аллокаций на замеры;
`.transcript.json`, `.md`, `.txt`, логи, SSE и UI не меняются. Ошибка записи
sidecar не ломает расшифровку.

Схема (`schema=1`):

```json
{
  "schema": 1,
  "commit": "9daded3",
  "engine": "gigaam",
  "ctc": true,
  "audio_sec": 60.0,
  "input_channels": 1,
  "processed_channels": 1,
  "total_ms": 21876.3,
  "stages_ms": {
    "decode": 412.0, "models": 934.1, "vad_model": 51.2, "vad": 233.3,
    "asr": 12011.0, "diar": 7900.4, "diar_models": 120.0, "json": 31.0
  },
  "other_ms": 303.3,
  "peak_rss_kb": 123456,
  "calib": {"version": 1, "ms": 73.42, "dim": 448, "profile": "release", "logical_cpus": 20},
  "counters": {"utterances": 42, "words": 901, "speakers": 2, "speech_sec": 55.4}
}
```

Все поля — технические параметры: канал, хронометраж, число спикеров, стадии.
Текста, имён, путей и тем в sidecar нет (P12).

### Определения

| Поле | Смысл |
|---|---|
| `total_ms` | от старта обработки файла после калибровки до конца записи `.md`/`.txt` и `record_stats` |
| `perf_ms` | `total_ms` в `metrics.csv` |
| `rtf_perf` | `perf_ms / 1000 / duration` — сырой RTF по in-process таймеру |
| `rtf_norm` | `(perf_ms / calib_ms) / duration` — калиброванный RTF, размерность 1/с; выше — медленнее |
| `decode` | декодирование всех каналов |
| `models` | загрузка ASR + CTC |
| `vad_model` | загрузка Silero VAD |
| `vad` | поиск речи по каналам |
| `asr` | распознавание всех каналов |
| `diar` | диаризация и привязка слов; включает ленивую загрузку моделей |
| `diar_models` | загрузка diar-моделей; вложена в `diar` и не вычитается |
| `json` | сериализация и запись `.transcript.json` |
| `other_ms` | `max(0, total - decode - models - vad_model - vad - asr - diar - json)` |
| `audio_sec` | длительность декодированного аудио |
| `input_channels` | каналов после декодирования, до downmix/дедупликации |
| `processed_channels` | каналов после `channel_mix` |
| `peak_rss_kb` | пиковый RSS процесса: Linux `VmHWM`, macOS/iOS `rusage_info_v4.ri_lifetime_max_phys_footprint`, Windows `GetProcessMemoryInfo`; остальные платформы — пусто |

Стадии измеряются RAII-таймером: длительность учитывается и при раннем `?`,
но `perf.json` пишется только после успешной записи артефактов; при ошибке
sidecar не остаётся. Время по каналам суммируется. Debug-события
`stage.start`/`stage.end` остаются как были. Ошибка записи sidecar не ломает
расшифровку: причина уходит в stderr без путей.

## 3. Калибровка

Перед `total_ms` выполняется фиксированный однопоточный CPU-кернел
(f32-умножение матриц и цикл `ln`/`exp`, 5 повторов, минимум). Он даёт
`calib.ms`, `calib.dim` и `calib.profile` (`release`/`debug`); `logical_cpus` —
число доступных потоков. Ограничения:

- кернел не учитывает GPU, память и ONNX Runtime — он меряет только загрузку CPU;
- калибровка один раз на процесс и не входит в `total_ms`;
- baseline снимается только с `profile=release`; `check` сверяет `version` и `dim`;
- на разных машинах сравнивать можно только `rtf_norm`, а решение о регрессе
  принимать только по парному A/B на одной машине.

## 4. Отпечаток машины и baseline

`perf.py capture` считает `machine_id = sha256(cpu | os | arch | logical_cpus | bench_id)[:12]`
(CPU: Linux `/proc/cpuinfo`, macOS `sysctl machdep.cpu.brand_string`, Windows
`PROCESSOR_IDENTIFIER`). `bench_id` — локальный `secrets.token_hex(6)` из
`<root>/machine-id` (создаётся с правами 0600): он различает одинаковые CPU.
Если файл недоступен, печатается предупреждение и используется прежний хеш.
Baseline хранится в `tools/diar_eval/perf-baseline.json`:

```json
{"schema": 1, "machines": {"<machine_id>": {
  "cpu": "...", "os": "Linux", "arch": "x86_64", "logical_cpus": 20,
  "bench_id": "...", "calib_version": 1, "calib_dim": 448,
  "calib_profile": "release", "suite": "perf", "reps": 3,
  "records": {"<requested_id>": {"perf_ms": 1.0, "calib_ms": 1.0, "rtf_norm": 1.0}},
  "aggregate": {"perf_ms": 1.0, "calib_ms": 1.0, "rtf_norm": 1.0}
}}}
```

Правила:

- ключ baseline — `requested_id` набора (`suites/perf.json`); `file_id` — запасной ключ для старых прогонов; старый baseline по `file_id` нужно переснять после появления `requested_id`;
- запись обновляется только для текущей машины, чужие не перезаписываются;
- если у записи машины другой `suite`, `capture` отказывается её менять;
- сырой RTF можно сравнивать только на одной машине и при одном `calib_ms`;
- между машинами сравнивается `rtf_norm`, но регресс подтверждается только A/B;
- baseline — точка отсчёта для конкретного набора `perf`, а не цель релиза.

## 5. Пороги

| Что | Порог |
|---|---|
| total `rtf_norm` | медианный рост больше +5% (по файлам и агрегату) → регресс |
| стадия (агрегат набора) | рост больше +10% **и** абсолютный вклад больше 20 мс → регресс |

Стадии с малым абсолютным вкладом (единицы миллисекунд) не блокируют: на таком
масштабе таймер шумит сильнее сигнала. Стадии по файлам показываются в таблице,
но не блокируют: ленивая загрузка diar-моделей даёт десятки мс разброса.
В CI перф-гейт не ставится.

## 6. Команды

Набор `perf` — три файла: ASR RU, ASR EN, встреча RU (`suites/perf.json`).
Полная базовая точка качества — `small`.

`--ids` сначала ищет точное совпадение с вычисленным `file_id`
(`id` либо `dataset/subset/stem`). Если его нет, id сопоставляется с суффиксом
пути аудио после нормализации слэшей: `audio` должен заканчиваться на
`<id>.wav`. Так канонический `fleurs/ru_ru/<stem>` работает и на легаси-манифесте
с `fleurs-ru`/`test`. Неизвестный id или неоднозначный суффикс — ошибка с
ненулевым кодом.

```bash
# 1. Снять baseline текущей машины (N повторов, медианы по файлам)
python3 tools/diar_eval/perf.py capture --suite perf --reps 3 \
  --app src-tauri/target/release/podtekst \
  --out ~/.cache/podtekst-bench/runs/perf-baseline \
  --baseline tools/diar_eval/perf-baseline.json
# --overwrite чистит rep-* в --out и разрешает замену результатов

# 2. Сверить прогоны с baseline (каталог или несколько metrics.csv)
python3 tools/diar_eval/perf.py check \
  --run ~/.cache/podtekst-bench/runs/perf-baseline \
  --baseline tools/diar_eval/perf-baseline.json

# 3. Парный A/B двух бинарников (порядок A/B меняется каждый повтор)
python3 tools/diar_eval/perf_ab.py --a old/podtekst --b new/podtekst \
  --suite perf --reps 5 --warmup 1 \
  --out ~/.cache/podtekst-bench/runs/perf-ab
```

`perf_ab.py` гоняет по одному файлу за запуск, первый повтор отбрасывает,
считает парные отношения `B/A` по `rtf_norm` и стадиям, медиану и
бутстрэп-CI 95% (1000 повторов, зерно 20261119). Пишет `ab-report.md` и
`ab.json` (в нём `commit`/`engine` обоих бинарников); exit 1 при медианном
total > 1.05. Предупреждения по стадиям (`> +10%` и `> 20 мс`) не меняют код
возврата; пары `(ratio, abs)` считаются по одним и тем же повторам.

## 7. Ограничения

- Шум слабого CPU и фоновой нагрузки не устраняется калибровкой полностью;
  поэтому порог стадий требует абсолютного вклада.
- `rtf_norm` имеет размерность 1/с: это калиброванный RTF, а не безразмерная
  величина и не `rtf`.
- `peak_rss_kb` — пик всего процесса, а не отдельной стадии; на macOS/iOS это
  `ri_lifetime_max_phys_footprint`, на Android пик не читается.
- `perf.py check` принимает несколько прогонов и берёт медианы по файлам;
  единичный выброс не должен менять вывод. Total блокирует по файлам и
  агрегату, стадии — только по агрегату (медиана по файлам): регресс стадии
  лишь в одном файле может не всплыть, для строгой проверки нужен A/B.
- Для строгого сравнения версий используйте `perf_ab.py`: он чередует A/B,
  отбрасывает прогрев и считает парные отношения на одной машине.
- Аудио, транскрипты и `perf.json` живут локально и не коммитятся
  (`*.perf.json` в `.gitignore`, `perf-baseline.json` — коммитится).
