// Проверка обновлений приложения: GitHub Releases + предложение скачать.
import { readJsonSafe } from './api'
import { toast } from './toast.svelte.ts'

export const update = $state({
  checking: false,
  current: '',
  latest: '',
  isNewer: false,
  url: '',
  dismissed: false,
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

export function dismissUpdate(): void {
  update.dismissed = true
}
