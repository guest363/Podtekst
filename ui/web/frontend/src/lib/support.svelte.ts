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
})

export async function loadSupportInfo(): Promise<void> {
  try {
    const r = await fetch('/api/support/info')
    const d = await readJsonSafe(r)
    if (!d.ok) return
    if (d.donation && d.donation.methods) {
      support.methods = d.donation.methods as DonateMethod[]
    }
  } catch {
  }
}

export async function copyMethod(m: DonateMethod): Promise<void> {
  const ok = await copyText(m.url || m.address || '')
  toast(ok ? 'Адрес скопирован' : 'Не удалось скопировать', ok ? '' : 'error')
}
