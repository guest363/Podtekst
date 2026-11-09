import type {
  Confidence,
  SummaryClassification,
  SummaryPreset,
  SummaryRecord,
} from './api'

const PRESET_PHRASE: Record<string, string> = {
  meeting: 'рабочую встречу',
  scrum: 'стендап',
  legal: 'юридический разбор',
  lecture: 'лекцию',
  psychology: 'психологическую сессию',
  personal: 'личные заметки',
  universal: 'смешанный разговор',
}

const PRESET_ACCUSATIVE: Record<string, string> = {
  meeting: 'Встречу',
  scrum: 'Скрам',
  legal: 'Суд и юридическую консультацию',
  lecture: 'Лекцию',
  psychology: 'Психологическую сессию',
  personal: 'Личные заметки',
  universal: 'Универсальный',
}

export function summaryPreset(id: string, presets: SummaryPreset[]): SummaryPreset | null {
  return presets.find((p) => p.id === id) ?? null
}

export function summaryPresetPhrase(id: string): string {
  return PRESET_PHRASE[id] ?? 'разговор'
}

export function summaryPresetAccusative(id: string): string {
  return PRESET_ACCUSATIVE[id] ?? id
}

export function summaryRecommendation(
  classification: SummaryClassification | null,
  presets: SummaryPreset[],
): string {
  if (!classification || !classification.preset_id) {
    return 'Тип разговора не определён. Выберите шаблон.'
  }
  const preset = summaryPreset(classification.preset_id, presets)
  const label = preset?.label ?? classification.preset_id
  const phrase = summaryPresetPhrase(classification.preset_id)
  if (classification.ambiguous) {
    const alt = classification.alternatives[0]
      ? summaryPresetAccusative(classification.alternatives[0])
      : ''
    if (alt) {
      return `Похоже на «${summaryPresetAccusative(classification.preset_id)}» или «${alt}». Выберите шаблон.`
    }
  }
  if (classification.confidence === 'high') {
    return `Похоже на ${phrase}. Сделать итоги по шаблону «${label}»?`
  }
  if (classification.confidence === 'medium') {
    return `Возможно, это ${phrase}. Сделать итоги по шаблону «${label}»?`
  }
  return `Не уверен, но больше похоже на ${phrase}.`
}

export function summaryConfidenceText(confidence: Confidence): string {
  if (confidence === 'high') return 'заметные признаки'
  if (confidence === 'medium') return 'умеренные признаки'
  return 'слабые признаки'
}

const SIGNAL_TEXT: Record<string, string> = {
  tasks: 'есть задачи',
  deadlines: 'названы сроки',
  decisions: 'есть решения',
  multiple_speakers: 'несколько участников',
  single_speaker: 'один говорящий',
  status_round: 'статусы по кругу',
  blockers: 'есть блокеры',
  sprint_terms: 'термины спринта',
  legal_terms: 'термины права',
  procedural_roles: 'процедурные роли',
  lecture_terms: 'учебные термины',
  definitions: 'есть определения',
  therapy_terms: 'темы терапии',
  personal_topics: 'личные темы',
  qa_pattern: 'вопросы и ответы',
  monologue: 'длинный монолог',
  short_turns: 'короткие реплики',
  timing_short: 'короткая запись',
  timing_long: 'длинная запись',
}

export function summarySignals(signals: string[]): string[] {
  return signals
    .filter((s) => s !== 'no_signal')
    .map((s) => SIGNAL_TEXT[s])
    .filter((s): s is string => Boolean(s))
    .slice(0, 4)
}

const WARNING_TEXT: Record<string, string> = {
  short_source: 'В записи мало текста. Итоги могут быть неточными.',
  slow_hardware: 'На этом компьютере сборка может занять больше времени.',
  truncated: 'Не весь текст поместился в модель. Итоги могут быть неполными.',
  coverage_low: 'Часть расшифровки осталась без пунктов итогов.',
  items_dropped_unsupported: 'Пункты без опоры в расшифровке убраны.',
  fact_unverified: 'Часть фактов не подтверждена дословно в расшифровке.',
  soft_claim_dropped: 'Утверждения без прямой опоры в записи убраны.',
  deadline_not_stated: 'Срок не назван: в записи его нет.',
  deadline_recovered: 'Срок взят из расшифровки дословно.',
  assignee_not_stated: 'Исполнитель не назван.',
  decision_without_cue: 'Решение записано без явной формулировки в записи.',
  decision_missed: 'Решение могло не попасть в итоги.',
  output_thin: 'Итоги получились короткими.',
  parser_recovered: 'Часть пунктов восстановлена из свободного текста.',
  output_capped: 'Итоги пришлось сократить.',
  brief_derived: 'Раздел «Кратко» собран из других разделов.',
  stale_source_race: 'Расшифровка изменилась во время сборки.',
  language_mixed: 'В записи смешаны языки. Итоги собраны по-русски.',
  md_write_failed: 'Файл .md не записан, но итоги доступны.',
  injection_suspected: 'Текст, похожий на инструкцию, не попал в итоги.',
  detail_long_short_source: 'Для короткой записи выбрана подробность «Подробно».',
  fallback_extracted: 'Часть задач восстановлена из реплик.',
}

export function summaryWarnings(record: SummaryRecord): string[] {
  const items = record.warnings.map((w) => WARNING_TEXT[w] ?? w)
  if (record.truncated && !record.warnings.includes('truncated')) {
    items.unshift(WARNING_TEXT.truncated)
  }
  return [...new Set(items.filter(Boolean))]
}

const ERROR_TEXT: Record<string, string> = {
  no_file: 'Не указан файл.',
  no_transcript: 'Расшифровка не найдена.',
  outside_save_dir: 'Файл вне папки записей.',
  body_too_large: 'Слишком большой запрос.',
  unknown_job: 'Задача не найдена.',
  disk_low: 'Не хватает места на диске. Освободите место и повторите.',
  confirmation_required: 'Для этого шаблона нужно подтверждение.',
  invalid_preset: 'Неизвестный шаблон.',
  invalid_detail: 'Неизвестная подробность.',
  source_rev_required: 'Не передана версия расшифровки. Обновите страницу.',
  summary_busy: 'Итоги уже собираются для другой записи. Повторите позже.',
  transcription_running: 'Идёт расшифровка. Итоги можно собрать после её окончания.',
  recording_active: 'Идёт запись. Итоги можно собрать после её окончания.',
  stale_source: 'Расшифровка изменилась. Итоги не сохранены. Повторите.',
  classifier_manual_only: 'Рекомендация отключена: недостаточно проверенных данных.',
  low_memory: 'Для модели нужно не меньше 12 ГБ ОЗУ и около 6,5 ГБ свободно. Закройте приложения и повторите.',
  too_short: 'В записи мало текста для итогов.',
  no_text: 'В расшифровке нет текста.',
  schema_unsupported: 'Эта версия не поддерживает формат итогов. Обновите приложение.',
  model_missing: 'Модель итогов не скачана.',
  runtime_missing: 'Локальный движок итогов не установлен. Переустановите приложение из релизной сборки.',
  summary_disabled: 'Итоги отключены в этой сборке.',
  summary_unsupported: 'Итоги на этом устройстве не поддерживаются.',
  gpu_unsupported: 'Ускорение GPU на этой платформе не поддерживается.',
  gpu_bundled: 'Metal уже встроен в приложение.',
  gpu_busy: 'GPU-пакет уже устанавливается. Дождитесь окончания.',
  gpu_summary_busy: 'Идёт сборка итогов. Дождитесь окончания или остановите её.',
  gpu_installing: 'Устанавливается ускорение GPU. Дождитесь окончания.',
  gpu_disk_low: 'Не хватает места на диске для GPU-пакета.',
  gpu_download_failed: 'Не удалось скачать GPU-пакет. Проверьте сеть и повторите.',
  gpu_sha_mismatch: 'Файл GPU-пакета повреждён. Повторите скачивание.',
  gpu_extract_failed: 'Не удалось распаковать GPU-пакет. Повторите скачивание.',
  gpu_probe_failed: 'GPU-пакет установлен, но Vulkan-устройство не найдено. Итоги продолжат работать на CPU.',
  gpu_cancelled: 'Скачивание отменено.',
  gpu_internal: 'Не удалось установить GPU-пакет. Повторите.',
  gpu_remove_failed: 'Не удалось удалить GPU-пакет. Закройте приложение и повторите.',
  model_load: 'Локальный движок не запустился. Повторите сборку.',
  oom: 'Не хватило памяти для модели. Закройте приложения или выберите «Кратко».',
  invalid_output: 'Модель не вернула корректный текст. Попробуйте другой шаблон или «Кратко».',
  output_thin: 'Модель не смогла выделить итоги. Попробуйте «Кратко» или другой шаблон.',
  output_capped: 'Итоги слишком большие для сохранения. Выберите «Кратко».',
  timeout: 'Сборка заняла слишком много времени и остановлена. Попробуйте «Кратко».',
  backend_crashed: 'Локальный движок остановился. Повторите сборку.',
  cancelled: 'Сборка остановлена.',
  internal: 'Внутренняя ошибка итогов. Повторите и проверьте лог.',
}

export function summaryErrorText(code: string, fallback?: string): string {
  return ERROR_TEXT[code] ?? fallback ?? 'Не удалось собрать итоги.'
}

export function summaryExpectedError(code: string): string {
  return ERROR_TEXT[code] ?? 'Не удалось загрузить итоги. Проверьте, что приложение запущено.'
}

function pad(n: number): string {
  return String(n).padStart(2, '0')
}

export function summaryDate(iso?: string | null): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()}, ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

export function summaryShortDate(iso?: string | null): string {
  const full = summaryDate(iso)
  const m = full.match(/^(\d{2}\.\d{2})\.\d{4}, (\d{2}:\d{2})$/)
  return m ? `${m[1]}, ${m[2]}` : full
}

export function summaryDuration(ms?: number | null): string {
  const sec = Math.max(0, Math.round((Number(ms) || 0) / 1000))
  if (sec < 60) return `${sec} с`
  const m = Math.floor(sec / 60)
  const s = sec % 60
  return `${m} мин ${pad(s)} с`
}

export function summaryElapsed(sec: number): string {
  const total = Math.max(0, Math.floor(sec))
  return `${pad(Math.floor(total / 60))}:${pad(total % 60)}`
}

export function summaryStageText(
  phase: string,
  chunk?: number | null,
  chunks?: number | null,
): string {
  if (phase === 'load') return 'Загружаю модель…'
  if (phase === 'prepare') return 'Готовлю текст…'
  if (phase === 'map') {
    if (chunk && chunks) return `Собираю фрагмент ${chunk} из ${chunks}…`
    return 'Собираю фрагменты…'
  }
  if (phase === 'reduce') return 'Свожу итоги…'
  if (phase === 'verify') return 'Проверяю факты…'
  return 'Собираю итоги…'
}
