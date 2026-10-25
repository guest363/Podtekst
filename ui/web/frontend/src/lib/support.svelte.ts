// Поддержка проекта: напоминание и способы доната.
import { readJsonSafe } from './api'
import { copyText, toast } from './toast.svelte.ts'

export interface DonateMethod {
  title?: string
  desc?: string
  url?: string
  address?: string
}

export const support = $state({
  visible: false,
  statsText: '',
  methods: [] as DonateMethod[],
})

export async function loadSupportInfo(): Promise<void> {
  try {
    const r = await fetch('/api/support/info')
    const d = await readJsonSafe(r)
    if (!d.ok) return
    const stats = d.stats || {}
    if (stats.transcribed_count >= 10 && !stats.donate_prompt_dismissed) {
      support.visible = true
      const hours = Math.round(stats.total_saved_minutes || 0)
      support.statsText = `Вы сэкономили около ${hours} мин: расшифровано ${stats.transcribed_count} встреч. Проект бесплатный и работает без облаков.`
    }
    if (d.donation && d.donation.methods) {
      support.methods = d.donation.methods as DonateMethod[]
    }
  } catch {
    /* молча */
  }
}

export async function dismissSupport(): Promise<void> {
  try {
    await fetch('/api/support/dismiss', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
  } catch {
    /* молча */
  }
  support.visible = false
}

export async function copyMethod(m: DonateMethod): Promise<void> {
  const ok = await copyText(m.url || m.address || '')
  toast(ok ? 'Адрес скопирован' : 'Не удалось скопировать', ok ? '' : 'error')
}
