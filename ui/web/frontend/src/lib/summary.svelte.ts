import { baseName, encPath, readJsonSafe } from './api'
import type {
  SummaryBlockReason,
  SummaryBusy,
  SummaryCancelResponse,
  SummaryCapabilities,
  SummaryClassifyResponse,
  SummaryClassification,
  SummaryDetail,
  SummaryGenerateResponse,
  SummaryLimits,
  SummaryModelInfo,
  SummaryPhase,
  SummaryPreset,
  SummaryPresetsResponse,
  SummaryRecord,
  SummaryResponse,
  SummarySettings,
  SummarySettingsResponse,
  SummaryStateName,
} from './api'
import { closeStreamIfIdle, downloadModel, engine, ensureStream, transcript } from './transcript.svelte.ts'
import { emit, on } from './ui.svelte.ts'
import { copyText, toast, toastConfirm } from './toast.svelte.ts'
import { buildComposedMarkdown, downloadMarkdown } from './export.ts'
import {
  summaryErrorText,
  summaryExpectedError,
  summaryPreset,
  summaryStageText,
} from './summaryUi.ts'

export const summaryState = $state({
  file: '',
  phase: 'idle' as SummaryPhase,
  expanded: false,
  presets: [] as SummaryPreset[],
  classification: null as SummaryClassification | null,
  chosenPresetId: '',
  pendingPresetId: '',
  manualChoice: false,
  record: null as SummaryRecord | null,
  sourceRev: '',
  stale: false,
  generating: false,
  jobId: '',
  jobFile: '',
  stage: '',
  pct: 0,
  elapsedSec: 0,
  note: '',
  notice: '',
  error: null as { code: string; message: string; scope: 'load' | 'generate' } | null,
  model: null as SummaryModelInfo | null,
  settings: null as SummarySettings | null,
  busy: null as SummaryBusy | null,
  blockReason: null as SummaryBlockReason | null,
  stateName: 'missing' as SummaryStateName,
  capabilities: null as SummaryCapabilities | null,
  limits: null as SummaryLimits | null,
})

class SummaryApiError extends Error {
  code: string
  status: number
  constructor(code: string, status: number, message?: string) {
    super(message || code)
    this.code = code
    this.status = status
  }
}

type DevModule = typeof import('./summary.dev.ts')
let devModule: DevModule | null | undefined
let mockMode = false
let loadSeq = 0
let abort: AbortController | null = null
let ticker: ReturnType<typeof setInterval> | null = null

async function devMock(): Promise<DevModule | null> {
  if (devModule !== undefined) return devModule
  if (
    !import.meta.env.DEV ||
    typeof window === 'undefined' ||
    !new URLSearchParams(window.location.search).has('summary_mock')
  ) {
    devModule = null
    return null
  }
  devModule = await import('./summary.dev.ts')
  return devModule
}

async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  const dev = await devMock()
  mockMode = Boolean(dev)
  if (dev) return (await dev.request(path, init)) as T
  const r = await fetch(path, init)
  let d: unknown
  try {
    d = await readJsonSafe(r)
  } catch {
    throw new SummaryApiError(r.status === 404 ? 'internal' : 'internal', r.status)
  }
  const body = d as { ok?: boolean; code?: string; error?: string }
  if (!r.ok || body?.ok === false) {
    throw new SummaryApiError(body?.code || `http_${r.status}`, r.status, body?.error)
  }
  return body as T
}

function stopTicker(): void {
  if (ticker) clearInterval(ticker)
  ticker = null
}

function startTicker(): void {
  stopTicker()
  ticker = setInterval(() => {
    if (!summaryState.generating) return stopTicker()
    summaryState.elapsedSec += 1
    // Терминальное SSE-событие могло потеряться при переподключении:
    // раз в 3 секунды сверяемся с сохранённым состоянием.
    if (summaryState.elapsedSec % 3 === 0 && summaryState.file) {
      void loadSummary(summaryState.file, { silent: true, openResult: true })
    }
  }, 1000)
}

function finishGenerating(): void {
  summaryState.generating = false
  engine.summaryRunning = false
  stopTicker()
  if (!mockMode) closeStreamIfIdle()
}

function resetForFile(): void {
  stopTicker()
  summaryState.phase = 'loading'
  summaryState.expanded = true
  summaryState.record = null
  summaryState.classification = null
  summaryState.chosenPresetId = ''
  summaryState.pendingPresetId = ''
  summaryState.manualChoice = false
  summaryState.stale = false
  summaryState.generating = false
  summaryState.jobId = ''
  summaryState.jobFile = ''
  summaryState.stage = ''
  summaryState.pct = 0
  summaryState.elapsedSec = 0
  summaryState.note = ''
  summaryState.notice = ''
  summaryState.error = null
  summaryState.busy = null
  summaryState.blockReason = null
  summaryState.stateName = 'missing'
  summaryState.sourceRev = transcript.rev
}

function applyResponse(d: SummaryResponse, opts: { openResult?: boolean }): void {
  const wasGenerating = summaryState.generating
  summaryState.settings = d.settings
  summaryState.model = d.model
  summaryState.capabilities = d.capabilities
  summaryState.limits = d.limits
  summaryState.busy = d.busy
  summaryState.blockReason = d.block_reason
  summaryState.stateName = d.state
  summaryState.sourceRev = transcript.rev || d.source_rev || summaryState.sourceRev
  summaryState.record = d.summary
  summaryState.classification = d.classification
  summaryState.note = ''

  if (d.active && (!d.active.file || d.active.file === summaryState.file)) {
    summaryState.phase = 'generating'
    summaryState.expanded = true
    summaryState.generating = true
    summaryState.jobId = d.active.job_id
    summaryState.jobFile = d.active.file || summaryState.file
    summaryState.stage = summaryStageText(d.active.phase, d.active.chunk, d.active.chunks)
    summaryState.pct = Math.max(0, Math.min(100, Number(d.active.pct) || 0))
    summaryState.elapsedSec = Math.max(0, Number(d.active.elapsed_sec) || 0)
    engine.summaryRunning = true
    if (!mockMode) ensureStream()
    startTicker()
    return
  }

  if (d.active) {
    // Идёт сборка для другой записи: поток и глобальный флаг не трогаем.
    summaryState.generating = false
    summaryState.jobId = ''
    summaryState.jobFile = d.active.file || ''
    stopTicker()
    engine.summaryRunning = true
    if (!mockMode) ensureStream()
  } else {
    finishGenerating()
  }
  if (d.summary) {
    summaryState.phase = 'result'
    summaryState.notice = ''
    summaryState.stale = d.summary.source_rev !== (d.source_rev || summaryState.sourceRev)
    summaryState.expanded = opts.openResult ?? false
    summaryState.chosenPresetId = d.summary.preset_id
    summaryState.manualChoice = d.classification?.user_override === true
    return
  }

  summaryState.phase = 'suggest'
  summaryState.expanded = d.settings.auto_suggest !== false
  summaryState.stale = false
  if (wasGenerating) summaryState.notice = 'Сборка остановлена.'
  const suggested =
    d.capabilities.classifier === 'heuristic' && d.settings.auto_suggest
      ? d.classification?.preset_id || ''
      : ''
  summaryState.chosenPresetId = suggested || d.settings.default_preset || ''
  summaryState.manualChoice = false
}

export async function loadSummary(
  file: string,
  opts: { openResult?: boolean; silent?: boolean } = {},
): Promise<void> {
  if (!file) {
    summaryState.file = ''
    summaryState.phase = 'idle'
    return
  }
  const id = ++loadSeq
  abort?.abort()
  abort = new AbortController()
  const freshFile = summaryState.file !== file
  summaryState.file = file
  if (freshFile) resetForFile()
  else summaryState.sourceRev = transcript.rev || summaryState.sourceRev
  if (!opts.silent && !summaryState.record && summaryState.phase !== 'generating') {
    summaryState.phase = 'loading'
  }
  void loadSummaryPresets()
  try {
    const d = await apiJson<SummaryResponse>(`/api/summary?file=${encPath(file)}`, {
      signal: abort.signal,
    })
    if (id !== loadSeq) return
    applyResponse(d, opts)
  } catch (e) {
    if (id !== loadSeq || (e as Error)?.name === 'AbortError') return
    if (summaryState.generating) {
      summaryState.note = 'Не удалось обновить состояние сборки. Повторяю…'
      return
    }
    summaryState.phase = 'error'
    summaryState.error = {
      code: (e as SummaryApiError).code || 'internal',
      message: summaryExpectedError((e as SummaryApiError).code || 'internal'),
      scope: 'load',
    }
  }
}

export async function loadSummaryPresets(): Promise<void> {
  if (summaryState.presets.length > 0) return
  try {
    const d = await apiJson<SummaryPresetsResponse>('/api/summary/presets')
    if (Array.isArray(d.presets)) summaryState.presets = d.presets
  } catch {
    /* шаблоны догрузятся в настройках или при повторном открытии */
  }
}

export async function loadSummarySettings(): Promise<void> {
  const d = await apiJson<SummarySettingsResponse>('/api/summary/settings')
  summaryState.settings = d.settings
  summaryState.presets = Array.isArray(d.presets) ? d.presets : summaryState.presets
  summaryState.model = d.model
  summaryState.capabilities = d.capabilities
}

export async function saveSummarySettings(patch: Partial<SummarySettings>): Promise<void> {
  const d = await apiJson<{ ok: boolean; settings: SummarySettings }>('/api/summary/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  })
  summaryState.settings = d.settings
  if (summaryState.file && summaryState.phase === 'suggest') {
    await loadSummary(summaryState.file, { silent: true })
  }
}

export async function classifySummary(force = false): Promise<void> {
  if (!summaryState.file || summaryState.capabilities?.classifier !== 'heuristic') return
  const d = await apiJson<SummaryClassifyResponse>('/api/summary/classify', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ file: summaryState.file, force }),
  })
  summaryState.classification = d.classification
  summaryState.sourceRev = d.source_rev || summaryState.sourceRev
}

function detail(): SummaryDetail {
  return summaryState.settings?.detail || 'normal'
}

function presetFor(id: string): SummaryPreset | null {
  return summaryPreset(id, summaryState.presets)
}

export async function pickPreset(id: string): Promise<void> {
  summaryState.manualChoice = true
  summaryState.chosenPresetId = id
  summaryState.error = null
  if (
    engine.running ||
    (summaryState.blockReason &&
      summaryState.blockReason !== 'model_missing' &&
      summaryState.blockReason !== 'model_download')
  ) {
    summaryState.phase = summaryState.record ? 'result' : 'suggest'
    summaryState.expanded = true
    return
  }
  if (summaryState.model && !summaryState.model.is_installed) {
    summaryState.phase = 'suggest'
    summaryState.expanded = true
    return
  }
  const preset = presetFor(id)
  if (preset?.sensitive) {
    summaryState.pendingPresetId = id
    summaryState.phase = 'confirm-sensitive'
    summaryState.expanded = true
    return
  }
  await generateSummary(id, { replace: false })
}

export function confirmSensitive(): void {
  const id = summaryState.pendingPresetId
  if (!id) return
  summaryState.pendingPresetId = ''
  void generateSummary(id, {
    confirmSensitive: true,
    replace: Boolean(summaryState.record),
    force: summaryState.stale,
  })
}

export function cancelSensitive(): void {
  summaryState.pendingPresetId = ''
  summaryState.phase = summaryState.record ? 'result' : 'suggest'
  summaryState.expanded = true
}

export async function generateSummary(
  presetId: string,
  opts: { force?: boolean; confirmSensitive?: boolean; replace?: boolean } = {},
): Promise<void> {
  const file = summaryState.file
  if (!file || summaryState.phase === 'generating') return
  const hardBlock = summaryState.blockReason
  if (
    engine.running ||
    (hardBlock && hardBlock !== 'model_missing' && hardBlock !== 'model_download')
  ) {
    summaryState.phase = summaryState.record ? 'result' : 'suggest'
    summaryState.expanded = true
    return
  }
  if (!summaryState.presets.length) await loadSummaryPresets()
  const preset = presetFor(presetId)
  if (!preset) {
    summaryState.phase = 'error'
    summaryState.error = { code: 'invalid_preset', message: summaryErrorText('invalid_preset'), scope: 'generate' }
    return
  }
  if (preset.sensitive && !opts.confirmSensitive) {
    summaryState.pendingPresetId = presetId
    summaryState.chosenPresetId = presetId
    summaryState.phase = 'confirm-sensitive'
    summaryState.expanded = true
    return
  }
  if (summaryState.record && !opts.replace && !opts.confirmSensitive) {
    toastConfirm(
      `Собрать итоги заново по шаблону «${preset.label}»? Текущий текст заменится.`,
      'Собрать',
      () => {
        void generateSummary(presetId, { force: true, replace: true, confirmSensitive: true })
      },
    )
    return
  }
  if (!summaryState.sourceRev) await loadSummary(file, { silent: true })
  if (!summaryState.sourceRev) {
    summaryState.phase = 'error'
    summaryState.error = {
      code: 'source_rev_required',
      message: summaryErrorText('source_rev_required'),
      scope: 'generate',
    }
    return
  }

  summaryState.chosenPresetId = presetId
  summaryState.error = null
  summaryState.notice = ''
  summaryState.phase = 'generating'
  summaryState.expanded = true
  summaryState.generating = true
  summaryState.jobId = ''
  summaryState.jobFile = file
  summaryState.stage = 'Собираю итоги…'
  summaryState.pct = 0
  summaryState.elapsedSec = 0
  engine.summaryRunning = true
  if (!mockMode) ensureStream()
  startTicker()

  try {
    const d = await apiJson<SummaryGenerateResponse>('/api/summary/generate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        file,
        preset_id: presetId,
        detail: detail(),
        source_rev: summaryState.sourceRev,
        force: opts.force === true,
        ...(preset.sensitive ? { confirm_sensitive: true } : {}),
      }),
    })
    if (d.cached) {
      finishGenerating()
      await loadSummary(file, { openResult: true })
      return
    }
    if (!d.ok || !d.job_id) {
      throw new SummaryApiError('internal', 200, 'нет job_id')
    }
    summaryState.jobId = d.job_id
  } catch (e) {
    finishGenerating()
    summaryState.phase = 'error'
    summaryState.expanded = true
    summaryState.error = {
      code: (e as SummaryApiError).code || 'internal',
      message: summaryErrorText((e as SummaryApiError).code || 'internal', (e as Error).message),
      scope: 'generate',
    }
  }
}

export async function cancelSummary(): Promise<void> {
  if (!summaryState.jobId) return
  summaryState.note = 'Останавливаю сборку…'
  try {
    const d = await apiJson<SummaryCancelResponse>('/api/summary/cancel', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ job_id: summaryState.jobId }),
    })
    if (!d.ok || !d.cancelled) summaryState.note = 'Задача уже завершилась.'
  } catch (e) {
    summaryState.note = 'Не удалось остановить сборку: ' + (e as Error).message
  }
}

export async function retrySummary(): Promise<void> {
  if (
    summaryState.file &&
    (summaryState.error?.code === 'stale_source' ||
      summaryState.error?.code === 'source_rev_required')
  ) {
    await loadSummary(summaryState.file, { silent: true })
  }
  const id =
    summaryState.chosenPresetId ||
    summaryState.record?.preset_id ||
    summaryState.settings?.default_preset ||
    ''
  if (!id) {
    summaryState.phase = 'suggest'
    summaryState.error = null
    summaryState.expanded = true
    return
  }
  const preset = presetFor(id)
  summaryState.error = null
  void generateSummary(id, {
    force: true,
    replace: true,
    confirmSensitive: preset?.sensitive === true,
  })
}

export function updateStaleSummary(): void {
  const id = summaryState.record?.preset_id
  if (!id) return
  const preset = presetFor(id)
  summaryState.error = null
  void generateSummary(id, {
    force: true,
    replace: true,
    confirmSensitive: preset?.sensitive === true,
  })
}

export function showPreviousResult(): void {
  if (!summaryState.record) return
  summaryState.phase = 'result'
  summaryState.error = null
  summaryState.expanded = true
}

export function toggleSummary(): void {
  summaryState.expanded = !summaryState.expanded
}

export function retryLoad(): void {
  if (summaryState.file) void loadSummary(summaryState.file)
}

export async function copySummary(): Promise<void> {
  if (!summaryState.record) return
  if (await copyText(summaryState.record.markdown)) toast('Итоги скопированы')
  else toast('Не удалось скопировать', 'error')
}

export async function exportSummary(transcriptMarkdown: string): Promise<void> {
  const record = summaryState.record
  if (!record) return
  const stem = baseName(summaryState.file).replace(/\.[^.]+$/, '').trim() || 'Запись'
  await downloadMarkdown(
    `${stem} — итоги и расшифровка`,
    buildComposedMarkdown(record.markdown, transcriptMarkdown),
  )
}

export function downloadSummaryModel(): void {
  const model = summaryState.model
  if (!model) return
  void downloadModel(model).then((ok) => {
    if (ok && summaryState.file) void loadSummary(summaryState.file, { silent: true, openResult: true })
  })
}

export function summaryModelPct(): number | null {
  const id = summaryState.model?.id
  if (!id) return null
  const pct = engine.downloading[id]
  return pct === undefined ? null : pct
}

function jobMatches(evt: { job_id?: string; file?: string }): boolean {
  if (!evt.file || evt.file !== summaryState.file) return false
  return !summaryState.jobId || evt.job_id === summaryState.jobId
}

function handleSummaryEvent(evt: unknown): void {
  const e = evt as {
    type?: string
    job_id?: string
    file?: string
    phase?: string
    pct?: number
    chunk?: number
    chunks?: number
    code?: string
  }
  if (!e || typeof e.type !== 'string') return
  if (e.type === 'summary-stage') {
    if (!jobMatches(e)) return
    if (!summaryState.jobId && e.job_id) summaryState.jobId = e.job_id
    if (summaryState.phase !== 'generating') {
      summaryState.phase = 'generating'
      summaryState.generating = true
      summaryState.expanded = true
      startTicker()
    }
    summaryState.stage = summaryStageText(e.phase || '', e.chunk, e.chunks)
    summaryState.pct = Math.max(summaryState.pct, Math.min(100, Number(e.pct) || 0))
    return
  }
  const sameFile = e.file === summaryState.file
  if (e.type === 'summary-done') {
    finishGenerating()
    if (sameFile) {
      const wasOpen = summaryState.expanded
      summaryState.jobId = ''
      void loadSummary(summaryState.file, { openResult: true }).then(() => {
        if (!wasOpen) toast('Итоги готовы')
      })
    } else {
      toast('Итоги готовы')
    }
    return
  }
  if (e.type === 'summary-error') {
    finishGenerating()
    summaryState.jobId = ''
    if (!sameFile) {
      toast(summaryErrorText(e.code || 'internal'), 'error')
      return
    }
    if (e.code === 'cancelled') {
      summaryState.phase = summaryState.record ? 'result' : 'suggest'
      summaryState.expanded = true
      summaryState.notice = 'Сборка остановлена.'
      summaryState.error = null
      summaryState.note = ''
      return
    }
    summaryState.phase = 'error'
    summaryState.expanded = true
    summaryState.note = ''
    summaryState.error = {
      code: e.code || 'internal',
      message: summaryErrorText(e.code || 'internal'),
      scope: 'generate',
    }
  }
}

function markStaleFromTranscript(): void {
  if (transcript.rev) summaryState.sourceRev = transcript.rev
  else summaryState.sourceRev = ''
  if (summaryState.record) {
    summaryState.stale = true
    return
  }
  if (
    summaryState.file &&
    summaryState.capabilities?.classifier === 'heuristic' &&
    summaryState.settings?.auto_suggest !== false
  ) {
    void loadSummary(summaryState.file, { silent: true })
  }
}

// Модульные слушатели регистрируются один раз: панель может перемонтироваться,
// а SSE-события и правки расшифровки обязаны доходить до одного состояния.
on('summary-event', handleSummaryEvent)
on('transcript-edited', markStaleFromTranscript)
on('models-changed', () => {
  if (!summaryState.file) return
  if (summaryState.blockReason === 'model_missing' || summaryState.model?.is_installed === false) {
    void loadSummary(summaryState.file, { silent: true })
  }
})
