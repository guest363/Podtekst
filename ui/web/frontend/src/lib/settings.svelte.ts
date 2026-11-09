import type { ModelInfo, Voicepack } from './api'
import { isMobile, readJsonSafe } from './api'
import { errText } from './guards.ts'
import { loadPeople } from './people.svelte.ts'
import { toast, toastConfirm } from './toast.svelte.ts'

type StatusResponse = {
  platform?: string
  features?: { summary?: boolean }
  device?: { has_system_audio?: boolean }
  system_audio_permission?: boolean
  ffmpeg?: Record<string, unknown>
}

type SummarySettingsResponse = {
  ok?: boolean
  code?: string
  capabilities?: { summary_supported?: boolean }
}

type TranscribeAutoResponse = { enabled?: boolean }
type RecordModeResponse = { mode?: 'mix' | 'split'; explicit?: boolean }
type VoicepacksResponse = { ok?: boolean; packs?: Voicepack[] }
type ModelsResponse = { ok?: boolean; models?: ModelInfo[] }
type HotwordsResponse = {
  ok?: boolean
  error?: string
  hotwords?: unknown[]
  ctc_enabled?: boolean
}
type ExportPackResponse = { ok?: boolean; path?: string; error?: string }
type ImportPackResponse = { ok?: boolean; pack?: { name?: string }; error?: string }
type OkResponse = { ok?: boolean; error?: string; code?: string }

type SettingsState = {
  packs: Voicepack[]
  models: ModelInfo[]
  newPackName: string
  recordMode: 'split' | 'mix'
  platform: string
  systemAudio: boolean
  systemAudioPermission: boolean
  autoTranscribe: boolean
  hotwords: string[]
  ctcEnabled: boolean
  hotwordsLoaded: boolean
  summaryFeature: boolean | null
}

export const settings: SettingsState = $state({
  packs: [],
  models: [],
  newPackName: '',
  recordMode: 'split',
  platform: '',
  systemAudio: false,
  systemAudioPermission: true,
  autoTranscribe: true,
  hotwords: [],
  ctcEnabled: true,
  hotwordsLoaded: false,
  summaryFeature: null,
})

export async function loadSummaryFeature(): Promise<void> {
  if (settings.summaryFeature !== null) return
  try {
    const st = await readJsonSafe<StatusResponse>(await fetch('/api/status'))
    if (st?.features && typeof st.features.summary === 'boolean' && !st.features.summary) {
      settings.summaryFeature = false
      return
    }
  } catch {
    /* статус недоступен — проверим сам маршрут итогов */
  }
  try {
    const d = await readJsonSafe<SummarySettingsResponse>(await fetch('/api/summary/settings'))
    if (d?.ok === false) {
      settings.summaryFeature = !(d.code === 'summary_disabled' || d.code === 'summary_unsupported')
      return
    }
    settings.summaryFeature = d?.capabilities?.summary_supported !== false
  } catch {
    // Старый/недоступный бэкенд: не прячем итоги из-за сбоя проверки.
    settings.summaryFeature = true
  }
}

export async function loadAutoTranscribe(): Promise<void> {
  try {
    const d = await readJsonSafe<TranscribeAutoResponse>(await fetch('/api/transcribe/auto'))
    settings.autoTranscribe = d.enabled !== false
  } catch {}
}

export async function setAutoTranscribe(enabled: boolean): Promise<void> {
  settings.autoTranscribe = enabled
  try {
    const d = await readJsonSafe<OkResponse>(
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
    const st = await readJsonSafe<StatusResponse>(await fetch('/api/status'))
    if (typeof st.platform === 'string') settings.platform = st.platform
    settings.systemAudio = st.device?.has_system_audio === true
    if (typeof st.system_audio_permission === 'boolean') {
      settings.systemAudioPermission = st.system_audio_permission
    }
  } catch {
    /* платформа неизвестна — покажем режим как есть */
  }
  try {
    const d = await readJsonSafe<RecordModeResponse>(await fetch('/api/record/mode'))
    if (d.mode === 'mix' || d.mode === 'split') settings.recordMode = d.mode
    // Старый конфиг мог сохранить mix, когда системного канала ещё не было.
    // Один раз переключаем на split; ручной выбор пользователя не трогаем.
    if (settings.systemAudio && d.explicit !== true && settings.recordMode === 'mix') {
      await setRecordMode('split')
      toast('Включили 2 канала: теперь слышны собеседники. Вернуть можно в настройках')
    }
  } catch {}
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

type FfmpegState = {
  known: boolean
  available: boolean
  downloading: boolean
  pct: number
  error: string | null
  manualUrl: string
}

export const ffmpeg: FfmpegState = $state({
  known: false,
  available: false,
  downloading: false,
  pct: 0,
  error: null,
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
    const st = await readJsonSafe<StatusResponse>(await fetch('/api/status'))
    applyFfmpegStatus(st.ffmpeg)
  } catch {
    /* статус недоступен — карточка просто не покажется */
  }
}

let ffmpegPoll: ReturnType<typeof setInterval> | undefined

export async function downloadFfmpeg(): Promise<void> {
  try {
    const r = await fetch('/api/tools/ffmpeg/download', { method: 'POST' })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось запустить скачивание', 'error')
      return
    }
  } catch (e) {
    toast(`Ошибка: ${errText(e)}`, 'error')
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
    const d = await readJsonSafe<VoicepacksResponse>(r)
    settings.packs = d.ok && d.packs ? d.packs : []
  } catch {
    settings.packs = []
  }
}

export async function loadModels(): Promise<void> {
  try {
    const r = await fetch('/api/models')
    const d = await readJsonSafe<ModelsResponse>(r)
    settings.models = d.ok && d.models ? d.models : []
  } catch {
    settings.models = []
  }
}

export async function loadHotwords(): Promise<void> {
  try {
    const d = await readJsonSafe<HotwordsResponse>(await fetch('/api/hotwords'))
    if (Array.isArray(d.hotwords))
      settings.hotwords = d.hotwords.filter((x: unknown) => typeof x === 'string')
    if (typeof d.ctc_enabled === 'boolean') settings.ctcEnabled = d.ctc_enabled
    settings.hotwordsLoaded = true
  } catch {}
}

export async function saveHotwords(hotwords: string[], ctcEnabled: boolean): Promise<boolean> {
  try {
    const d = await readJsonSafe<HotwordsResponse>(
      await fetch('/api/hotwords', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ hotwords, ctc_enabled: ctcEnabled }),
      }),
    )
    if (!d.ok) {
      toast(d.error || 'Не удалось сохранить словарь', 'error')
      return false
    }
    settings.hotwords = Array.isArray(d.hotwords)
      ? d.hotwords.filter((x): x is string => typeof x === 'string')
      : []
    settings.ctcEnabled = d.ctc_enabled !== false
    settings.hotwordsLoaded = true
    return true
  } catch (e) {
    toast(`Не удалось сохранить словарь: ${errText(e)}`, 'error')
    return false
  }
}

export async function addHotword(term: string): Promise<boolean> {
  const t = term.trim()
  if (!t) return false
  let cur: string[] = settings.hotwordsLoaded ? [...settings.hotwords] : []
  if (!settings.hotwordsLoaded) {
    try {
      const d = await readJsonSafe<HotwordsResponse>(await fetch('/api/hotwords'))
      if (Array.isArray(d.hotwords))
        cur = d.hotwords.filter((x): x is string => typeof x === 'string')
      if (typeof d.ctc_enabled === 'boolean') settings.ctcEnabled = d.ctc_enabled
      settings.hotwordsLoaded = true
    } catch {}
  }
  if (cur.some((x) => x.toLowerCase() === t.toLowerCase())) {
    toast(`«${t}» уже в словаре`)
    return true
  }
  if (cur.length >= 500) {
    toast('Словарь полон (500). Удалите лишнее в «Настройки» → «Компоненты».', 'error')
    return false
  }
  return saveHotwords([...cur, t], settings.ctcEnabled)
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

export function deletePack(p: Voicepack): void {
  if (p.id === 'default') {
    toast('Базовый пакет удалить нельзя: выключите его или удалите голоса по одному', 'error')
    return
  }
  toastConfirm(
    `Удалить пакет «${p.name}»? Его голоса и люди пропадут из распознавания.`,
    'Удалить',
    () =>
      void (async () => {
        try {
          const d = await readJsonSafe<OkResponse>(
            await fetch('/api/voicepacks/delete', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ pack_id: p.id }),
            }),
          )
          if (!d.ok) {
            toast(d.error || 'Не удалось удалить пакет', 'error')
            return
          }
          toast(`Пакет «${p.name}» удалён`)
          await loadVoicePacks()
          await loadPeople()
        } catch (e) {
          toast(`Не удалось удалить пакет: ${errText(e)}`, 'error')
        }
      })(),
  )
}

export async function exportPack(p: Voicepack): Promise<void> {
  try {
    const rr = await fetch('/api/voicepacks/export', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pack_id: p.id }),
    })
    const dd = await readJsonSafe<ExportPackResponse>(rr)
    toast(
      dd.ok ? `Пакет экспортирован: ${dd.path}` : dd.error || 'Ошибка экспорта',
      dd.ok ? '' : 'error',
    )
  } catch (e) {
    toast(`Ошибка экспорта: ${errText(e)}`, 'error')
  }
}

export async function importPack(file: File): Promise<boolean> {
  const name = file.name || 'pack.voicepack.zip'
  if (!/\.zip$|\.json$/i.test(name)) {
    toast('Нужен .voicepack.zip или .voicepack.json', 'error')
    return false
  }
  try {
    const r = await fetch('/api/voicepacks/import', {
      method: 'POST',
      headers: { 'X-Filename': encodeURIComponent(name) },
      body: file,
    })
    const d = await readJsonSafe<ImportPackResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось импортировать пакет', 'error')
      return false
    }
    toast(`Пакет «${d.pack?.name || name}» импортирован`)
    await loadVoicePacks()
    await loadPeople()
    return true
  } catch (e) {
    toast(`Не удалось импортировать: ${errText(e)}`, 'error')
    return false
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
    const d = await readJsonSafe<OkResponse>(r)
    if (d.ok) {
      settings.newPackName = ''
      toast(`Пакет «${name}» создан`)
      await loadVoicePacks()
    } else {
      toast(d.error || 'Не удалось создать пакет', 'error')
    }
  } catch (e) {
    toast(`Ошибка: ${errText(e)}`, 'error')
  }
}
