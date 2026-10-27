// Расшифровка: загрузка транскрипта, запуск движка (SSE), состояние прогресса.
import { baseName, encPath, readJsonSafe } from './api'
import type { ModelInfo, SpeakerInfo, TranscriptData, Utterance } from './api'
import { toast } from './toast.svelte.ts'
import { emit, ui } from './ui.svelte.ts'
import { settings } from './settings.svelte.ts'
import { isVideoPath } from './api'

// Контент транскрипта, открытого в модалке.
export const transcript = $state({
  speakers: [] as SpeakerInfo[],
  utterances: [] as Utterance[],
  markdown: '',
  hasText: false,
  loadError: '',
  extractVisible: false,
})

// Единственное состояние движка для всего UI (дека, модалка, настройки):
// ядро выполняет одну расшифровку одновременно — UI обязан показывать её
// одинаково везде и блокировать остальные запуски.
export const engine = $state({
  running: false,
  file: '',
  msg: 'Обработка…',
  pct: 0,
  downloading: {} as Record<string, number>,
})

export async function openTranscriptFile(rawPath: string): Promise<void> {
  transcript.speakers = []
  transcript.utterances = []
  transcript.markdown = ''
  transcript.hasText = false
  transcript.loadError = ''
  transcript.extractVisible = isVideoPath(rawPath)

  let d: TranscriptData
  try {
    const r = await fetch(`/api/transcript?file=${encPath(rawPath)}`)
    d = (await readJsonSafe(r)) as TranscriptData
  } catch (e) {
    transcript.hasText = false
    transcript.loadError = 'Не удалось загрузить расшифровку: ' + (e as Error).message
    return
  }

  transcript.utterances = d.utterances || []
  transcript.speakers = d.speakers_info || []
  transcript.markdown = d.markdown || ''
  transcript.hasText = Boolean((d.markdown && d.markdown.trim()) || transcript.utterances.length > 0)
}

// Подсказка пустого состояния (модалка решает, показывать ли CTA).
export function emptyHint(): string {
  if (ui.currentTranscriptIsVideo) {
    return 'Расшифровка ещё не выполнялась. Звук из видео извлекается автоматически. Сначала скачайте Silero VAD и GigaAM: «Настройки» → «Модели».'
  }
  return 'Расшифровка ещё не выполнялась. Сначала скачайте Silero VAD и GigaAM: «Настройки» → «Модели».'
}

// ---------- Общий SSE-стрим движка ----------

let evtSource: EventSource | null = null
let finishedAt = 0

function closeStreamIfIdle(): void {
  if (!engine.running && Object.keys(engine.downloading).length === 0) {
    evtSource?.close()
    evtSource = null
  }
}

/** Один EventSource на приложение; повторное подключение браузер выполняет
 *  сам с Last-Event-ID, так что обрыв связи не теряет и не дублирует события. */
export function ensureStream(): void {
  if (evtSource) return
  if (!engine.running && Object.keys(engine.downloading).length === 0) return
  evtSource = new EventSource('/api/transcribe_stream')
  evtSource.onmessage = (e: MessageEvent) => handleEngineEvent(e)
  evtSource.onerror = () => {
    // Если движок не работает и нет активных скачиваний — закрываем стрим,
    // чтобы браузер не крутил бесконечный реконнект каждые 3 секунды.
    if (!engine.running && Object.keys(engine.downloading).length === 0) {
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

function handleEngineEvent(e: MessageEvent): void {
  let evt: any
  try {
    evt = JSON.parse(e.data)
  } catch {
    return
  }
  if (evt.type === 'stage' || evt.type === 'progress') {
    if (evt.stage === 'model-download') {
      if (evt.model_id && evt.pct !== undefined) {
        engine.downloading[evt.model_id] = Math.round(evt.pct)
      }
      return
    }
    engine.msg = evt.message || 'Обработка…'
    if (evt.pct !== undefined) engine.pct = Math.max(engine.pct, Math.max(5, Math.min(100, evt.pct)))
  } else if (evt.type === 'segment') {
    engine.msg = `Реплик распознано: ${evt.done || 0} из ${evt.total || 0}`
    if (evt.pct !== undefined) engine.pct = evt.pct
  } else if (evt.type === 'done') {
    finishEngine(String(evt.file || engine.file))
  } else if (evt.type === 'model-done') {
    delete engine.downloading[evt.model_id]
    emit('models-changed')
    const title = settings.models.find((m) => m.id === evt.model_id)?.title || evt.model_id
    if (evt.ok) toast(`Модель «${title}» готова`)
    else toast('Ошибка модели: ' + (evt.error || ''), 'error')
    closeStreamIfIdle()
  } else if (evt.type === 'stats') {
    if (evt.should_prompt_donate) emit('support-changed')
  } else if (evt.type === 'error') {
    engine.running = false
    toast('Расшифровка прервана: ' + (evt.message || 'неизвестная ошибка'), 'error')
    closeStreamIfIdle()
  }
}

function finishEngine(donePath: string): void {
  if (!engine.running) return
  engine.running = false
  engine.file = ''
  engine.pct = 100
  engine.msg = 'Расшифровка завершена'
  finishedAt = Date.now()
  closeStreamIfIdle()
  emit('recordings-changed')
  emit('support-changed')
  if (!ui.transcriptOpen) toast('Расшифровка записи завершена')
  if (ui.transcriptOpen && donePath && donePath === ui.currentTranscriptPath) {
    void openTranscriptFile(donePath)
  }
}

export async function startTranscription(rawPath: string): Promise<void> {
  if (engine.running) {
    toast(
      engine.file
        ? `Уже расшифровывается «${baseName(engine.file)}». Дождитесь окончания.`
        : 'Расшифровка уже выполняется. Дождитесь окончания.',
    )
    return
  }
  try {
    const r = await fetch('/api/transcribe/start', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file: rawPath, engine: 'auto' }),
    })
    const d = await readJsonSafe(r)
    if (!d.ok) throw new Error(d.error || 'движок отклонил запуск')
  } catch (e) {
    toast('Не удалось запустить расшифровку: ' + (e as Error).message, 'error')
    return
  }
  engine.running = true
  engine.file = rawPath
  engine.msg = 'Подготовка к расшифровке…'
  engine.pct = 5
  ensureStream()
}

/** Синхронизация с опросом /api/status: расшифровку могли запустить или
 *  завершить мимо этой страницы (авто-расшифровка после записи, CLI, IPC,
 *  сон телефона). 2 секунды после done игнорируем — флаг ядра снимается
 *  чуть позже самого события, иначе опрос «оживил» бы завершённую задачу. */
export function syncEngineFromStatus(running: boolean, file: string): void {
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

export const downloadingCore = $state({
  active: false,
  queued: [] as string[],
})

export async function downloadModel(m: ModelInfo): Promise<boolean> {
  engine.downloading[m.id] = 0
  ensureStream()
  try {
    const r = await fetch('/api/models/update', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ model_id: m.id }),
    })
    const d = await readJsonSafe(r)
    if (!d.ok) throw new Error(d.error || `HTTP ${r.status}`)
  } catch (e) {
    delete engine.downloading[m.id]
    toast('Не удалось запустить скачивание: ' + (e as Error).message, 'error')
    return false
  }

  return new Promise<boolean>((resolve) => {
    let stall = 0
    const lastPct = () => engine.downloading[m.id] ?? -1
    let prevPct = lastPct()
    const timer = setInterval(async () => {
      if (lastPct() === -1) {
        clearInterval(timer)
        resolve(true)
        return
      }
      if (lastPct() !== prevPct) {
        prevPct = lastPct()
        stall = 0
      } else if (++stall > 30) {
        clearInterval(timer)
        delete engine.downloading[m.id]
        emit('models-changed')
        resolve(false)
        return
      }
      try {
        const d = await readJsonSafe(await fetch('/api/models'))
        const cur = ((d.models || []) as ModelInfo[]).find((x) => x.id === m.id)
        if (cur && cur.is_installed && !cur.has_update) {
          clearInterval(timer)
          delete engine.downloading[m.id]
          emit('models-changed')
          resolve(true)
        }
      } catch {}
    }, 1500)
  })
}

export async function downloadAllCoreModels(): Promise<void> {
  const core = settings.models.filter(
    (m) => !m.id.toLowerCase().includes('whisper') && !m.is_installed
  )
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
