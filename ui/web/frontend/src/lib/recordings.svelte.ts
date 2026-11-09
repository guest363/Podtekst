import type { OkResponse, RecordingItem } from './api'
import { encPath, isMobile, isVideoPath, readJsonSafe } from './api'
import { errText } from './guards.ts'
import { toast } from './toast.svelte.ts'
import { emit, openTranscript } from './ui.svelte.ts'

type RecordingsResponse = { recordings?: RecordingItem[] }
type UploadResponse = OkResponse & { path: string }
type DeleteResponse = OkResponse & { kept?: boolean }

type RecordingsState = {
  items: RecordingItem[]
}

export const recordings: RecordingsState = $state({
  items: [],
})

export const loadRecordings = async (): Promise<void> => {
  try {
    const r = await fetch('/api/recordings')
    const d = await readJsonSafe<RecordingsResponse>(r)
    recordings.items = d.recordings || []
  } catch {
    recordings.items = []
  }
}

export const handleFileUpload = async (file: File, btn: HTMLButtonElement): Promise<void> => {
  if (isMobile && isVideoPath(file.name)) {
    toast(
      'На телефоне видео не поддерживается. Выберите аудиофайл: MP3, WAV, M4A, OGG или FLAC.',
      'error',
    )
    return
  }
  const oldText = btn.textContent
  btn.textContent = 'Загрузка…'
  btn.disabled = true
  try {
    const res = await fetch('/api/upload', {
      method: 'POST',
      headers: {
        'X-Filename': encPath(file.name),
        'Content-Type': file.type || 'application/octet-stream',
      },
      body: file,
    })
    const data = await readJsonSafe<UploadResponse>(res)
    if (!res.ok || !data.ok) throw new Error(data.error || `HTTP ${res.status}`)
    toast(`Файл «${file.name}» загружен`)
    await loadRecordings()
    openTranscript(data.path, isVideoPath(data.path))
  } catch (e) {
    toast(`Не удалось загрузить файл: ${errText(e)}`, 'error')
  } finally {
    btn.textContent = oldText
    btn.disabled = false
  }
}

export const deleteRecording = async (rec: RecordingItem): Promise<boolean> => {
  try {
    const r = await fetch('/api/recordings/delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: rec.path }),
    })
    const d = await readJsonSafe<DeleteResponse>(r)
    if (d.ok) {
      toast(d.kept ? 'Убрано из приложения — файл остался на диске' : 'Запись удалена')
      emit('recordings-changed')
      return true
    }
    toast(d.error || 'Не удалось удалить запись', 'error')
    return false
  } catch (e) {
    toast(`Не удалось удалить запись: ${errText(e)}`, 'error')
    return false
  }
}

export const extractMp3 = async (rawPath: string): Promise<void> => {
  if (isMobile) {
    toast('Извлечение MP3 из видео недоступно на телефоне', 'error')
    return
  }
  try {
    const r = await fetch('/api/media/extract_audio', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file: rawPath, codec: 'mp3' }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (d.ok) {
      toast('Звук сохранён в MP3 рядом с видео')
      emit('recordings-changed')
    } else {
      toast(`Не удалось извлечь звук: ${d.error || 'неизвестная ошибка'}`, 'error')
    }
  } catch (e) {
    toast(`Не удалось извлечь звук: ${errText(e)}`, 'error')
  }
}
