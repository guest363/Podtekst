import type { ModelInfo, OkResponse, SpeakerInfo, TranscriptData, Utterance } from './api'
import { baseName, encPath, isCoreModel, isVideoPath, readJsonSafe } from './api'
import { errText, isAbortError } from './guards.ts'
import { loadQueue, type QueueItem, queueState } from './queue.svelte.ts'
import { settings } from './settings.svelte.ts'
import { summaryStageText } from './summary-ui.ts'
import { toast } from './toast.svelte.ts'
import { emit, ui } from './ui.svelte.ts'

type TranscriptEditResponse = OkResponse & TranscriptData
type TranscribeStartResponse = OkResponse & {
  started?: boolean
  queued?: boolean
  position?: number
}
type ModelsResponse = { models?: ModelInfo[] }
type EngineEvent = {
  type?: string
  file?: string
  pct?: number
  stage?: string
  message?: string
  done?: number
  total?: number
  model_id?: string
  ok?: boolean
  error?: string
  queue?: QueueItem[]
  should_prompt_donate?: boolean
  phase?: string
  chunk?: number
  chunks?: number
}

type TranscriptState = {
  speakers: SpeakerInfo[]
  utterances: Utterance[]
  markdown: string
  rev: string
  hasText: boolean
  loadError: string
  extractVisible: boolean
}

export const transcript: TranscriptState = $state({
  speakers: [],
  utterances: [],
  markdown: '',
  rev: '',
  hasText: false,
  loadError: '',
  extractVisible: false,
})

// Единственное состояние движка для всего UI (дека, модалка, настройки):
// ядро обрабатывает файлы по одному, поэтому прогресс и очередь должны
// выглядеть одинаково в любом месте интерфейса.
type EngineState = {
  running: boolean
  summaryRunning: boolean
  file: string
  msg: string
  pct: number
  summaryFile: string
  summaryMsg: string
  summaryPct: number
  downloading: Record<string, number>
}

export const engine: EngineState = $state({
  running: false,
  summaryRunning: false,
  file: '',
  msg: 'Обработка…',
  pct: 0,
  summaryFile: '',
  summaryMsg: '',
  summaryPct: 0,
  downloading: {},
})

let openSeq = 0
let openAbort: AbortController | null = null

export const openTranscriptFile = async (rawPath: string): Promise<void> => {
  const seq = ++openSeq
  openAbort?.abort()
  openAbort = new AbortController()
  transcript.speakers = []
  transcript.utterances = []
  transcript.markdown = ''
  transcript.rev = ''
  transcript.hasText = false
  transcript.loadError = ''
  transcript.extractVisible = isVideoPath(rawPath)

  let d: TranscriptData
  try {
    const r = await fetch(`/api/transcript?file=${encPath(rawPath)}`, {
      signal: openAbort.signal,
    })
    if (seq !== openSeq) return
    d = await readJsonSafe<TranscriptData>(r)
  } catch (e) {
    if (seq !== openSeq || isAbortError(e)) return
    transcript.hasText = false
    transcript.loadError = `Не удалось загрузить расшифровку: ${errText(e)}`
    return
  }
  if (seq !== openSeq) return

  transcript.utterances = d.utterances || []
  transcript.speakers = d.speakers_info || []
  transcript.markdown = d.markdown || ''
  transcript.rev = typeof d.rev === 'string' ? d.rev : ''
  transcript.hasText = Boolean(d.markdown?.trim() || transcript.utterances.length > 0)
}

export const editTranscript = async (
  rawPath: string,
  change: { index?: number; text?: string; speaker?: string; old_speaker?: string },
): Promise<void> => {
  const r = await fetch('/api/transcript/edit', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ file: rawPath, ...change }),
  })
  const d = await readJsonSafe<TranscriptEditResponse>(r)
  if (!r.ok || !d.ok) throw new Error(d.error || `HTTP ${r.status}`)
  transcript.utterances = d.utterances || []
  transcript.speakers = d.speakers_info || []
  transcript.markdown = d.markdown || ''
  transcript.rev = typeof d.rev === 'string' ? d.rev : transcript.rev
  transcript.hasText = Boolean(transcript.markdown.trim() || transcript.utterances.length)
  emit('transcript-edited')
}

export const emptyHint = (): string => {
  // Критерий «ядра» — как в downloadAllCoreModels и app.svelte.
  const missingCore = settings.models.filter((m) => isCoreModel(m) && !m.is_installed)
  const base = ui.currentTranscriptIsVideo
    ? 'Расшифровка ещё не выполнялась. Звук из видео извлекается автоматически.'
    : 'Расшифровка ещё не выполнялась.'
  if (missingCore.length > 0) {
    return `${base} Для работы нужны модели: «Настройки» → «Компоненты».`
  }
  return `${base} Нажмите «Запустить расшифровку».`
}

let evtSource: EventSource | null = null
let finishedAt = 0
let finishing = false

export const closeStreamIfIdle = (): void => {
  if (!engine.running && !engine.summaryRunning && Object.keys(engine.downloading).length === 0) {
    evtSource?.close()
    evtSource = null
  }
}

export const ensureStream = (): void => {
  if (evtSource) return
  if (!engine.running && !engine.summaryRunning && Object.keys(engine.downloading).length === 0)
    return
  evtSource = new EventSource('/api/transcribe_stream')
  evtSource.onmessage = (e: MessageEvent) => handleEngineEvent(e)
  evtSource.onerror = () => {
    // Если движок не работает, нет активных скачиваний и итогов — закрываем
    // стрим, чтобы браузер не крутил бесконечный реконнект каждые 3 секунды.
    if (!engine.running && !engine.summaryRunning && Object.keys(engine.downloading).length === 0) {
      evtSource?.close()
      evtSource = null
      return
    }
    // readyState CONNECTING — браузер переподключается сам, это не ошибка.
    // CLOSED — сервер недоступен окончательно.
    if (evtSource && evtSource.readyState === EventSource.CLOSED) {
      evtSource.close()
      evtSource = null
      if (engine.running) {
        engine.running = false
        toast('Соединение с движком потеряно. Повторите расшифровку.', 'error')
      }
    }
  }
}

// Итог model-done по id: без него исчезновение прогресса неотличимо от успеха.
const downloadOutcome: Record<string, boolean> = {}

const handleEngineEvent = (e: MessageEvent): void => {
  let evt: EngineEvent
  try {
    evt = JSON.parse(e.data) as EngineEvent // SAFETY: контракт событий движка; поля проверяются по месту
  } catch {
    return
  }
  if (typeof evt.type === 'string' && evt.type.startsWith('summary-')) {
    if (evt.type === 'summary-stage') {
      if (!engine.summaryRunning) {
        engine.summaryPct = 0
        engine.summaryMsg = 'Собираю итоги…'
      }
      engine.summaryRunning = true
      engine.summaryFile = String(evt.file || engine.summaryFile || '')
      engine.summaryPct = Math.max(engine.summaryPct, Math.min(100, Number(evt.pct) || 0))
      engine.summaryMsg = summaryStageText(String(evt.phase || ''), evt.chunk, evt.chunks)
    } else if (evt.type === 'summary-done' || evt.type === 'summary-error') {
      engine.summaryRunning = false
      engine.summaryPct = 0
      engine.summaryMsg = ''
      engine.summaryFile = ''
    }
    emit('summary-event', evt)
    // Слушатель итогов снимает engine.summaryRunning синхронно; после
    // терминального события поток можно закрыть, если больше никто не работает.
    if (evt.type === 'summary-done' || evt.type === 'summary-error') closeStreamIfIdle()
    return
  }
  if (evt.type === 'queue') {
    if (Array.isArray(evt.queue)) {
      queueState.items = evt.queue
      queueState.total = queueState.running ? evt.queue.length + 1 : evt.queue.length
    }
    return
  }
  if (evt.type === 'stage' || evt.type === 'progress') {
    if (evt.stage === 'model-download') {
      if (evt.model_id && evt.pct !== undefined) {
        engine.downloading[evt.model_id] = Math.round(evt.pct)
      }
      return
    }
    if (evt.file && evt.file !== engine.file) {
      engine.file = evt.file
      engine.pct = 0
      queueState.currentFile = evt.file
    }
    engine.msg = evt.message || 'Обработка…'
    if (evt.pct !== undefined)
      engine.pct = Math.max(engine.pct, Math.max(5, Math.min(100, evt.pct)))
  } else if (evt.type === 'segment') {
    engine.msg = `Реплик распознано: ${evt.done || 0} из ${evt.total || 0}`
    if (evt.pct !== undefined) engine.pct = evt.pct
  } else if (evt.type === 'done') {
    finishEngine(String(evt.file || engine.file))
  } else if (evt.type === 'model-done') {
    const modelId = String(evt.model_id || '')
    downloadOutcome[modelId] = evt.ok === true
    delete engine.downloading[modelId]
    emit('models-changed')
    const title = settings.models.find((m) => m.id === modelId)?.title || modelId
    if (evt.ok) toast(`Модель «${title}» готова`)
    else toast(`Ошибка модели: ${evt.error || ''}`, 'error')
    closeStreamIfIdle()
  } else if (evt.type === 'stats') {
    if (evt.should_prompt_donate) emit('support-changed')
  } else if (evt.type === 'error') {
    void loadQueue().then(() => {
      if (!queueState.running) {
        engine.running = false
        closeStreamIfIdle()
      }
    })
    toast(`Расшифровка прервана: ${evt.message || 'неизвестная ошибка'}`, 'error')
  }
}

const finishEngine = (donePath: string): void => {
  if (!engine.running || finishing) return
  finishing = true
  void loadQueue()
    .then(() => {
      if (!queueState.running) {
        engine.running = false
        engine.file = ''
        engine.pct = 100
        engine.msg = 'Расшифровка завершена'
        finishedAt = Date.now()
        closeStreamIfIdle()
      }
    })
    .finally(() => {
      finishing = false
    })
  emit('recordings-changed')
  emit('support-changed')
  if (!ui.transcriptOpen) toast(`Расшифровка «${baseName(donePath)}» завершена`)
  if (ui.transcriptOpen && donePath && donePath === ui.currentTranscriptPath) {
    void openTranscriptFile(donePath)
  }
}

export const startTranscription = async (rawPath: string): Promise<void> => {
  try {
    const r = await fetch('/api/transcribe/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        file: rawPath,
        engine: 'auto',
        min_speakers: settings.speakers,
        max_speakers: settings.speakers,
      }),
    })
    const d = await readJsonSafe<TranscribeStartResponse>(r)
    if (!d.ok) throw new Error(d.error || 'движок отклонил запуск')
    if (d.started === false) {
      toast(
        d.queued
          ? `Запись добавлена в очередь (#${d.position})`
          : 'Эта запись уже расшифровывается',
      )
      await loadQueue()
      ensureStream()
      return
    }
  } catch (e) {
    toast(`Не удалось запустить расшифровку: ${errText(e)}`, 'error')
    return
  }
  engine.running = true
  engine.file = rawPath
  engine.msg = 'Подготовка к расшифровке…'
  engine.pct = 5
  await loadQueue()
  ensureStream()
}

/** Синхронизация с опросом /api/status: расшифровку могли запустить или
 *  завершить мимо этой страницы (авто-расшифровка после записи, CLI, IPC,
 *  сон телефона). 2 секунды после done игнорируем — флаг ядра снимается
 *  чуть позже самого события, иначе опрос «оживил» бы завершённую задачу. */
export const syncEngineFromStatus = (
  running: boolean,
  file: string,
  summary?: { active: boolean; file?: string; phase?: string; pct?: number },
): void => {
  if (summary?.active) {
    if (!engine.summaryRunning) {
      engine.summaryPct = 0
      engine.summaryMsg = 'Собираю итоги…'
    }
    engine.summaryRunning = true
    if (summary.file) engine.summaryFile = summary.file
    if (summary.pct !== undefined) {
      engine.summaryPct = Math.max(engine.summaryPct, Math.min(100, summary.pct))
    }
    if (summary.phase) {
      engine.summaryMsg = summaryStageText(summary.phase)
    }
    ensureStream()
  } else if (summary && !summary.active && engine.summaryRunning) {
    // Задача могла завершиться, пока SSE был потерян. Итоговый экран
    // обновится при следующем GET /api/summary.
    engine.summaryRunning = false
    engine.summaryPct = 0
    engine.summaryMsg = ''
    engine.summaryFile = ''
    closeStreamIfIdle()
  }
  if (running && !engine.running) {
    if (Date.now() - finishedAt < 2000) return
    engine.running = true
    engine.file = file
    engine.msg = 'Обработка…'
    if (engine.pct >= 100 || engine.pct === 0) engine.pct = 5
    ensureStream()
  } else if (!running && engine.running) {
    finishEngine(engine.file)
  } else if (running && file && !engine.file) {
    engine.file = file
  }
}

type DownloadingCoreState = { active: boolean; queued: string[] }

export const downloadingCore: DownloadingCoreState = $state({
  active: false,
  queued: [],
})

export const downloadModel = async (m: ModelInfo): Promise<boolean> => {
  delete downloadOutcome[m.id]
  engine.downloading[m.id] = 0
  ensureStream()
  try {
    const r = await fetch('/api/models/update', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model_id: m.id }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) throw new Error(d.error || `HTTP ${r.status}`)
  } catch (e) {
    delete engine.downloading[m.id]
    toast(`Не удалось запустить скачивание: ${errText(e)}`, 'error')
    return false
  }

  return new Promise<boolean>((resolve) => {
    let stall = 0
    const lastPct = () => engine.downloading[m.id] ?? -1
    let prevPct = lastPct()
    const timer = setInterval(async () => {
      if (lastPct() === -1) {
        clearInterval(timer)
        const ok = downloadOutcome[m.id] === true
        delete downloadOutcome[m.id]
        resolve(ok)
        return
      }
      if (lastPct() !== prevPct) {
        prevPct = lastPct()
        stall = 0
      } else if (++stall > 30) {
        clearInterval(timer)
        delete engine.downloading[m.id]
        delete downloadOutcome[m.id]
        emit('models-changed')
        resolve(false)
        return
      }
      try {
        const d = await readJsonSafe<ModelsResponse>(await fetch('/api/models'))
        const cur = (d.models || []).find((x) => x.id === m.id)
        if (cur?.is_installed && !cur.has_update) {
          clearInterval(timer)
          delete engine.downloading[m.id]
          delete downloadOutcome[m.id]
          emit('models-changed')
          resolve(true)
        }
      } catch {}
    }, 1500)
  })
}

export const downloadAllCoreModels = async (): Promise<void> => {
  const core = settings.models.filter((m) => isCoreModel(m) && !m.is_installed)
  if (core.length === 0) return
  downloadingCore.active = true
  downloadingCore.queued = core.map((m) => m.id)
  try {
    for (const m of core) {
      const fresh = settings.models.find((x) => x.id === m.id)
      if (fresh?.is_installed) {
        downloadingCore.queued = downloadingCore.queued.filter((id) => id !== m.id)
        continue
      }
      await downloadModel(m)
      downloadingCore.queued = downloadingCore.queued.filter((id) => id !== m.id)
    }
  } finally {
    downloadingCore.active = false
    downloadingCore.queued = []
  }
}
