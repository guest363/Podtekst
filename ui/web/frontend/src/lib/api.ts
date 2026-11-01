// Типизированный клиент REST-ядра. Формы ответов зафиксированы здесь;
// страж контракта — Rust-тест restored_endpoints_roundtrip.

export interface Person {
  name: string
  role?: string
  domain?: string
  is_external?: boolean
}

export interface Voicepack {
  id: string
  name: string
  enabled: boolean
  voiceprints_count?: number
  people_count?: number
  snippets_count?: number
}

export interface ModelInfo {
  id: string
  title: string
  version?: string
  installed_version?: string
  is_installed: boolean
  has_update?: boolean
  total_size_bytes?: number
  license?: string
  description?: string
}

export interface SpeakerSample {
  start: number
  end: number
  dur?: number
  text?: string
  full_text?: string
  channel?: number | null
}

export interface SpeakerCandidate {
  name: string
}

export interface SpeakerInfo {
  id?: string
  name: string
  channel?: number | null
  matched?: boolean
  matched_name?: string | null
  candidates?: SpeakerCandidate[] | Array<[string, number]> | null
  embedding?: number[] | null
  sample?: SpeakerSample | null
  samples?: SpeakerSample[] | null
  utterances_count?: number
  total_speech_sec?: number
}

export interface Utterance {
  start: number
  end?: number
  speaker?: string
  channel?: number | null
  text?: string
}

export interface TranscriptData {
  filename?: string
  meta?: Record<string, unknown>
  speakers_info?: SpeakerInfo[]
  utterances?: Utterance[]
  markdown?: string
}

export interface RecordingItem {
  name: string
  path: string
  is_video: boolean
  size_mb: number
  duration_sec?: number
  date?: string
  has_transcript: boolean
  speakers_count?: number
  owned?: boolean
}

export function encPath(p: string): string {
  return encodeURIComponent(p)
}

export async function readJsonSafe(res: Response): Promise<any> {
  const ct = res.headers.get('content-type') || ''
  if (ct.includes('application/json')) return await res.json()
  const text = await res.text()
  throw new Error(`Сервер вернул HTTP ${res.status}: ${text.slice(0, 160)}`)
}

export function baseName(p: string): string {
  return String(p).split(/[\\/]/).pop() ?? ''
}

export function isVideoPath(p: string): boolean {
  return ['.mp4', '.mkv', '.mov', '.avi', '.webm', '.m4v'].some((ext) =>
    String(p).toLowerCase().endsWith(ext),
  )
}

export function fmtClock(sec: number): string {
  if (!isFinite(sec) || sec < 0) sec = 0
  const h = Math.floor(sec / 3600)
  const m = Math.floor((sec % 3600) / 60)
  const s = Math.floor(sec % 60)
  const mm = String(m).padStart(2, '0')
  const ss = String(s).padStart(2, '0')
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`
}

export function fmtClockShort(sec: number): string {
  const s = Math.max(0, Number(sec) || 0)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${(s % 60).toFixed(0).padStart(2, '0')}`
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
]

function speakerIndex(name: string): number {
  // FNV-1a: стабильно раскладывает имена по палитре без частых совпадений.
  let h = 2166136261
  for (const ch of String(name || '')) {
    h ^= ch.charCodeAt(0)
    h = Math.imul(h, 16777619)
  }
  return (h >>> 0) % SPEAKER_PALETTE.length
}

export function speakerColor(name: string): string {
  return SPEAKER_PALETTE[speakerIndex(name)]
}

export function speakerTint(_name: string): string {
  return '#ffffff'
}

declare const __IS_MOBILE_BUILD__: boolean

export const isMobile: boolean =
  (typeof __IS_MOBILE_BUILD__ !== 'undefined' && __IS_MOBILE_BUILD__) ||
  (typeof navigator !== 'undefined' && /Android|iPhone|iPad|iPod/i.test(navigator.userAgent))

export function pluralSpeakers(n: number): string {
  const mod10 = n % 10
  const mod100 = n % 100
  if (mod10 === 1 && mod100 !== 11) return 'спикер'
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 10 || mod100 >= 20)) return 'спикера'
  return 'спикеров'
}
