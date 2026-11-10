# Статус гейтов бенчмарка Podtekst

Снимок на 10.11.2026. Ветка `fix/benchmark-gates`, HEAD `84f718d`;
автоопределение числа спикеров (auto-k) закоммичено в `b03ac04`.
Цифры взяты из готовых артефактов `~/.cache/podtekst-bench/runs/`; новых прогонов
для этого документа не делалось. Подробные таблицы и оговорки - в
`docs/testing/BENCHMARK-REPORT.md`, критерии приёмки - в `docs/testing/OPEN-DATASETS-TZ.md`.

Сводка: 12 гейтов, из них 4 done, 6 partial, 2 blocked.
done - сделано и подтверждено артефактом; partial - сделано частично или не закоммичено;
blocked - внешнее ограничение, без которого гейт не закрыть.

| Gate | Что сделано | Метрика / доказательство | Статус | Коммит |
|---|---|---|---|---|
| P0-1. Whisper turbo | Encoder KV-кэш между шагами, лимит декодера 448 позициями (`448 - prompt_len`), env-gated регрессия `whisper_transcribes_every_vad_segment` | `xmyyy` 411,6 с: exit 0, 37 реплик, Reshape-ошибок нет. FLEURS `en_us`, 5 файлов: WER 4,2 / 3,0 / 0 / 0 / 11,8%, среднее 3,8% (не 96%) | done | `1642244`, `e6c8cc5` |
| P0-2. Выбор движка в `auto` | Для не-русского языка `auto` выбирает Whisper, для `ru`/пустого/`auto` - GigaAM; тесты на `en`, `de`, `zh`, `ru` | Английский файл идёт через Whisper, русский через GigaAM; русская транскрипция на английском больше не появляется | done | `5a19a04` |
| P0-3. Диаризация без текста ASR | VAD-группы и акустические единицы без текста доходят до диаризации | На Whisper IS1009a miss 13,1%; на GigaAM fallback miss около 30%, потому что текстовый RTTM не видит textless-группы. Архитектурная правка, выигрыш DER не измерен | partial | `ddb0335` |
| P1-4. Калибровка числа спикеров | `agglomerative_gap` (`MIN_AUTO_K_GAP 0,04`), финальное слияние центроидов (`CENTROID_MERGE_SIM 0,62`), `AUTO_K_MAX_SPEAKERS 12`, нижняя граница 2 при `min_speakers == 0`; guard cannot-link не выбран | Vox, 5 файлов: DER 44,35% → 33,18% (CI 16,25-47,39), SCA 0% → 60%, MAE 2,80 → 0,60. AMI: 71,48% (4 → 13) → 61,31% (4 → 7, over-split); guard-вариант 51,79% (4 → 2) ломал Vox `mxdpo` (66,2%) | partial | нет, рабочее дерево (`diarize.rs`, `cluster.rs`) |
| P1-5. Число спикеров в CLI/UI | Флаги `--min-speakers` / `--max-speakers` и выбор в UI; явный k обходит auto-k | До auto-k: Vox `mxdpo` без флагов - 3 спикера, с `--min-speakers 2 --max-speakers 2` - ровно 2. Oracle-k: AMI 45,32% (4/4), Vox 30,70% (SCA 75%) | done | `7861866` |
| P1-6. A/B CTC | Флаг `--ctc off`, прогон A/B на Golos | 20 файлов Golos: WER 9,52% в обоих режимах; `off` быстрее на 38% и легче на 334 МБ RSS | done | `750d8d8` |
| P1-7. Нормализация чисел | `normalize_wer.py`: числа, латиница, vowel-fold; raw и normalized WER в `metrics_norm.csv` | Golos Crowd: 20,79% → 10,00%; Farfield: 15,53% → 12,58% | partial | `b3bd132` |
| P1-8. Ленивая загрузка моделей | Грузится только выбранный ASR-движок, CTC - по флагу; кэш моделей на процесс | GigaAM: RSS 2424 → 848 МБ, wall 9,7 → 5,1-6,4 с; Whisper: RSS 3103 → 2574 МБ | partial | `457df95` |
| P2-9. Русский корпус встреч | Открытого корпуса с эталонной диаризацией нет; синтетические встречи как суррогат | Синтетика: 4 сценария по 60 с (ru 2/3/4 спикера, en 2), DER 13,39-40,24%, cpWER 18,85-41,96% | blocked | `ad39e7b` (только синтетика) |
| P2-10. Профили и атрибуция | cpWER и attribution на синтетике; на AMI/AISHELL-4 эталонного текста с метками спикеров нет | Attribution: ru 2 сп. 85,34%, ru 3 сп. 60,80%, ru 4 сп. 79,56%, en 2 сп. 100%; cpWER 18,85-41,96% | partial | `ad39e7b` |
| P2-11. Bootstrap CI и RTTM | CI по файлам в `report.py`, экспорт RTTM в харнессе | CI 95%: Crowd 20,79 [14,28; 27,59], Farfield 15,53 [10,73; 21,05], FLEURS ru 5,42 [3,08; 8,38], Vox auto-k DER 33,18 [16,25; 47,39] | partial | `9ad735c`, `054f8a7` |
| P2-12. GPU Execution Provider | Не выполнялся: на машине нет `nvidia-smi` и CUDA | Замера GPU нет; ориентир CPU RTF 0,23-0,55 на длинных файлах | blocked | нет |

## Почему partial и blocked

- **P0-3 (partial):** архитектурная правка сделана, но текстовый RTTM не видит
  textless-группы, поэтому выигрыш DER не измерить без правки харнесса.
- **P1-4 (partial):** Vox выигрывает, но AMI переразбивается 4 → 7; guard-вариант
  давал 4 → 2 и ломал Vox, код не закоммичен, CI Vox пересекаются с базовой линией.
- **P1-7 (partial):** нормализация работает только в метрике; постобработка самого
  транскрипта (приведение чисел к словам) не сделана.
- **P1-8 (partial):** выбирается только ASR-движок; VAD и диаризация грузятся всегда,
  фонового прогрева нет.
- **P2-9 (blocked):** открытого русского корпуса встреч с эталоном «кто и когда» нет;
  синтетика не заменяет естественные перекрытия и шум.
- **P2-10 (partial):** атрибуция измерена только на синтетике; на AMI/AISHELL-4 нет
  эталонного текста с метками спикеров, поэтому cpWER и attribution там не считаются.
- **P2-11 (partial):** CI и RTTM есть, но CI для cpWER/attribution не строится
  (нет эталонного текста), а выборки Vox/AMI - 1-5 файлов.
- **P2-12 (blocked):** GPU и CUDA на машине отсутствуют, сравнение провайдеров
  выполнить не на чем.

## Источники цифр

| Что | Файл / прогон |
|---|---|
| Vox auto-k | `runs/vox-autok/metrics.csv`, 5 файлов, `e6c8cc5` |
| Vox без auto-k | `runs/vox-whisper-final/metrics.csv`, 5 файлов, `e6c8cc5` |
| AMI текущий код (auto-k + min 2, без guard) | `runs/ami-final/metrics.csv`, 1 файл, `e6c8cc5` |
| AMI guard cannot-link (не выбран) | `runs/ami-autok2/metrics.csv`, 1 файл, `e6c8cc5` |
| AMI без auto-k | `runs/ami-whisper-final/metrics.csv`, 1 файл, `e6c8cc5` |
| Русский ASR | `runs/golos-crowd-100`, `runs/golos-farfield-100`, `runs/fleurs-ru-50`, сборка `3624ee4` |
| Синтетика | `runs/synth-meetings/metrics.csv`, `9ad735c` |
| Bootstrap CI | `tools/diar_eval/report.py`, 1000 ресэмплов, seed `20261109`, пересчёт по `metrics.csv` |

## Что осталось

1. Закоммитить auto-k (`diarize.rs`, `cluster.rs`) и после коммита перепрогнать Vox/AMI.
2. Довести оценку k на AMI: текущий код даёт 4 → 7 (over-split), guard-вариант - 4 → 2
   и ломает Vox. Нужен критерий k, устойчивый к выбросам. Потолок при верном k - 45,32%.
3. P0-3: научить RTTM и метрику видеть textless-группы, иначе вклад правки в DER
   на GigaAM fallback не измерить.
4. Полные test split и недостающие языки: Golos Crowd 300, Farfield, FLEURS `ru`/`en`
   целиком, `de_de`/`cmn_hans_cn`, AMI SDM, AISHELL-4, AliMeeting, Vox 8/8.
5. P1-7: нормализация чисел и латиницы в постобработке транскрипта, а не только в метрике.
6. P1-8: ленивая загрузка диаризации и прогрев; замеры 60 и 120 мин для RTF и RSS.
7. P2-9/P2-10: естественный русский корпус встреч и CI для cpWER/attribution.
8. P2-12: GPU-провайдер на машине с CUDA; кросс-платформенный смоук macOS/Windows.
9. Зафиксировать дефолт CTC по результатам A/B (WER равный, `off` быстрее и легче).
