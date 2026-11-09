// Production не импортирует: стенд включается только `?summary_mock=1`
// в dev-сборке, пока backend-маршруты /api/summary* ещё не подняты.
import type {
  SummaryClassification,
  SummaryGpuStatus,
  SummaryModelInfo,
  SummaryPreset,
  SummaryRecord,
  SummarySettings,
} from './api'
import { emit } from './ui.svelte.ts'

const PRESETS: SummaryPreset[] = [
  {
    id: 'meeting',
    label: 'Встреча',
    short: 'Рабочая встреча, планёрка, созвон команды.',
    caution: 'Проверьте имена, сроки и формулировки задач.',
    sections: ['brief', 'decisions', 'tasks', 'questions'],
    sensitive: false,
  },
  {
    id: 'scrum',
    label: 'Скрам',
    short: 'Короткая командная встреча: статус, блокеры, план.',
    caution: 'Сверьте статусы и блокеры.',
    sections: ['brief', 'done', 'in_progress', 'blockers', 'decisions', 'tasks'],
    sensitive: false,
  },
  {
    id: 'legal',
    label: 'Суд и юридическая консультация',
    short: 'Заседание, консультация или разбор дела.',
    caution: 'Это черновик, не юридическая консультация. Проверьте нормы, номера и формулировки.',
    sections: ['substance', 'parties', 'facts', 'norms', 'questions', 'next_steps'],
    sensitive: true,
  },
  {
    id: 'lecture',
    label: 'Лекция',
    short: 'Учебное выступление, лекция или объяснение темы.',
    caution: 'Определения и примеры приведены по словам лектора.',
    sections: ['topic', 'concepts', 'structure', 'examples', 'takeaways', 'questions'],
    sensitive: false,
  },
  {
    id: 'psychology',
    label: 'Психологическая сессия',
    short: 'Сессия или доверительный разговор.',
    caution: 'Это заметки о разговоре, не медицинская консультация и не диагноз. Проверьте формулировки.',
    sections: ['topics', 'client_words', 'observations', 'agreements', 'next_session', 'open_topics'],
    sensitive: true,
  },
  {
    id: 'personal',
    label: 'Личные заметки',
    short: 'Монолог или заметка для себя: мысли, выводы, планы.',
    caution: 'Личные заметки. Проверьте, что смысл не искажён.',
    sections: ['about', 'thoughts', 'decisions', 'todo', 'questions'],
    sensitive: false,
  },
  {
    id: 'universal',
    label: 'Универсальный',
    short: 'Если тип разговора непонятен или смешанный.',
    caution: 'Проверьте итоги.',
    sections: ['brief', 'main', 'decisions', 'tasks', 'questions'],
    sensitive: false,
  },
]

const MODEL: SummaryModelInfo = {
  id: 'sum-qwen3-4b-instruct-2507-q4km',
  kind: 'summary',
  title: 'Qwen3-4B Instruct 2507 (итоги, Q4_K_M)',
  version: '2507-q4km-1',
  installed_version: '2507-q4km-1',
  is_installed: true,
  total_size_bytes: 2497281120,
  on_disk_bytes: 2497281120,
  license: 'Apache-2.0',
  license_url: 'https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507',
  source_url: 'https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF',
  ram_mb: 5530,
  required_free_mb: 6656,
  context_tokens: 8192,
  min_system_ram_mb: 12288,
}

let settings: SummarySettings = { auto_suggest: true, default_preset: null, detail: 'normal', gpu: 'auto' }
const mocks: { gpu: SummaryGpuStatus } = {
  gpu: {
    supported: true,
    bundled: false,
    backend: 'vulkan',
    title: 'Vulkan (Intel, AMD, NVIDIA)',
    description: 'Ускорение итогов через Vulkan.',
    pack_id: 'linux-x86_64-vulkan',
    installed: false,
    has_update: false,
    device: 'Mock GPU',
    reason: 'not_installed',
    compressed_bytes: 35871160,
    installed_bytes: 41209848,
    min_free_mb: 256,
    license: 'MIT',
    license_url: 'https://github.com/ggml-org/llama.cpp/blob/master/LICENSE',
    installing: false,
    pct: 0,
    stage: '',
    error: null,
    error_code: null,
    cancelled: false,
  },
}
let record: SummaryRecord | null = null
let seq = 0
let job: { id: string; file: string; phase: string; pct: number; timers: number[]; cancelled: boolean } | null = null

function classification(): SummaryClassification {
  return {
    preset_id: 'meeting',
    confidence: 'high',
    alternatives: ['scrum', 'lecture'],
    signals: ['tasks', 'deadlines', 'decisions', 'multiple_speakers'],
    ambiguous: false,
    style: 'multi-party',
    language: 'ru',
    source_rev: 'mock-rev-1',
    classified_at: new Date().toISOString(),
    heuristic_version: 1,
    spec_version: 'classify-spec.v1',
    user_override: false,
  }
}

function buildMarkdown(preset: SummaryPreset): string {
  return [
    '# Итоги встречи',
    '',
    `**Шаблон:** «${preset.label}» · **Дата:** ${new Date().toLocaleString('ru-RU')} · **Модель:** ${MODEL.title}`,
    '',
    '## Кратко',
    '- Обсудили сроки запуска.',
    '',
    '## Решения',
    '- Запуск переносится на следующую неделю.',
    '',
    '## Задачи',
    '- Подготовить отчёт. — не назначен, до среды',
    '',
    'Итоги собраны локальной моделью. Проверьте имена, сроки и решения.',
  ].join('\n')
}

function buildRecord(presetId: string, file: string): SummaryRecord {
  const preset = PRESETS.find((p) => p.id === presetId) ?? PRESETS[0]
  return {
    id: 'mock-summary-' + seq,
    job_id: job?.id || 'mock-job-' + seq,
    file,
    preset_id: preset.id,
    preset_label: preset.label,
    model_id: MODEL.id,
    model_title: MODEL.title,
    created_at: new Date().toISOString(),
    duration_ms: 38000,
    source_rev: 'mock-rev-1',
    detail: settings.detail,
    language: 'ru',
    output_language: 'ru',
    sections: [
      {
        id: 'brief',
        title: 'Кратко',
        role: 'plain',
        items: [
          {
            text: 'Обсудили сроки запуска.',
            role: 'plain',
          },
        ],
      },
      {
        id: 'decisions',
        title: 'Решения',
        role: 'decision',
        items: [
          {
            text: 'Запуск переносится на следующую неделю.',
            role: 'decision',
          },
        ],
      },
      {
        id: 'tasks',
        title: 'Задачи',
        role: 'task',
        items: [
          {
            text: 'Подготовить отчёт.',
            role: 'task',
            task: {
              assignee: null,
              deadline: 'до среды',
              deadline_source: 'extracted',
            },
          },
        ],
      },
    ],
    markdown: buildMarkdown(preset),
    truncated: false,
    warnings: [],
    schema: 'podtekst.summary/v1',
    schema_minor: 0,
  }
}

function clearJob(): void {
  if (job) for (const timer of job.timers) clearTimeout(timer)
  job = null
}

function emitStage(phase: string, pct: number, chunk?: number, chunks?: number): void {
  if (!job || job.cancelled) return
  job.phase = phase
  job.pct = pct
  emit('summary-event', {
    type: 'summary-stage',
    job_id: job.id,
    file: job.file,
    phase,
    pct,
    ...(chunk ? { chunk, chunks } : {}),
  })
}

function startJob(file: string, presetId: string): { job_id: string; started: boolean } {
  clearJob()
  seq += 1
  const id = 'mock-job-' + seq
  const timers: number[] = []
  job = { id, file, phase: 'load', pct: 0, timers, cancelled: false }
  const later = (ms: number, fn: () => void) => timers.push(window.setTimeout(fn, ms))
  later(120, () => emitStage('load', 8))
  later(600, () => emitStage('prepare', 18))
  later(1200, () => emitStage('map', 45, 1, 2))
  later(2000, () => emitStage('map', 62, 2, 2))
  later(2700, () => emitStage('reduce', 78))
  later(3200, () => emitStage('verify', 92))
  later(3800, () => {
    if (!job || job.cancelled) return
    record = buildRecord(presetId, file)
    const done = { type: 'summary-done', job_id: job.id, file, ok: true, summary_id: record.id }
    clearJob()
    emit('summary-event', done)
  })
  return { job_id: id, started: true }
}

function summaryResponse(file: string): unknown {
  const active = job
    ? {
        job_id: job.id,
        file: job.file,
        phase: job.phase,
        pct: job.pct,
        elapsed_sec: 3,
      }
    : null
  return {
    ok: true,
    state: record ? 'fresh' : 'missing',
    source_rev: 'mock-rev-1',
    summary: record,
    classification: settings.auto_suggest ? classification() : null,
    settings,
    model: MODEL,
    busy: null,
    active,
    capabilities: { summary_supported: true, runtime: 'sidecar', classifier: 'heuristic' },
    limits: {
      max_chars: 180000,
      max_seconds: 7200,
      min_free_ram_mb: 6656,
      min_system_ram_mb: 12288,
      warn_chars: 400,
    },
    block_reason: null,
  }
}

function parseBody(init?: RequestInit): Record<string, unknown> {
  try {
    return JSON.parse(String(init?.body || '{}')) as Record<string, unknown>
  } catch {
    return {}
  }
}

export async function request(path: string, init?: RequestInit): Promise<unknown> {
  const url = new URL(path, 'http://mock.local')
  if (url.pathname === '/api/summary') {
    return summaryResponse(url.searchParams.get('file') || '')
  }
  if (url.pathname === '/api/summary/presets') {
    return { ok: true, catalog_version: 1, presets: PRESETS }
  }
  if (url.pathname === '/api/summary/settings') {
    if ((init?.method || 'GET') === 'POST') {
      const body = parseBody(init)
      if (typeof body.auto_suggest === 'boolean') settings.auto_suggest = body.auto_suggest
      if (body.default_preset === null || typeof body.default_preset === 'string') {
        settings.default_preset = body.default_preset as string | null
      }
      if (body.detail === 'short' || body.detail === 'normal' || body.detail === 'long') {
        settings.detail = body.detail
      }
      if (body.gpu === 'auto' || body.gpu === 'off') settings.gpu = body.gpu
    }
    return {
      ok: true,
      settings,
      presets: PRESETS,
      model: MODEL,
      capabilities: { summary_supported: true, runtime: 'sidecar', classifier: 'heuristic' },
      gpu: mocks.gpu,
    }
  }
  if (url.pathname === '/api/summary/gpu') {
    return { ok: true, ...mocks.gpu }
  }
  if (url.pathname === '/api/summary/gpu/install') {
    mocks.gpu.installed = true
    mocks.gpu.reason = null
    return { ok: true, started: true }
  }
  if (url.pathname === '/api/summary/gpu/cancel') {
    return { ok: true, cancelled: false }
  }
  if (url.pathname === '/api/summary/gpu/remove') {
    mocks.gpu.installed = false
    return { ok: true, ...mocks.gpu }
  }
  if (url.pathname === '/api/summary/classify') {
    return { ok: true, classification: classification(), source_rev: 'mock-rev-1', cache_hit: false }
  }
  if (url.pathname === '/api/summary/generate') {
    const body = parseBody(init)
    return { ok: true, ...startJob(String(body.file || ''), String(body.preset_id || 'meeting')) }
  }
  if (url.pathname === '/api/summary/cancel') {
    if (!job) return { ok: true, cancelled: false }
    const current = job
    current.cancelled = true
    clearJob()
    emit('summary-event', {
      type: 'summary-error',
      job_id: current.id,
      file: current.file,
      code: 'cancelled',
      phase: current.phase,
      retryable: false,
    })
    return { ok: true, cancelled: true }
  }
  return { ok: false, code: 'internal', error: 'mock: unknown route' }
}
