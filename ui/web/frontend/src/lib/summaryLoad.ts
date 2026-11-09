import type { SummaryPresetsResponse, SummaryResponse, SummarySettingsResponse } from './api'
import { encPath } from './api'
import { settings } from './settings.svelte.ts'
import { apiJson, isMockMode, type SummaryApiError } from './summaryApi.ts'
import { summaryState } from './summaryState.svelte.ts'
import { summaryErrorText, summaryExpectedError, summaryStageText } from './summaryUi.ts'
import { toast } from './toast.svelte.ts'
import { closeStreamIfIdle, engine, ensureStream, transcript } from './transcript.svelte.ts'
import { on } from './ui.svelte.ts'

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
  engine.summaryPct = 0
  engine.summaryMsg = ''
  engine.summaryFile = ''
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
  summaryState.pendingDowngrade = false
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

function summaryReady(d: SummaryResponse): boolean {
  const hard =
    d.block_reason === 'too_short' ||
    d.block_reason === 'unsupported' ||
    d.block_reason === 'runtime_missing' ||
    d.block_reason === 'low_memory'
  const modelMissing =
    d.block_reason === 'model_missing' ||
    d.block_reason === 'model_download' ||
    d.model?.is_installed === false
  return d.capabilities.summary_supported !== false && !hard && !modelMissing
}

function applyResponse(
  d: SummaryResponse,
  opts: { openResult?: boolean; silent?: boolean },
  freshFile: boolean,
): void {
  const wasGenerating = summaryState.generating
  summaryState.settings = d.settings
  summaryState.model = d.model
  summaryState.capabilities = d.capabilities
  summaryState.limits = d.limits
  summaryState.busy = d.busy
  summaryState.blockReason = d.state === 'unsupported' ? 'unsupported' : d.block_reason
  summaryState.stateName = d.state
  summaryState.sourceRev = transcript.rev || d.source_rev || summaryState.sourceRev
  summaryState.record = d.summary
  summaryState.classification = d.classification
  summaryState.note = ''

  if (
    d.active &&
    (d.active.job_id === summaryState.jobId ||
      !d.active.file ||
      sameRecording(d.active.file, summaryState.file))
  ) {
    summaryState.phase = 'generating'
    if (freshFile || !opts.silent) summaryState.expanded = true
    summaryState.generating = true
    summaryState.jobId = d.active.job_id
    summaryState.jobFile = d.active.file || summaryState.file
    summaryState.stage = summaryStageText(d.active.phase, d.active.chunk, d.active.chunks)
    summaryState.pct = Math.max(0, Math.min(100, Number(d.active.pct) || 0))
    summaryState.elapsedSec = Math.max(0, Number(d.active.elapsed_sec) || 0)
    engine.summaryRunning = true
    engine.summaryPct = summaryState.pct
    engine.summaryMsg = summaryState.stage
    engine.summaryFile = summaryState.jobFile
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
    engine.summaryPct = Math.max(0, Math.min(100, Number(d.active.pct) || 0))
    engine.summaryMsg = summaryStageText(d.active.phase, d.active.chunk, d.active.chunks)
    engine.summaryFile = d.active.file || ''
    if (!isMockMode()) ensureStream()
  } else {
    finishGenerating()
  }
  if (d.summary) {
    summaryState.phase = 'result'
    summaryState.notice = ''
    summaryState.stale = d.summary.source_rev !== (d.source_rev || summaryState.sourceRev)
    summaryState.expanded = opts.openResult ?? (freshFile || summaryState.expanded)
    summaryState.chosenPresetId = d.summary.preset_id
    summaryState.manualChoice = d.classification?.user_override === true
    return
  }

  summaryState.phase = 'suggest'
  if (freshFile) {
    summaryState.expanded = summaryReady(d) && d.settings.auto_suggest !== false
  }
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
    applyResponse(d, opts, freshFile)
  } catch (e) {
    if (id !== loadSeq || (e as Error)?.name === 'AbortError') return
    const code = (e as SummaryApiError).code || 'internal'
    if (code === 'summary_disabled' || code === 'summary_unsupported') {
      settings.summaryFeature = false
      summaryState.phase = 'idle'
      return
    }
    if (summaryState.generating) {
      summaryState.note = 'Не удалось обновить состояние сборки. Повторяю…'
      return
    }
    summaryState.phase = 'error'
    summaryState.error = {
      code,
      message: summaryExpectedError(code),
      scope: 'load',
    }
  }
}

export async function loadSummaryPresets(): Promise<void> {
  if (summaryState.presets.length > 0 && summaryState.systemPromptBase) return
  try {
    const d = await apiJson<SummaryPresetsResponse>('/api/summary/presets')
    if (Array.isArray(d.presets)) summaryState.presets = d.presets
    if (d.system_prompt_base) summaryState.systemPromptBase = d.system_prompt_base
  } catch {
    /* шаблоны догрузятся в настройках или при повторном открытии */
  }
}

export async function loadSummarySettings(): Promise<void> {
  const d = await apiJson<SummarySettingsResponse>('/api/summary/settings')
  summaryState.settings = d.settings
  summaryState.presets = Array.isArray(d.presets) ? d.presets : summaryState.presets
  if (d.system_prompt_base) summaryState.systemPromptBase = d.system_prompt_base
  summaryState.model = d.model
  summaryState.capabilities = d.capabilities
  summaryState.gpu = d.gpu
}

function pathKey(file: string): string {
  return file.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '')
}

// Бэкенд отдаёт путь относительно save_dir, а UI открывает запись абсолютным
// путём из /api/recordings. Сравниваем хвост пути, пока job_id ещё неизвестен.
function sameRecording(a?: string | null, b?: string | null): boolean {
  if (!a || !b) return false
  if (a === b) return true
  const x = pathKey(a)
  const y = pathKey(b)
  return x === y || x.endsWith('/' + y) || y.endsWith('/' + x)
}

function jobMatches(evt: { job_id?: string; file?: string }): boolean {
  if (evt.job_id && summaryState.jobId) return evt.job_id === summaryState.jobId
  return sameRecording(evt.file, summaryState.file)
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
  const mine = jobMatches(e)
  if (e.type === 'summary-done') {
    if (!mine) {
      toast('Итоги готовы')
      return
    }
    const wasOpen = summaryState.expanded
    finishGenerating()
    summaryState.jobId = ''
    void loadSummary(summaryState.file, { openResult: true }).then(() => {
      if (!wasOpen) toast('Итоги готовы')
    })
    return
  }
  if (e.type === 'summary-error') {
    if (!mine) {
      toast(summaryErrorText(e.code || 'internal'), 'error')
      return
    }
    finishGenerating()
    summaryState.jobId = ''
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
