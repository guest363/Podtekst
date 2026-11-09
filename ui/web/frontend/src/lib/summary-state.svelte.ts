import type {
  SummaryBlockReason,
  SummaryBusy,
  SummaryCapabilities,
  SummaryClassification,
  SummaryGpuStatus,
  SummaryLimits,
  SummaryModelInfo,
  SummaryPhase,
  SummaryPreset,
  SummaryRecord,
  SummarySettings,
  SummaryStateName,
} from './api'
import { settings } from './settings.svelte.ts'

type SummaryState = {
  file: string
  phase: SummaryPhase
  expanded: boolean
  presets: SummaryPreset[]
  systemPromptBase: string
  classification: SummaryClassification | null
  chosenPresetId: string
  pendingPresetId: string
  pendingDowngrade: boolean
  manualChoice: boolean
  record: SummaryRecord | null
  sourceRev: string
  stale: boolean
  generating: boolean
  jobId: string
  jobFile: string
  stage: string
  pct: number
  elapsedSec: number
  note: string
  notice: string
  error: { code: string; message: string; scope: 'load' | 'generate' } | null
  model: SummaryModelInfo | null
  gpu: SummaryGpuStatus | null
  settings: SummarySettings | null
  busy: SummaryBusy | null
  blockReason: SummaryBlockReason | null
  stateName: SummaryStateName
  capabilities: SummaryCapabilities | null
  limits: SummaryLimits | null
}

export const summaryState: SummaryState = $state({
  file: '',
  phase: 'idle',
  expanded: false,
  presets: [],
  systemPromptBase: '',
  classification: null,
  chosenPresetId: '',
  pendingPresetId: '',
  pendingDowngrade: false,
  manualChoice: false,
  record: null,
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
  error: null,
  model: null,
  gpu: null,
  settings: null,
  busy: null,
  blockReason: null,
  stateName: 'missing',
  capabilities: null,
  limits: null,
})

export const refineAvailable = (): boolean =>
  settings.summaryFeature !== false &&
  summaryState.capabilities?.summary_supported !== false &&
  Boolean(
    summaryState.model?.is_installed ||
      settings.models.find((m) => m.kind === 'summary')?.is_installed,
  )
