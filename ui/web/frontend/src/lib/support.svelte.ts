import type { OkResponse } from './api'
import { readJsonSafe } from './api'
import { copyText, toast } from './toast.svelte.ts'

type SupportInfoResponse = OkResponse & {
  donation?: { methods?: DonateMethod[] }
  stats?: { transcribed_count?: number; total_saved_minutes?: number }
}

export type DonateMethod = {
  title?: string
  desc?: string
  url?: string
  address?: string
}

type SupportState = {
  methods: DonateMethod[]
  transcribedCount: number
  savedMinutes: number
}

export const support: SupportState = $state({
  methods: [],
  transcribedCount: 0,
  savedMinutes: 0,
})

export async function loadSupportInfo(): Promise<void> {
  try {
    const r = await fetch('/api/support/info')
    const d = await readJsonSafe<SupportInfoResponse>(r)
    if (!d.ok) return
    if (d.donation?.methods) {
      support.methods = d.donation.methods
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
