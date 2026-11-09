import { readJsonSafe } from './api'
import { copyText, toast } from './toast.svelte.ts'

export interface DonateMethod {
  title?: string
  desc?: string
  url?: string
  address?: string
}

export const support = $state({
  methods: [] as DonateMethod[],
  transcribedCount: 0,
  savedMinutes: 0,
})

export async function loadSupportInfo(): Promise<void> {
  try {
    const r = await fetch('/api/support/info')
    const d = await readJsonSafe(r)
    if (!d.ok) return
    if (d.donation && d.donation.methods) {
      support.methods = d.donation.methods as DonateMethod[]
    }
    const stats = d.stats || {}
    support.transcribedCount = Number(stats.transcribed_count) || 0
    support.savedMinutes = Number(stats.total_saved_minutes) || 0
  } catch {}
}

export async function copyMethod(m: DonateMethod): Promise<void> {
  const ok = await copyText(m.url || m.address || '')
  toast(ok ? 'Адрес скопирован' : 'Не удалось скопировать', ok ? '' : 'error')
}
