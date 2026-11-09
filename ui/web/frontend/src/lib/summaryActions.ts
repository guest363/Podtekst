import { baseName } from './api'
import type {
  SummaryCancelResponse,
  SummaryClassifyResponse,
  SummaryDetail,
  SummaryGenerateResponse,
  SummaryPreset,
  SummarySettings,
} from './api'
import { downloadModel, engine, ensureStream } from './transcript.svelte.ts'
import { copyText, toast, toastConfirm } from './toast.svelte.ts'
import { buildComposedMarkdown, downloadMarkdown } from './export.ts'
import { summaryErrorText, summaryPreset } from './summaryUi.ts'
import { apiJson, isMockMode, SummaryApiError } from './summaryApi.ts'
import { finishGenerating, loadSummary, loadSummaryPresets, startTicker } from './summaryLoad.ts'
import { summaryState } from './summaryState.svelte.ts'

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
  summaryState.pendingDowngrade = false
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
  const allowDowngrade = summaryState.pendingDowngrade
  summaryState.pendingPresetId = ''
  summaryState.pendingDowngrade = false
  void generateSummary(id, {
    confirmSensitive: true,
    allowDowngrade,
    replace: Boolean(summaryState.record),
    force: summaryState.stale || allowDowngrade,
  })
}

export function cancelSensitive(): void {
  summaryState.pendingPresetId = ''
  summaryState.pendingDowngrade = false
  summaryState.phase = summaryState.record ? 'result' : 'suggest'
  summaryState.expanded = true
}

export async function generateSummary(
  presetId: string,
  opts: {
    force?: boolean
    confirmSensitive?: boolean
    replace?: boolean
    allowDowngrade?: boolean
  } = {},
): Promise<void> {
  const file = summaryState.file
  if (!file || summaryState.phase === 'generating') return
  const hardBlock = summaryState.blockReason
  const allowUnsupported = opts.allowDowngrade === true && hardBlock === 'unsupported'
  if (
    engine.running ||
    (hardBlock &&
      hardBlock !== 'model_missing' &&
      hardBlock !== 'model_download' &&
      !allowUnsupported)
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
  summaryState.pendingDowngrade = false
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
  engine.summaryPct = 0
  engine.summaryMsg = 'Собираю итоги…'
  engine.summaryFile = file
  if (!isMockMode()) ensureStream()
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
        // Новее-мажор заменяется только вместе с force.
        force: opts.force === true || opts.allowDowngrade === true,
        allow_downgrade: opts.allowDowngrade === true,
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
  if (summaryState.error?.code === 'schema_unsupported') {
    await rebuildInOldFormat()
    return
  }
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

/** Новее мажор артефакта: единственный путь замены — явные force + allow_downgrade. */
export async function rebuildInOldFormat(): Promise<void> {
  if (!summaryState.presets.length) await loadSummaryPresets()
  const id =
    summaryState.chosenPresetId ||
    summaryState.classification?.preset_id ||
    summaryState.settings?.default_preset ||
    summaryState.record?.preset_id ||
    ''
  if (!id) {
    summaryState.phase = 'suggest'
    summaryState.error = null
    summaryState.expanded = true
    return
  }
  summaryState.chosenPresetId = id
  summaryState.pendingDowngrade = true
  summaryState.error = null
  await generateSummary(id, {
    force: true,
    allowDowngrade: true,
    replace: true,
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
