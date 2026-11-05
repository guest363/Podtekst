import { toast } from './toast.svelte.ts'
import { readJsonSafe } from './api'

export interface QueueItem {
  position: number
  file: string
  name: string
  engine: string
}

export interface QueueState {
  running: boolean
  currentFile: string | null
  items: QueueItem[]
  total: number
}

export const queueState = $state<QueueState>({
  running: false,
  currentFile: null,
  items: [],
  total: 0,
})

export async function loadQueue(): Promise<void> {
  try {
    const res = await fetch('/api/transcribe/queue')
    const data = await readJsonSafe(res)
    if (data) {
      queueState.running = Boolean(data.running)
      queueState.currentFile = data.current_file || null
      queueState.items = Array.isArray(data.queue) ? data.queue : []
      queueState.total =
        typeof data.total === 'number'
          ? data.total
          : queueState.running
            ? queueState.items.length + 1
            : queueState.items.length
    }
  } catch {}
}

export async function removeFromQueue(file: string): Promise<boolean> {
  try {
    const res = await fetch('/api/transcribe/queue/remove', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ file }),
    })
    const data = await readJsonSafe(res)
    if (data?.ok) {
      toast('Файл снят с очереди')
      await loadQueue()
      return true
    }
  } catch (e) {
    toast('Не удалось снять файл: ' + (e as Error).message, 'error')
  }
  return false
}

export async function clearQueue(): Promise<boolean> {
  try {
    const res = await fetch('/api/transcribe/queue/clear', {
      method: 'POST',
    })
    const data = await readJsonSafe(res)
    if (data?.ok) {
      toast(`Очередь очищена (снято: ${data.cleared || 0})`)
      await loadQueue()
      return true
    }
  } catch (e) {
    toast('Не удалось очистить очередь: ' + (e as Error).message, 'error')
  }
  return false
}

export function isQueued(filePath: string): boolean {
  return queueState.items.some((i) => i.file === filePath)
}

export function isActive(filePath: string): boolean {
  return queueState.running && queueState.currentFile === filePath
}

export function getPosition(filePath: string): number | null {
  if (isActive(filePath)) return 1
  return queueState.items.find((i) => i.file === filePath)?.position ?? null
}
