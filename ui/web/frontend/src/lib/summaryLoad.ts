import { encPath } from './api'
import type { SummaryPresetsResponse, SummaryResponse, SummarySettingsResponse } from './api'
import { closeStreamIfIdle, engine, ensureStream, transcript } from './transcript.svelte.ts'
import { on } from './ui.svelte.ts'
import { toast } from './toast.svelte.ts'
import {
  summaryErrorText,
  summaryExpectedError,
  summaryStageText,
} from './summaryUi.ts'
import { apiJson, isMockMode, SummaryApiError } from './summaryApi.ts'
import { summaryState } from './summaryState.svelte.ts'

let loadSeq = 0
let abort: AbortController | null = null
let ticker: ReturnType<typeof setInterval> | null = null

function stopTicker(): void {
  if (ticker) clearInterval(ticker)
  ticker = null
}

export function startTicker(): void {
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

export function finishGenerating(): void {
  summaryState.generating = false
  engine.summaryRunning = false
  stopTicker()
  if (!isMockMode()) closeStreamIfIdle()
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
    if (!isMockMode()) ensureStream()
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
    if (!isMockMode()) ensureStream()
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
