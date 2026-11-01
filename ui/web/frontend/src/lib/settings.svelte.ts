import { isMobile, readJsonSafe } from './api'
import type { ModelInfo, Voicepack } from './api'
import { toast } from './toast.svelte.ts'
import { loadPeople } from './people.svelte.ts'

export const settings = $state({
  packs: [] as Voicepack[],
  models: [] as ModelInfo[],
  newPackName: '',
  recordMode: 'split',
  platform: '',
  systemAudio: false,
  autoTranscribe: true,
})

export async function loadAutoTranscribe(): Promise<void> {
  try {
    const d = await readJsonSafe(await fetch('/api/transcribe/auto'))
    settings.autoTranscribe = d.enabled !== false
  } catch {
  }
}

export async function setAutoTranscribe(enabled: boolean): Promise<void> {
  settings.autoTranscribe = enabled
  try {
    const d = await readJsonSafe(
      await fetch('/api/transcribe/auto', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled }),
      }),
    )
    if (!d.ok) toast(d.error || 'Не удалось сохранить авто-расшифровку', 'error')
  } catch {
    toast('Не удалось сохранить авто-расшифровку', 'error')
  }
}

export async function loadRecordMode(): Promise<void> {
  if (isMobile) {
    settings.platform = 'android'
    settings.recordMode = 'mix'
    return
  }
  try {
    const st = await readJsonSafe(await fetch('/api/status'))
    if (typeof st.platform === 'string') settings.platform = st.platform
    settings.systemAudio = st.device?.has_system_audio === true
  } catch {
    /* платформа неизвестна — покажем режим как есть */
  }
  try {
    const d = await readJsonSafe(await fetch('/api/record/mode'))
    if (d.mode === 'mix' || d.mode === 'split') settings.recordMode = d.mode
  } catch {
  }
  // Без второго канала split недоступен: бэкенд пишет моно.
  if (!settings.systemAudio) settings.recordMode = 'mix'
}

export function hasSystemAudio(): boolean {
  return settings.systemAudio && !isMobile
}

export async function setRecordMode(mode: 'split' | 'mix'): Promise<void> {
  settings.recordMode = mode
  try {
    await fetch('/api/record/mode', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode }),
    })
  } catch {
    toast('Не удалось сохранить режим записи', 'error')
  }
}

export function statusBadge(kind: string, text: string): string {
  const esc = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  return `<span class="status-badge ${kind}">${esc}</span>`
}

export const ffmpeg = $state({
  known: false,
  available: false,
  downloading: false,
  pct: 0,
  error: null as string | null,
  manualUrl: 'https://www.gyan.dev/ffmpeg/builds/',
})

function applyFfmpegStatus(f: Record<string, unknown> | undefined): void {
  if (!f || typeof f !== 'object') return
  ffmpeg.known = true
  ffmpeg.available = f.available === true
  ffmpeg.downloading = f.downloading === true
  ffmpeg.pct = Number(f.pct) || 0
  ffmpeg.error = typeof f.error === 'string' ? f.error : null
  if (typeof f.manual_url === 'string') ffmpeg.manualUrl = f.manual_url
}

export async function loadFfmpeg(): Promise<void> {
  try {
    const st = await readJsonSafe(await fetch('/api/status'))
    applyFfmpegStatus(st.ffmpeg)
  } catch {
    /* статус недоступен — карточка просто не покажется */
  }
}

let ffmpegPoll: ReturnType<typeof setInterval> | undefined

export async function downloadFfmpeg(): Promise<void> {
  try {
    const r = await fetch('/api/tools/ffmpeg/download', { method: 'POST' })
    const d = await readJsonSafe(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось запустить скачивание', 'error')
      return
    }
  } catch (e) {
    toast('Ошибка: ' + (e as Error).message, 'error')
    return
  }
  ffmpeg.downloading = true
  ffmpeg.error = null
  if (ffmpegPoll) return
  ffmpegPoll = setInterval(() => {
    void (async () => {
      await loadFfmpeg()
      if (!ffmpeg.downloading && ffmpegPoll) {
        clearInterval(ffmpegPoll)
        ffmpegPoll = undefined
        if (ffmpeg.available && !ffmpeg.error) toast('ffmpeg установлен')
      }
    })()
  }, 1000)
}

export async function loadVoicePacks(): Promise<void> {
  try {
    const r = await fetch('/api/voicepacks')
    const d = await readJsonSafe(r)
    settings.packs = (d.ok && d.packs ? d.packs : []) as Voicepack[]
  } catch {
    settings.packs = []
  }
}

export async function loadModels(): Promise<void> {
  try {
    const r = await fetch('/api/models')
    const d = await readJsonSafe(r)
    settings.models = (d.ok && d.models ? d.models : []) as ModelInfo[]
  } catch {
    settings.models = []
  }
}

export async function togglePack(p: Voicepack): Promise<void> {
  await fetch('/api/voicepacks/toggle', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pack_id: p.id, enabled: !p.enabled }),
  })
  await loadVoicePacks()
  await loadPeople()
}

export async function exportPack(p: Voicepack): Promise<void> {
  try {
    const rr = await fetch('/api/voicepacks/export', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pack_id: p.id }),
    })
    const dd = await readJsonSafe(rr)
    toast(dd.ok ? `Пакет экспортирован: ${dd.path}` : dd.error || 'Ошибка экспорта', dd.ok ? '' : 'error')
  } catch (e) {
    toast('Ошибка экспорта: ' + (e as Error).message, 'error')
  }
}

export async function createPack(): Promise<void> {
  const name = settings.newPackName.trim()
  if (!name) {
    toast('Введите название нового пакета')
    return
  }
  try {
    const r = await fetch('/api/voicepacks/create', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    const d = await readJsonSafe(r)
    if (d.ok) {
      settings.newPackName = ''
      toast(`Пакет «${name}» создан`)
      await loadVoicePacks()
    } else {
      toast(d.error || 'Не удалось создать пакет', 'error')
    }
  } catch (e) {
    toast('Ошибка: ' + (e as Error).message, 'error')
  }
}
