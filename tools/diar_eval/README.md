# Офлайн-оценка диаризации

Dev-инструмент, не часть рантайма. Цель — заменить калибровку порогов «на слух»
на DER и точность числа спикеров на публичных наборах (VoxConverse, AISHELL-4,
AMI-IHM, AliMeeting).

## 1. Получить транскрипт

```bash
./run.sh --transcribe audio.wav --engine gigaam
# рядом появится audio.transcript.json
```

## 2. Конвертировать в RTTM

```bash
python3 tools/diar_eval/transcript_to_rttm.py audio.transcript.json audio.rttm
python3 tools/diar_eval/transcript_to_rttm.py --self-test
```

Спикеры перенумеровываются в `SPEAKER_01..NN` по времени речи: имена UI
(«Участник 1», «Собеседник 2») не обязаны совпадать с разметкой датасета,
для DER важна только согласованность меток внутри файла.

## 3. Посчитать DER

Установить метрику один раз (dev-окружение, в рантайм не попадает):

```bash
pip install pyannote.metrics
```

Затем:

```bash
python3 tools/diar_eval/evaluate.py reference.rttm audio.rttm --per-file
```

File id в гипотезе должен совпадать с эталоном: при конвертации передай
`--id <uri из reference.rttm>`. Через [OpenBench](https://github.com/argmaxinc/OpenBench)
(тот же инструмент, что использует senko) считаются те же метрики на стандартных
датасетах.

Для сравнения можно прогнать [senko](https://github.com/narcotic-sh/senko)
на тех же WAV и подать его RTTM в тот же расчёт. Senko — эталон, не зависимость.

## 4. Что калибровать

| Порог | Где | Смысл |
|---|---|---|
| `0.52` | `mr-app/src/transcribe/diarize.rs` | расстояние AHC (косинус) |
| `0.04` | `mr-ml/src/cluster.rs` | минимальный разрыв для auto-k |
| `0.62` | `mr-app/src/transcribe/diarize.rs` | merge центроидов одного голоса |
| `0.45` | `mr-app/src/transcribe/diarize.rs` | merge мелких кластеров |
| `0.35` | `mr-ml/src/onnx/segmentation.rs` | порог перекрытия PyAnnote |
| `0.45 / 0.06` | `mr-ml/src/lib.rs` | матчинг голосовых профилей |

Меняем один порог за прогон, фиксируем DER и SCA на всех наборах, откатываем
ухудшения. Ручной `min_speakers`/`max_speakers` проверяем отдельно: на встречах
автоматическое число спикеров ошибается заметно чаще, чем на чистых интервью.

## P12

В репозиторий и тесты попадают только публичные датасеты и синтетические
фикстуры. Личные записи, имена и фрагменты речи не коммитим.
