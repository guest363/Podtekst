import { readJsonSafe } from './api'
import { toast } from './toast.svelte.ts'

export interface CheckFix {
  kind: string
  target: string
  label: string
}

export interface SystemCheck {
  id: string
  title: string
  status: string
  detail: string
  hint: string
  fix: CheckFix | null
  duration_ms: number
}

export interface ChecksSummary {
  status: string
  ok: number
  warn: number
  error: number
  skip: number
}

export interface DiskInfo {
  path?: string
  total_bytes?: number
  free_bytes?: number
  used_bytes?: number
}

export interface MemoryInfo {
  total_bytes?: number
  available_bytes?: number
  used_bytes?: number
}

export interface RecordingInfo {
  active?: boolean
  elapsed?: number
  mode?: string
}

export interface ModelItem {
  id: string
  kind?: string
  title: string
  version?: string
  installed_version?: string | null
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

export interface ModelsSummary {
  core_ready?: boolean
  installed?: number
  total?: number
  missing_core?: string[]
  updates?: string[]
  items?: ModelItem[]
}

export interface FfmpegStatus {
  available?: boolean
  downloading?: boolean
  pct?: number
  error?: string | null
  manual_url?: string
}

export interface DeviceInfo {
  id?: string
  name?: string
  muted?: boolean
  system?: boolean
}

export interface DevicesInfo {
  platform?: string
  inputs?: DeviceInfo[]
  system?: DeviceInfo[]
  default_input?: string | null
  default_system?: string | null
  has_system_audio?: boolean
}

export interface PermissionInfo {
  id?: string
  name?: string
  granted?: boolean
  detail?: string
}

export interface PermissionsInfo {
  system_audio?: boolean | string
  screen_capture_supported?: boolean | string
  microphone?: boolean | string
  [key: string]: boolean | string | undefined
}

export interface HotkeyInfo {
  accelerator?: string
  registered?: boolean | null
}

export interface IntegrationInfo {
  hotkey?: HotkeyInfo
  autostart?: boolean
  single_instance?: boolean
  ipc?: boolean
  updater?: string
  [key: string]: unknown
}

export interface UpdateSnapshotInfo {
  current?: string
  mode?: string
  install_mode?: string
  self_update?: boolean
  last_check?: { at?: string; latest?: string; is_newer?: boolean; stale?: boolean } | null
  last_check_at?: string
  latest?: string
  skipped_version?: string
  signed?: boolean
}

export interface DirStats {
  count?: number
  last_at?: string
  unacknowledged?: number
  session_count?: number
}

export interface JournalSession {
  id: string
  started_at?: string
  ended_at?: string
  duration_sec?: number
  status?: string
  mode?: string
  channels?: number
  mic?: string
  system?: string
  screen?: boolean
  is_video?: boolean
  file?: { id?: string; name?: string; size_bytes?: number; ext?: string } | string | null
  finalized?: boolean
  warnings?: string[]
  errors?: Array<string | { at?: string; stage?: string; code?: string; message?: string }>
  silent?: boolean
  sys_silent?: boolean
  transcribe?: { status?: string; at?: string } | null
  app_version?: string
}

export interface JournalSummary {
  count?: number
  last_at?: string
  total?: number
  sessions?: JournalSession[]
  truncated?: boolean
}

export interface CrashItem {
  id: string
  created_at?: string
  kind?: string
  synthetic?: boolean
  app_version?: string
  app?: { version?: string; profile?: string }
  thread?: string
  message?: string
  location?: string
  panic?: { message?: string; location?: string }
  backtrace?: string[]
  has_backtrace?: boolean
  log_tail?: string
  acknowledged?: boolean
  acked?: boolean
}

export interface SystemSnapshot {
  version?: string
  os?: string | { name?: string; family?: string; version?: string; arch?: string; kernel?: string }
  paths?: Record<string, string>
  disk?: DiskInfo
  memory?: MemoryInfo
  recording?: RecordingInfo
  models?: ModelsSummary
  ffmpeg?: FfmpegStatus
  devices?: DevicesInfo
  permissions?: PermissionsInfo | PermissionInfo[]
  integration?: IntegrationInfo
  update?: UpdateSnapshotInfo
  crashes?: DirStats
  journal?: JournalSummary
  log?: { path?: string; bytes?: number; modified_at?: string }
}

export const system = $state({
  snapshot: null as SystemSnapshot | null,
  checks: [] as SystemCheck[],
  summary: null as ChecksSummary | null,
  checkedAt: '',
  loading: false,
  checksLoading: false,
  error: '',
  journal: [] as JournalSession[],
  journalTotal: 0,
  journalTruncated: false,
  crashes: [] as CrashItem[],
  unacknowledged: 0,
  crashesLoading: false,
})

export function fmtBytes(n?: number | null): string {
  const v = Number(n) || 0
  if (v <= 0) return ''
  const units = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ']
  let x = v
  let i = 0
  while (x >= 1024 && i < units.length - 1) {
    x /= 1024
    i++
  }
  return `${x >= 10 ? Math.round(x) : x.toFixed(1).replace(/\.0$/, '')} ${units[i]}`
}

export function fmtDateTime(raw?: string | null): string {
  if (!raw) return ''
  const d = new Date(raw)
  if (Number.isNaN(d.getTime())) return String(raw)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getDate())}.${p(d.getMonth() + 1)}.${d.getFullYear()} ${p(d.getHours())}:${p(d.getMinutes())}`
}

export function crashUnacknowledged(c: CrashItem): boolean {
  return c.acknowledged !== true && c.acked !== true
}

export function crashMessage(c: CrashItem): string {
  return c.message || c.panic?.message || ''
}

export function crashLocation(c: CrashItem): string {
  return c.location || c.panic?.location || ''
}

export function crashVersion(c: CrashItem): string {
  return c.app_version || c.app?.version || ''
}

export function crashHasBacktrace(c: CrashItem): boolean {
  return c.has_backtrace === true || (c.backtrace?.length ?? 0) > 0
}

export async function loadSystem(): Promise<void> {
  if (system.loading) return
  system.loading = true
  system.error = ''
  try {
    const d = await readJsonSafe(await fetch('/api/system'))
    if (d.ok === false) throw new Error(String(d.error || 'Не удалось получить состояние системы'))
    const snap = (d.system ?? d.snapshot ?? d) as SystemSnapshot
    system.snapshot = snap
    const crashes = snap.crashes
    if (crashes && typeof crashes.unacknowledged === 'number') {
      system.unacknowledged = crashes.unacknowledged
    }
  } catch (e) {
    system.error = (e as Error).message
  } finally {
    system.loading = false
  }
}

export async function runChecks(probeAudio = true): Promise<void> {
  if (system.checksLoading) return
  system.checksLoading = true
  try {
    const d = await readJsonSafe(
      await fetch('/api/system/check', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ probe_audio: probeAudio, include_optional: true }),
      }),
    )
    if (d.ok === false) throw new Error(String(d.error || 'Проверка не выполнилась'))
    system.checks = (Array.isArray(d.checks) ? d.checks : []).map((c: any) => ({
      id: String(c?.id || ''),
      title: String(c?.title || c?.id || 'Проверка'),
      status: String(c?.status || 'skip'),
      detail: String(c?.detail || ''),
      hint: String(c?.hint || ''),
      fix: c?.fix && typeof c.fix === 'object'
        ? {
            kind: String(c.fix.kind || ''),
            target: String(c.fix.target || ''),
            label: String(c.fix.label || ''),
          }
        : null,
      duration_ms: Number(c?.duration_ms) || 0,
    }))
    const s = d.summary
    system.summary = s && typeof s === 'object'
      ? {
          status: String(s.status || ''),
          ok: Number(s.ok) || 0,
          warn: Number(s.warn) || 0,
          error: Number(s.error) || 0,
          skip: Number(s.skip) || 0,
        }
      : null
    system.checkedAt = String(d.checked_at || '')
  } catch (e) {
    toast('Проверка не выполнилась: ' + (e as Error).message, 'error')
  } finally {
    system.checksLoading = false
  }
}

export async function openTarget(target: string): Promise<void> {
  try {
    const d = await readJsonSafe(
      await fetch('/api/system/open', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ target }),
      }),
    )
    if (d.ok === false) throw new Error(String(d.error || 'Не удалось открыть'))
  } catch (e) {
    toast('Не удалось открыть: ' + (e as Error).message, 'error')
  }
}

export async function loadJournal(limit = 20): Promise<void> {
  try {
    const d = await readJsonSafe(await fetch(`/api/journal?limit=${limit}`))
    if (d.ok === false) throw new Error(String(d.error || 'Журнал недоступен'))
    system.journal = (Array.isArray(d.sessions) ? d.sessions : []) as JournalSession[]
    system.journalTotal = Number(d.total) || system.journal.length
    system.journalTruncated = d.truncated === true
  } catch (e) {
    toast('Журнал не загрузился: ' + (e as Error).message, 'error')
  }
}

export async function loadCrashes(): Promise<void> {
  if (system.crashesLoading) return
  system.crashesLoading = true
  try {
    const d = await readJsonSafe(await fetch('/api/crashes'))
    if (d.ok === false) throw new Error(String(d.error || 'Сбои недоступны'))
    system.crashes = (Array.isArray(d.items) ? d.items : []) as CrashItem[]
    system.unacknowledged = Number(d.unacknowledged) || system.crashes.filter(crashUnacknowledged).length
  } catch (e) {
    toast('Сбои не загрузились: ' + (e as Error).message, 'error')
  } finally {
    system.crashesLoading = false
  }
}

export async function ackCrash(id: string | null = null): Promise<void> {
  try {
    const d = await readJsonSafe(
      await fetch('/api/crashes/ack', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(id ? { id } : { all: true }),
      }),
    )
    if (d.ok === false) throw new Error(String(d.error || 'Не удалось скрыть сбой'))
    if (id) {
      system.crashes = system.crashes.filter((c) => c.id !== id)
      system.unacknowledged = Math.max(0, system.unacknowledged - 1)
    } else {
      system.crashes = []
      system.unacknowledged = 0
    }
  } catch (e) {
    toast('Не удалось скрыть сбой: ' + (e as Error).message, 'error')
  }
}

export async function collectDiagnostics(): Promise<void> {
  try {
    const d = await readJsonSafe(await fetch('/api/diagnostics', { method: 'POST' }))
    if (d.ok === false) throw new Error(String(d.error || 'Не удалось собрать отчёт'))
    if (d.shared === true) {
      toast('Отчёт готов. Выберите, куда его отправить.')
      return
    }
    toast('Отчёт диагностики сохранён: ' + String(d.path || ''))
  } catch (e) {
    toast('Отчёт не собрался: ' + (e as Error).message, 'error')
  }
}
