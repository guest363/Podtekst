// Записи: список, загрузка файлов, извлечение MP3.
import { encPath, isMobile, readJsonSafe } from './api'
import type { RecordingItem } from './api'
import { toast } from './toast.svelte.ts'
import { emit, openTranscript } from './ui.svelte.ts'
import { isVideoPath } from './api'

export const recordings = $state({
  items: [] as RecordingItem[],
})

export async function loadRecordings(): Promise<void> {
  try {
    const r = await fetch('/api/recordings')
    const d = await readJsonSafe(r)
    recordings.items = (d.recordings || []) as RecordingItem[]
  } catch {
    recordings.items = []
  }
}

export async function handleFileUpload(file: File, btn: HTMLButtonElement): Promise<void> {
  if (isMobile && isVideoPath(file.name)) {
    toast('На телефоне видео не поддерживается. Выберите аудиофайл: MP3, WAV, M4A, OGG или FLAC.', 'error')
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
    const data = await readJsonSafe(res)
    if (!res.ok || !data.ok) throw new Error(data.error || `HTTP ${res.status}`)
    toast(`Файл «${file.name}» загружен`)
    await loadRecordings()
    openTranscript(data.path as string, isVideoPath(data.path as string))
  } catch (e) {
    toast('Не удалось загрузить файл: ' + (e as Error).message, 'error')
  } finally {
    btn.textContent = oldText
    btn.disabled = false
  }
}

export async function deleteRecording(rec: RecordingItem): Promise<boolean> {
  try {
    const r = await fetch('/api/recordings/delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: rec.path }),
    })
    const d = await readJsonSafe(r)
    if (d.ok) {
      toast(
        d.kept
          ? 'Убрано из приложения — файл остался на диске'
          : 'Запись удалена',
      )
      emit('recordings-changed')
      return true
    }
    toast(d.error || 'Не удалось удалить запись', 'error')
    return false
  } catch (e) {
    toast('Не удалось удалить запись: ' + (e as Error).message, 'error')
    return false
  }
}

export async function extractMp3(rawPath: string): Promise<void> {
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
    const d = await readJsonSafe(r)
    if (d.ok) {
      toast('Звук сохранён в MP3 рядом с видео')
      emit('recordings-changed')
    } else {
      toast('Не удалось извлечь звук: ' + (d.error || 'неизвестная ошибка'), 'error')
    }
  } catch (e) {
    toast('Не удалось извлечь звук: ' + (e as Error).message, 'error')
  }
}
