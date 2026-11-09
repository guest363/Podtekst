# Внешние бенчмарки для сравнения с Podtekst

Статус: черновик v1 от 09.11.2026.
Назначение: собрать в одном месте опубликованные WER и DER, с которыми можно сопоставлять Podtekst, и явно отметить, где условия не совпадают.
Область: только открытые публикации и репозитории. Внутренние замеры Podtekst сюда не входят.

Как читать таблицы:

- WER и DER - проценты, ниже лучше.
- `н/д` - величина или условие не опубликованы. `-` - система на этом наборе не оценивалась.
- Ссылки `[S1]`, `[S2]` и т. д. кликабельны; расшифровка - в разделе «Источники». У каждой строки есть источник.
- Публикации почти всегда используют не тот же режим, что Podtekst: другой сплит, beam search, fp32/fp16 вместо int8, другие аннотации. Это не повод не сравнивать, но повод сравнивать только однородные прогоны.

---

## 1. ASR

### 1.1. GigaAM v3: Golos, Common Voice RU, OpenSTT

Официальная таблица GigaAM v3 - это WER на validation-наборах, не обязательно на официальном test. Для GigaAM внешний языковой LM не заявлен; в той же таблице T-One идёт с `+ LM`, и это единственная строка с явным LM.

| Набор | Язык | CTC, WER % | RNNT, WER % | Декодирование | Железо | Размер модели | Источник |
|---|---|---:|---:|---|---|---|---|
| Golos Crowd | ru | 2,8 | 2,4 | н/д | н/д | 220-240M | [S2] |
| Golos Farfield | ru | 4,5 | 3,9 | н/д | н/д | 220-240M | [S2] |
| Common Voice 19 | ru | 1,3 | 0,9 | н/д | н/д | 220-240M | [S2] |
| OpenSTT Phone Calls | ru | 18,6 | 17,4 | н/д | н/д | 220-240M | [S2] |
| OpenSTT YouTube | ru | 11,6 | 10,6 | н/д | н/д | 220-240M | [S2] |
| OpenSTT Audiobooks | ru | 8,7 | 8,2 | н/д | н/д | 220-240M | [S2] |

Примечания:

- «OpenSTT» в таблице GigaAM не сводится к одному числу: есть три подмножества. Отдельного aggregate-числа нет.
- Common Voice - версия 19, не 15.
- GigaAM v3 - Conformer на 220-240M параметров, обучен на 700 тыс. ч русского SSL и 4 тыс. ч ASR [S1][S3]. Лицензия MIT [S1].
- В таблице GigaAM также приведён `Whisper*` на тех же русских наборах: Golos Crowd 19,0; Golos Farfield 16,4; Common Voice 19 - 5,5 [S2]. Версия Whisper и декодирование не указаны, поэтому мы не выносим эти числа в отдельную строку ASR-таблицы.

### 1.2. Whisper на FLEURS: en, de, fr, es, zh, ru

Официальной поязыковой таблицы для `large-v3` и `large-v3-turbo` нет. В анонсе `large-v3` опубликован график, а не таблица [S6]; в анонсе `turbo` - тоже график [S7]. Единственная официальная таблица FLEURS по языкам, которую можно процитировать построчно, - это `large-v2` из статьи Whisper, Appendix D.2.4 [S5]. Поэтому ниже она идёт как **прокси**, а не как измерение `large-v3`. Если нужны именно числа `turbo`, их придётся получить своим прогоном на FLEURS test с beam 5: опубликованной таблицы нет.

| Язык | Whisper large-v2, % | Whisper large-v3, % | Whisper large-v3-turbo, % | Метрика | Источник |
|---|---:|---:|---:|---|---|
| en | 4,2 | н/д | н/д | WER | [S5] |
| de | 4,5 | н/д | н/д | WER | [S5] |
| fr | 8,3 | н/д | н/д | WER | [S5] |
| es | 3,0 | н/д | н/д | WER | [S5] |
| zh | 14,7 | н/д | н/д | CER | [S5] |
| ru | 5,6 | н/д | н/д | WER | [S5] |

Почему нет чисел `large-v3` и `turbo`:

- OpenAI в анонсе `large-v3` пишет, что модель снижает ошибки на 10-20% относительно `large-v2`, но даёт только график [S6].
- В анонсе `turbo` сказано, что по языкам `turbo` работает примерно как `large-v2`, с более заметной деградацией на отдельных языках (тайский, кантонский) [S7].
- Поэтому `large-v2` - проверяемый табличный прокси для `turbo`, но не для `large-v3`. Нельзя выдавать эти числа за `large-v3`.
- Размеры: `large-v2` и `large-v3` - 1550M, `turbo` - 809M [S8].

Условия публикации Whisper:

- Оценка в статье Whisper - beam search с 5 лучами и temperature fallback [S5]. Это не greedy.
- В `openai/whisper` `model.transcribe` по умолчанию использует beam size 1, а `faster-whisper` - beam size 5 [S17]. Один и тот же чекпойнт даёт разные WER в зависимости от обвязки.
- Железо в статье и анонсах не указано - `н/д`.
- FLEURS - чистая студийная речь одного диктора; для встреч и шума это оптимистичная оценка.
- Код и чекпойнты Whisper: [S4].

---

## 2. Диаризация

### 2.1. DER на стандартных наборах

Все числа - DER в процентах. Для AMI, AISHELL-4, AliMeeting и VoxConverse условия различаются; их нельзя смешивать без проверки аннотаций и collar.

| Набор (язык, канал) | pyannote 3.1 [S9] | pyannote community-1 [S10] | senko [S11][S12] | DiariZen v2 [S14][S15] | 3D-Speaker [S16] |
|---|---:|---:|---:|---:|---:|
| AMI IHM (en, headset mix) | 18,8 | 17,0 | 26,5 * | - | - |
| AMI SDM (en, array1 ch1) | 22,4 | 19,9 | - | 13,9 | 21,76 |
| AISHELL-4 (zh, mono mix) | 12,2 | 11,7 | 13,3 | 10,1 | 10,30 |
| AliMeeting (zh, ch1 / far) | 24,4 | 20,3 | - | 10,8 | 19,73 |
| VoxConverse v0.3 (en, single channel) | 11,3 | 11,2 | 13,5 | 9,1 | 11,75 |

`*` Число senko для AMI-IHM получено на аннотациях OpenBench (`argmaxinc/ami-openbench`, tighter forced-alignment), а не на `BUTSpeechFIT/AMI-diarization-setup` с `only_words`. OpenBench прямо предупреждает, что результаты на исходных аннотациях AMI с этими числами не сравнимы [S13]. Поэтому 26,5 нельзя ставить рядом с 18,8 или 17,0 как «senko хуже pyannote».

### 2.2. Условия сравнения

| Система | collar | skip_overlap | Железо | Размер модели | Доступ и лицензия | Источник |
|---|---:|---|---|---|---|---|
| pyannote 3.1 | 0 | False (перекрытия оцениваются) | н/д | н/д (сегментация + эмбеддинги) | MIT, HF gated | [S9] |
| pyannote community-1 | 0 | False | н/д | н/д (сегментация + эмбеддинги) | CC-BY-4.0, HF gated | [S10] |
| senko | 0 | False | RTX 5090 + Ryzen 9 9950X; Apple M3 | н/д; эмбеддинги CAM++ 7,2M, VAD отдельно | MIT по `pyproject.toml`; сторонние компоненты - отдельные лицензии | [S11][S12][S16] |
| DiariZen v2 | 0 | н/д (в README не указано) | н/д | 63,3M после прунинга (WavLM Large 316,6M → 63,3M; 3,8G MAC/s) | Код MIT; веса CC BY-NC 4.0 | [S14][S15] |
| 3D-Speaker | н/д | н/д | н/д | н/д (пайплайн; CAM++ 7,2M - только эмбеддинги) | Apache-2.0 | [S16] |

Что важно в условиях:

- **pyannote 3.1.** Полностью автоматический режим: без ручного VAD, без ручного числа спикеров, без подстройки гиперпараметров под набор. DER «Full»: без collar, перекрытия оцениваются. AMI - headset mix и `only_words`; AMI SDM - array1, channel 1, `only_words`; AliMeeting - channel 1 [S9].
- **pyannote community-1.** Тот же «Full»-режим: без collar, без skipping overlap. Улучшены назначение и подсчёт спикеров. Числа в таблице обновлены в 2025-09 [S10]. В колонке `legacy` у community-1 некоторые значения отличаются от карточки 3.1 на 0,1-0,3 п.п. (AMI SDM 22,7 против 22,4; AliMeeting 24,5 против 24,4; VoxConverse 11,2 против 11,3). Это следствие обновления протокола, а не ошибка; для 3.1 берём карточку 3.1.
- **senko.** Оценка через OpenBench, `collar=0.0`, `skip_overlap=False`. Числа в таблице - CPU-кластеризация; на GPU-кластеризации AMI-IHM 26,6, VoxConverse 17,7; на Apple M3 AMI-IHM 27,5, VoxConverse 13,9, AISHELL-4 13,6 [S12]. AMI-аннотации отличаются от pyannote, см. сноску `*`.
- **DiariZen.** DER без collar, без domain adaptation, с одними и теми же гиперпараметрами кластеризации для всех наборов. AISHELL-4 приведён к моно, AliMeeting - far. Модель обучена на AMI, AISHELL-4, AliMeeting, NOTSOFAR-1, MSDWild, DIHARD3, RAMC и VoxConverse [S14][S15]. Это in-domain, а не zero-shot.
- **3D-Speaker.** В README опубликована таблица DER, но collar, skip_overlap, железо и точная конфигурация пайплайна не указаны. В той же таблице колонка `DiariZen_WavLM` даёт VoxConverse 28,39 - это устаревший результат, не совпадающий с текущим README DiariZen (9,1-9,2) [S14]. Колонку `DiariZen_WavLM` из таблицы 3D-Speaker использовать нельзя.
- **pyannote** (сегментация) обучалась в том числе на AISHELL, AliMeeting, AMI и VoxConverse [S18]; **DiariZen** - на AMI, AISHELL-4, AliMeeting и VoxConverse [S15]. Их DER на этих наборах - in-domain. Состав обучения 3D-Speaker в README не раскрыт. Это не доказательство zero-shot обобщения на русские встречи.

---

## 3. Отличия методик

| Аспект | ASR-публикации | Диаризация-публикации | Что это значит для Podtekst |
|---|---|---|---|
| Метрика | WER; для zh/ja - CER | DER = (miss + false alarm + confusion) / reference speech | WER и DER нельзя пересчитывать друг в друга; сравниваем только одинаковые метрики |
| Нормализация текста | нижний регистр, удаление пунктуации, числа словами или цифрами - по-разному | не применимо | Наши NFKC, `ё → е`, удаление пунктуации могут дать другой WER; фиксируем правила до прогона |
| Сплит | GigaAM - validation; Whisper FLEURS - test; OpenSTT - подмножества | AMI/AISHELL/AliMeeting/VoxConverse - официальные test, но senko - OpenBench-сплиты | Сравниваем только тот же сплит; «validation vs test» даёт смещение |
| Декодирование | GigaAM - не указано; Whisper - beam 5 + temperature fallback; CLI-обвязки - beam 1 или 5 | не применимо | Наш greedy/int8 - другой режим; для честного сравнения нужен тот же beam и fp32/fp16 |
| Квантование | опубликованные числа - fp32/fp16; int8 не заявлен | обычно fp32/fp16 | Наш int8 может дать небольшой минус; отдельно измеряем fp32 vs int8 |
| collar / overlap | не применимо | pyannote и senko - collar 0, overlap оценивается; DiariZen - collar 0, overlap не указан; 3D - н/д | Наш основной режим collar 0 / skip_overlap False совпадает с pyannote; для сравнения с публикациями дополнительно считаем collar 0,25 |
| Аннотации | эталонный текст | AMI `only_words` vs OpenBench forced alignment; AISHELL-4 mono; AliMeeting ch1 | Разные RTTM дают разный DER; нельзя смешивать числа из разных аннотаций |
| Обучение | GigaAM - русский; Whisper - 5 млн ч, zero-shot по FLEURS | pyannote и DiariZen обучались на части тестовых наборов; состав 3D-Speaker не раскрыт | DER на этих наборах - in-domain; для русских встреч внешнего baseline нет |
| Железо | у GigaAM/Whisper не указано | у senko указано; у остальных - н/д | RTF и стоимость сравниваем только на своей эталонной машине |

---

## 4. Как цели ТЗ выглядят на фоне публикаций

Это не оценка Podtekst, а проверка, насколько цели из `OPEN-DATASETS-TZ.md` амбициозны.

**ASR.** Цели ТЗ (Crowd ≤8%, Farfield ≤16%, Common Voice ≤15%) заметно мягче опубликованных GigaAM v3 RNNT (2,4%, 3,9%, 0,9%). Это нормальный порог приёмки первой версии, но не «уровень GigaAM». Если Podtekst уложится в 8/16/15, он всё ещё будет в разы хуже опубликованной GigaAM; если захочет приблизиться, придётся разбираться с VAD, нарезкой и квантованием.

**Диаризация.** Цель AMI IHM ≤20% близка к pyannote 3.1 (18,8%) и community-1 (17,0%). Цели AMI SDM ≤30%, AISHELL-4 ≤16%, AliMeeting ≤28%, VoxConverse ≤16% на 3,6-7,6 п.п. мягче pyannote 3.1. То есть ТЗ требует «подойти к сильному baseline», а не «побить SOTA». Это стоит проговорить в отчёте, чтобы 16% на VoxConverse не читались как победа над 11,3%.

---

## 5. Что можно сравнивать с Podtekst, а что нельзя

1. **Можно:** GigaAM v3 RNNT/CTC на Golos Crowd, Golos Farfield и Common Voice 19 - при условии того же сплита и той же нормализации текста. Опубликованные числа GigaAM - validation, наш прогон - официальный test; это надо писать в отчёте, а не замалчивать.
2. **Можно:** наш WER на Golos/Common Voice сравнивать с GigaAM в двух режимах - fp32 и int8. Только так отделяется ошибка квантования от ошибки пайплайна (VAD, нарезка, склейка). Публикация не сообщает ни квантование, ни железо.
3. **Нельзя:** сравнивать наш greedy/int8 Whisper с опубликованными FLEURS-числами Whisper напрямую. Публикация - beam 5 + temperature fallback, CLI-обвязки по умолчанию - beam 1 или 5. Нужен отдельный прогон beam 5 / fp16, иначе разница будет в основном методической.
4. **Нельзя:** выдавать `large-v2` за `large-v3` или `turbo`. Официальной поязыковой таблицы для `large-v3`/`turbo` нет; `large-v2` - только прокси, и то для `turbo`, а не для `large-v3`. Для Podtekst это значит: FLEURS-числа Whisper - ориентир, а не цель «догнать large-v3».
5. **Можно:** pyannote 3.1 и community-1 на AMI IHM/SDM, AISHELL-4, AliMeeting и VoxConverse сравнивать с нашим DER, если мы держим `collar=0`, `skip_overlap=False` и те же RTTM (AMI `only_words`, AliMeeting ch1, AISHELL-4 mono). Для AMI IHM наша цель ≤20% близка к 18,8% у 3.1 и 17,0% у community-1; для остальных наборов цели ТЗ на 3,6-7,6 п.п. мягче.
6. **Нельзя:** ставить senko AMI-IHM 26,5% рядом с pyannote 18,8% как «senko хуже». У senko другие AMI-аннотации (OpenBench forced alignment). VoxConverse и AISHELL-4 у senko ближе к сопоставимым, но всё равно это OpenBench-сплиты; используем senko как отдельный ориентир, а не как прямого конкурента pyannote.
7. **Нельзя:** считать pyannote и DiariZen zero-shot baseline. Они обучались на части этих наборов; DER 9-14% у DiariZen - это in-domain результат. Состав обучения 3D-Speaker в README не раскрыт. Для русской диаризации внешнего опубликованного baseline на встречах нет: `RuMeet-Synth` синтетический, сравнивать его можно только между версиями Podtekst.
8. **Нельзя:** выбирать модель только по DER. Веса DiariZen - CC BY-NC 4.0, то есть не для коммерческого продукта; pyannote 3.1/community-1 - HF gated; 3D-Speaker - Apache-2.0. Лицензия и доступ входят в решение не меньше, чем DER.

---

## Источники

Все ссылки проверены 09.11.2026. Если карточка или README обновились, числа могли измениться.

- [S1] GigaAM-v3, модель-карточка Hugging Face: https://huggingface.co/ai-sage/GigaAM-v3
- [S2] GigaAM, `evaluation.md` (таблицы WER): https://github.com/salute-developers/GigaAM/blob/main/evaluation.md
- [S3] GigaAM, статья InterSpeech 2025: https://arxiv.org/abs/2506.01192
- [S4] OpenAI Whisper, репозиторий: https://github.com/openai/whisper
- [S5] Whisper, статья arXiv:2212.04356, Appendix D.2.4 (FLEURS, large-v2): https://arxiv.org/abs/2212.04356
- [S6] Whisper `large-v3`, анонс (график, не таблица): https://github.com/openai/whisper/discussions/1762
- [S7] Whisper `turbo`, анонс (график, не таблица): https://github.com/openai/whisper/discussions/2363
- [S8] `openai/whisper-large-v3-turbo`, модель-карточка (809M; large-v3 1550M): https://huggingface.co/openai/whisper-large-v3-turbo
- [S9] `pyannote/speaker-diarization-3.1`, модель-карточка и benchmark: https://huggingface.co/pyannote/speaker-diarization-3.1
- [S10] `pyannote/speaker-diarization-community-1`, модель-карточка и benchmark: https://huggingface.co/pyannote/speaker-diarization-community-1
- [S11] senko, README: https://github.com/narcotic-sh/senko
- [S12] senko, `evaluation/README.md` (OpenBench, collar=0, skip_overlap=False): https://github.com/narcotic-sh/senko/blob/main/evaluation/README.md
- [S13] OpenBench, `BENCHMARKS.md` (оговорка про AMI-аннотации): https://github.com/argmaxinc/OpenBench/blob/main/BENCHMARKS.md
- [S14] DiariZen, README и benchmark: https://github.com/BUTSpeechFIT/DiariZen
- [S15] `BUT-FIT/diarizen-wavlm-large-s80-md-v2`, модель-карточка (63,3M после прунинга): https://huggingface.co/BUT-FIT/diarizen-wavlm-large-s80-md-v2
- [S16] 3D-Speaker, README и таблица DER: https://github.com/modelscope/3D-Speaker
- [S17] faster-whisper, README (beam size 1 у openai/whisper, 5 у faster-whisper): https://github.com/SYSTRAN/faster-whisper
- [S18] `pyannote/segmentation-3.0`, модель-карточка (состав обучающих наборов): https://huggingface.co/pyannote/segmentation-3.0

[S1]: https://huggingface.co/ai-sage/GigaAM-v3
[S2]: https://github.com/salute-developers/GigaAM/blob/main/evaluation.md
[S3]: https://arxiv.org/abs/2506.01192
[S4]: https://github.com/openai/whisper
[S5]: https://arxiv.org/abs/2212.04356
[S6]: https://github.com/openai/whisper/discussions/1762
[S7]: https://github.com/openai/whisper/discussions/2363
[S8]: https://huggingface.co/openai/whisper-large-v3-turbo
[S9]: https://huggingface.co/pyannote/speaker-diarization-3.1
[S10]: https://huggingface.co/pyannote/speaker-diarization-community-1
[S11]: https://github.com/narcotic-sh/senko
[S12]: https://github.com/narcotic-sh/senko/blob/main/evaluation/README.md
[S13]: https://github.com/argmaxinc/OpenBench/blob/main/BENCHMARKS.md
[S14]: https://github.com/BUTSpeechFIT/DiariZen
[S15]: https://huggingface.co/BUT-FIT/diarizen-wavlm-large-s80-md-v2
[S16]: https://github.com/modelscope/3D-Speaker
[S17]: https://github.com/SYSTRAN/faster-whisper
[S18]: https://huggingface.co/pyannote/segmentation-3.0
