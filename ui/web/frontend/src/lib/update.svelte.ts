// Проверка обновлений приложения: GitHub Releases + установка по кнопке.
import { readJsonSafe } from './api'
import { toast } from './toast.svelte.ts'

export const update = $state({
  checking: false,
  current: '',
  latest: '',
  isNewer: false,
  selfUpdate: false,
  url: '',
  dismissed: false,
  applying: false,
  pct: 0,
  error: '',
})

export async function checkUpdate(force = false, notify = false): Promise<void> {
  if (update.checking) return
  update.checking = true
  try {
    const r = await fetch(`/api/update/check${force ? '?force=1' : ''}`)
    const d = await readJsonSafe(r)
    if (d.current) update.current = String(d.current)
    if (!d.ok) {
      if (notify) toast(d.error || 'Не удалось проверить обновления', 'error')
      return
    }
    update.latest = String(d.latest || '')
    update.isNewer = !!d.is_newer
    update.selfUpdate = !!d.self_update
    update.url = String(d.url || '')
    if (update.isNewer) {
      update.dismissed = false
      if (notify) toast(`Доступна версия ${update.latest} — предложение на главном экране`)
    } else if (notify) {
      toast(`У вас последняя версия ${update.current}`)
    }
  } catch (e) {
    if (notify) toast('Не удалось проверить обновления: ' + (e as Error).message, 'error')
  } finally {
    update.checking = false
  }
}

/** Скачать и установить обновление. При успехе приложение перезапустится само. */
export async function applyUpdate(): Promise<void> {
  if (update.applying) return
  update.applying = true
  update.pct = 0
  update.error = ''
  try {
    const d = await readJsonSafe(
      await fetch('/api/update/apply', { method: 'POST' })
    )
    if (!d.ok) throw new Error(d.error || 'Не удалось начать обновление')
  } catch (e) {
    update.applying = false
    update.error = (e as Error).message
    toast('Обновление не запустилось: ' + update.error, 'error')
    return
  }
  const timer = setInterval(async () => {
    try {
      const p = await readJsonSafe(await fetch('/api/update/progress'))
      if (typeof p.pct === 'number') update.pct = p.pct
      if (p.error) {
        clearInterval(timer)
        update.applying = false
        update.error = String(p.error)
        toast('Обновление не установилось: ' + update.error, 'error')
      } else if (!p.running) {
        // Ядро перезапускается: дальше сервер просто перестанет отвечать.
        clearInterval(timer)
      }
    } catch {
      clearInterval(timer)
    }
  }, 1000)
}

export function dismissUpdate(): void {
  update.dismissed = true
}
