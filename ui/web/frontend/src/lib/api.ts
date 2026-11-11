// Типизированный клиент REST-ядра. Формы ответов зафиксированы здесь;
// страж контракта — Rust-тест restored_endpoints_roundtrip.

export type OkResponse = {
  ok?: boolean
  error?: string
  code?: string
}

export type Person = {
  name: string
  role?: string
  domain?: string
  is_external?: boolean
  pack_id?: string
  pack_name?: string
  pack_enabled?: boolean
}

export type Voicepack = {
  id: string
  name: string
  enabled: boolean
  voiceprints_count?: number
  people_count?: number
  snippets_count?: number
}

export type ModelInfo = {
  id: string
  kind?: string
  title: string
  version?: string
  installed_version?: string
  is_installed: boolean
  has_update?: boolean
  total_size_bytes?: number
  on_disk_bytes?: number
  license?: string
  description?: string
  ram_mb?: number
  required_free_mb?: number
  context_tokens?: number
  native_context_tokens?: number
  min_system_ram_mb?: number
  license_url?: string
  source_url?: string
}

/**
 * Файл с совпавшим размером, но битым содержимым считается скачанным:
 * точную проверку делает только SHA-256 при скачивании.
 */
export const remainingMb = (m: ModelInfo): number => {
  const left = Math.max(0, (m.total_size_bytes || 0) - (m.on_disk_bytes || 0))
  return Math.round(left / 1048576)
}

/** Whisper и CTC — дополнения: без них распознавание и спикеры работают.
 *  Модель итогов не входит в базовый набор и не влияет на его готовность. */
export const isCoreModel = (m: ModelInfo): boolean => {
  if (m.kind === 'summary') return false
  const id = m.id.toLowerCase()
  return !id.includes('whisper') && !id.includes('ctc')
}

export type SummaryPhase =
  | 'idle'
  | 'loading'
  | 'suggest'
  | 'confirm-sensitive'
  | 'generating'
  | 'result'
  | 'error'
export type SummaryStateName = 'missing' | 'fresh' | 'stale' | 'unsupported' | 'corrupt'
export type Confidence = 'high' | 'medium' | 'low'
export type SummaryDetail = 'short' | 'normal' | 'long'
export type Lang = 'ru' | 'en' | 'other'
export type OutLang = 'ru' | 'en'
export type SummaryRole = 'plain' | 'decision' | 'task' | 'status'
export type InlineSegment = { kind: 'text' | 'bold' | 'code'; value: string }

export type TaskMeta = {
  assignee: string | null
  deadline: string | null
  deadline_source: 'model' | 'extracted'
}

export type SummaryItem = {
  text: string
  role: SummaryRole
  task?: TaskMeta
}

export type SummarySection = {
  id: string
  title: string
  role: SummaryRole
  items: SummaryItem[]
}

export type SummaryPresetSection = {
  id: string
  title: string
  role: SummaryRole
}

export type SummaryPreset = {
  id: string
  label: string
  short: string
  caution: string
  sections: string[]
  sections_detail?: SummaryPresetSection[]
  sensitive: boolean
  role?: string
  domain?: string
  prompt_version?: string
  speaker_primary?: string[]
}

export type SummaryModelInfo = ModelInfo & {
  kind: 'summary'
}

export type SummaryClassification = {
  preset_id: string | null
  confidence: Confidence
  alternatives: string[]
  signals: string[]
  ambiguous: boolean
  style: 'monologue' | 'dialogue' | 'multi-party'
  language: Lang
  source_rev: string
  classified_at: string
  heuristic_version: number
  spec_version: string
  user_override: boolean
  cache_hit?: boolean
}

export type SummaryRecord = {
  id: string
  job_id: string
  file: string
  preset_id: string
  preset_label: string
  model_id: string
  model_title: string
  created_at: string
  duration_ms: number
  source_rev: string
  detail: SummaryDetail
  language: Lang
  output_language: OutLang
  sections: SummarySection[]
  markdown: string
  truncated: boolean
  warnings: string[]
  schema: string
  schema_minor: number
}

export type SummaryArtifact = {
  schema: string
  schema_minor: number
  id: string
  generated_at: string
  source: {
    schema: number
    rev: string
    utterances: number
    chars: number
    language: Lang
    duration_sec: number
  }
  classification: {
    preset_id: string | null
    confidence: Confidence | null
    alternatives: string[]
    signals: string[]
    ambiguous: boolean
    style: 'monologue' | 'dialogue' | 'multi-party'
    language: Lang
    classified_at: string
    heuristic_version: number
    spec_version: string
    user_override: boolean
  }
  preset: {
    id: string
    version: number
    label: string
    confirmed_sensitive: boolean
  }
  detail: SummaryDetail
  output_language: OutLang
  generator: {
    runtime: string
    runtime_commit: string
    backend: string
    model_id: string
    model_title: string
    model_sha256: string
    quant: string
    context: number
    threads: number
    temperature: number
    top_p: number
    repeat_penalty: number
    max_tokens: number
    seed: number
    prompt_version: string
    duration_ms: number
    timings_ms: {
      load: number
      prepare: number
      generate: number
      verify: number
      total: number
    }
  }
  summary: {
    sections: SummarySection[]
    warnings: string[]
    truncated: boolean
    coverage: number
    items_total: number
  }
  provenance: {
    mode: string
    chunks: number
    reduce_levels: number
    items_dropped: number
    soft_claims_dropped: number
    parser_retries: number
    parser_recovered: number
    retry_reason: string | null
  }
}

export type SummaryGpuMode = 'auto' | 'off'

export type SummaryGpuStatus = {
  supported: boolean
  bundled: boolean
  backend: string
  title: string
  description: string
  pack_id: string | null
  installed: boolean
  has_update: boolean
  device: string | null
  reason: 'unsupported' | 'not_installed' | 'no_device' | null
  compressed_bytes: number
  installed_bytes: number
  min_free_mb: number
  license: string
  license_url: string
  installing: boolean
  pct: number
  stage: string
  error: string | null
  error_code: string | null
  cancelled: boolean
}

export type SummarySettings = {
  auto_suggest: boolean
  default_preset: string | null
  detail: SummaryDetail
  gpu: SummaryGpuMode
}

export type SummaryCapabilities = {
  summary_supported: boolean
  runtime: 'sidecar' | 'missing'
  classifier: 'heuristic' | 'manual_only'
}

export type SummaryBusy = {
  kind: 'transcribing' | 'recording' | 'summary'
  file?: string
  job_id?: string
}

export type SummaryActive = {
  job_id: string
  file: string
  phase: string
  pct: number
  chunk?: number
  chunks?: number
  elapsed_sec?: number
}

export type SummaryLimits = {
  max_chars: number
  max_seconds: number
  min_free_ram_mb: number
  min_system_ram_mb: number
  warn_chars: number
}

export type SummaryBlockReason =
  | 'model_missing'
  | 'model_download'
  | 'transcribing'
  | 'recording'
  | 'busy_summary'
  | 'unsupported'
  | 'runtime_missing'
  | 'low_memory'
  | 'too_short'
  | 'no_text'

export type SummaryResponse = {
  ok: boolean
  state: SummaryStateName
  source_rev: string
  summary: SummaryRecord | null
  classification: SummaryClassification | null
  settings: SummarySettings
  model: SummaryModelInfo | null
  busy: SummaryBusy | null
  active: SummaryActive | null
  capabilities: SummaryCapabilities
  limits: SummaryLimits
  block_reason: SummaryBlockReason | null
}

export type SummaryPresetsResponse = {
  ok: boolean
  catalog_version: number
  presets: SummaryPreset[]
  system_prompt_base?: string
}

export type SummarySettingsResponse = {
  ok: boolean
  settings: SummarySettings
  presets: SummaryPreset[]
  system_prompt_base?: string
  model: SummaryModelInfo | null
  capabilities: SummaryCapabilities
  gpu: SummaryGpuStatus
}

export type SummaryClassifyResponse = {
  ok: boolean
  classification: SummaryClassification | null
  source_rev: string
  cache_hit?: boolean
}

export type SummaryGenerateResponse = {
  ok: boolean
  job_id?: string
  started?: boolean
  cached?: boolean
  summary_id?: string
}

export type SummaryCancelResponse = {
  ok: boolean
  cancelled: boolean
}

export type SpeakerSample = {
  start: number
  end: number
  dur?: number
  text?: string
  full_text?: string
  channel?: number | null
}

export type SpeakerCandidate = {
  name: string
  similarity?: number
  pack_id?: string
  pack_name?: string
}

export type VoiceprintSummary = {
  name: string
  samples_count: number
  updated_at?: string
}

export type VoicePrototype = {
  id?: string | null
  created_at?: string | null
  duration_sec?: number | null
  quality?: number | null
  has_snippet?: boolean
}

export type VoiceDetail = {
  name?: string | null
  pack_id?: string
  samples_count?: number | null
  prototypes?: VoicePrototype[]
}

export type SpeakerInfo = {
  id?: string
  name: string
  channel?: number | null
  matched?: boolean
  matched_name?: string | null
  matched_pack_id?: string | null
  matched_pack_name?: string | null
  similarity?: number
  silhouette?: number
  candidates?: SpeakerCandidate[] | Array<[string, number]> | null
  embedding?: number[] | null
  sample?: SpeakerSample | null
  samples?: SpeakerSample[] | null
  utterances_count?: number
  total_speech_sec?: number
}

export type WordConf = {
  w: string
  conf: number
  needs_review: boolean
}

export type Utterance = {
  start: number
  end?: number
  speaker?: string
  channel?: number | null
  text?: string
  words?: WordConf[] | null
  ctc_used?: boolean
}

export type TranscriptData = {
  filename?: string
  meta?: Record<string, unknown>
  speakers_info?: SpeakerInfo[]
  utterances?: Utterance[]
  markdown?: string
  rev?: string
}

export type RecordingItem = {
  name: string
  path: string
  is_video: boolean
  size_mb: number
  duration_sec?: number
  date?: string
  has_transcript: boolean
  has_summary?: boolean
  speakers_count?: number
  owned?: boolean
}

export const encPath = (p: string): string => {
  return encodeURIComponent(p)
}

// Без таймаута зависший ответ оставляет UI в «загрузке» навсегда: fetch сам не истекает.
export const fetchWithTimeout = async (
  url: string,
  init: RequestInit = {},
  ms = 15000,
): Promise<Response> => {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), ms)
  try {
    return await fetch(url, { ...init, signal: controller.signal })
  } finally {
    clearTimeout(timer)
  }
}

export const readJsonSafe = async <T = Record<string, unknown>>(res: Response): Promise<T> => {
  const ct = res.headers.get('content-type') || ''
  if (ct.includes('application/json')) return (await res.json()) as T // SAFETY: контракт endpoint, тип задаёт вызывающая сторона
  const text = await res.text()
  throw new Error(`Сервер вернул HTTP ${res.status}: ${text.slice(0, 160)}`)
}

export const baseName = (p: string): string => {
  return String(p).split(/[\\/]/).pop() ?? ''
}

export const isVideoPath = (p: string): boolean => {
  return ['.mp4', '.mkv', '.mov', '.avi', '.webm', '.m4v'].some((ext) =>
    String(p).toLowerCase().endsWith(ext),
  )
}

export const fmtClock = (sec: number): string => {
  const total = Number.isFinite(sec) && sec > 0 ? sec : 0
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = Math.floor(total % 60)
  const mm = String(m).padStart(2, '0')
  const ss = String(s).padStart(2, '0')
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`
}

export const fmtClockShort = (sec: number): string => {
  const s = Math.max(0, Number(sec) || 0)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${(s % 60).toFixed(0).padStart(2, '0')}`
}

export type FormattedRecDate = {
  day: string
  time: string
  full: string
}

export const parseRecDate = (rawDate?: string, rawName?: string): Date | null => {
  if (rawDate) {
    const parts = rawDate.trim().split(/[\sT]/)
    if (parts.length >= 1) {
      const [y, m, d] = parts[0].split('-').map(Number)
      if (!Number.isNaN(y) && !Number.isNaN(m) && !Number.isNaN(d)) {
        return new Date(y, m - 1, d)
      }
    }
  }
  const match = String(rawName).match(/__(\d{2})-(\d{2})-(\d{4})/)
  if (match) {
    const [, d, m, y] = match.map(Number)
    if (!Number.isNaN(y) && !Number.isNaN(m) && !Number.isNaN(d)) {
      return new Date(y, m - 1, d)
    }
  }
  return null
}

export const formatRecDate = (rawDate?: string, rawName?: string): FormattedRecDate => {
  let time = ''
  if (rawDate) {
    const parts = rawDate.trim().split(/[\sT]/)
    if (parts.length >= 2) time = parts[1].slice(0, 5)
  }
  if (!time) {
    const m = String(rawName).match(/(\d{2})[:-](\d{2})/)
    if (m) time = `${m[1]}:${m[2]}`
  }

  const itemDate = parseRecDate(rawDate, rawName)
  if (itemDate) {
    const now = new Date()
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    const yesterday = new Date(today)
    yesterday.setDate(yesterday.getDate() - 1)

    let day = ''
    if (itemDate.getTime() === today.getTime()) {
      day = 'Сегодня'
    } else if (itemDate.getTime() === yesterday.getTime()) {
      day = 'Вчера'
    } else {
      const months = [
        'января',
        'февраля',
        'марта',
        'апреля',
        'мая',
        'июня',
        'июля',
        'августа',
        'сентября',
        'октября',
        'ноября',
        'декабря',
      ]
      const d = itemDate.getDate()
      const m = itemDate.getMonth()
      const y = itemDate.getFullYear()
      const monthName = months[m] || ''
      const yearSuffix = y !== now.getFullYear() ? ` ${y}` : ''
      day = `${d} ${monthName}${yearSuffix}`
    }
    return { day, time, full: time ? `${day}, ${time}` : day }
  }

  return { day: 'Запись', time: '', full: 'Запись' }
}

export const formatRangeLabel = (start: Date, end: Date): string => {
  const now = new Date()
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  if (start.getTime() === end.getTime()) {
    if (start.getTime() === today.getTime()) return 'Сегодня'
    const yesterday = new Date(today)
    yesterday.setDate(yesterday.getDate() - 1)
    if (start.getTime() === yesterday.getTime()) return 'Вчера'
    return `${start.getDate()} ${['янв', 'фев', 'мар', 'апр', 'май', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек'][start.getMonth()]}`
  }
  const fmt = (d: Date) => `${d.getDate()}.${String(d.getMonth() + 1).padStart(2, '0')}`
  return `${fmt(start)}–${fmt(end)}`
}

export const pluralRecords = (n: number): string => {
  const mod10 = n % 10
  const mod100 = n % 100
  if (mod10 === 1 && mod100 !== 11) return 'запись'
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 10 || mod100 >= 20)) return 'записи'
  return 'записей'
}

export const displayRecTitle = (rawName: string): string => {
  const stem = rawName.replace(/\.[^.]+$/, '')
  if (/^\d{2}[:-]\d{2}(?:-\d{2}[:-]\d{2})?__\d{2}-\d{2}-\d{4}$/.test(stem)) {
    return 'Запись встречи'
  }
  return stem
}

// Сбалансированная палитра спикеров (WCAG AA > 4.5:1 к белому фону).
const SPEAKER_PALETTE = [
  '#1e40af',
  '#0f766e',
  '#6b21a8',
  '#9a3412',
  '#374151',
  '#047857',
  '#92400e',
  '#9d174d',
  '#3730a3',
  '#115e59',
  '#7c2d12',
  '#4c1d95',
]

const speakerIndex = (name: string): number => {
  // FNV-1a: стабильно раскладывает имена по палитре без частых совпадений.
  let h = 2166136261
  for (const ch of String(name || '')) {
    h ^= ch.charCodeAt(0)
    h = Math.imul(h, 16777619)
  }
  return (h >>> 0) % SPEAKER_PALETTE.length
}

export const speakerColor = (name: string): string => {
  return SPEAKER_PALETTE[speakerIndex(name)]
}

export const speakerTint = (_name: string): string => {
  return '#ffffff'
}

declare const __IS_MOBILE_BUILD__: boolean

export const isMobile: boolean =
  (typeof __IS_MOBILE_BUILD__ !== 'undefined' && __IS_MOBILE_BUILD__) ||
  (typeof navigator !== 'undefined' && /Android|iPhone|iPad|iPod/i.test(navigator.userAgent))

// Итоги скрываем только там, где их нет в ядре (Android); iOS-браузер
// должен видеть мобильный flow, а не пустой экран.
export const isAndroid: boolean =
  typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)

export const pluralSpeakers = (n: number): string => {
  const mod10 = n % 10
  const mod100 = n % 100
  if (mod10 === 1 && mod100 !== 11) return 'спикер'
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 10 || mod100 >= 20)) return 'спикера'
  return 'спикеров'
}
