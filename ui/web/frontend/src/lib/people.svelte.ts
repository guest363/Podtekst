import { readJsonSafe } from './api'
import type { Person, VoiceDetail, VoiceprintSummary } from './api'
import { toast } from './toast.svelte.ts'

export const peopleState = $state({
  list: [] as Person[],
  voiceprints: [] as string[],
  voiceprintCounts: {} as Record<string, number>,
  query: '',
  voiceDetail: null as VoiceDetail | null,
  voiceDetailName: '',
})

export async function loadPeople(): Promise<void> {
  try {
    const r = await fetch('/api/people')
    const d = await readJsonSafe(r)
    peopleState.list = (d.people || []) as Person[]
    peopleState.voiceprints = ((d.voiceprints || []) as string[]).map((v) =>
      String(v).toLowerCase().trim(),
    )
  } catch {
  }
  // Сколько эмбеддингов у каждого: тот же /api/voiceprints, без нового контракта.
  try {
    const r = await fetch('/api/voiceprints')
    const d = await readJsonSafe(r)
    const counts: Record<string, number> = {}
    for (const v of (d.voiceprints || []) as VoiceprintSummary[]) {
      const key = String(v.name || '').toLowerCase().trim()
      if (key) counts[key] = Number(v.samples_count || 1)
    }
    peopleState.voiceprintCounts = counts
  } catch {
  }
}

export function hasVoiceprint(name: string): boolean {
  return peopleState.voiceprints.includes(String(name || '').toLowerCase().trim())
}

// Зачем: один взгляд «у кого сколько голосов», без сырого voiceprints.json.
export function voiceprintCount(name: string): number | undefined {
  const n = peopleState.voiceprintCounts[String(name || '').toLowerCase().trim()]
  return typeof n === 'number' ? n : undefined
}

export async function loadVoiceDetail(name: string): Promise<boolean> {
  peopleState.voiceDetail = null
  peopleState.voiceDetailName = name
  try {
    const d = await readJsonSafe(await fetch(`/api/voiceprints/detail?name=${encodeURIComponent(name)}`))
    if (!d.ok) {
      toast(d.error || 'Не удалось загрузить голоса', 'error')
      return false
    }
    peopleState.voiceDetail = {
      name: d.name,
      pack_id: d.pack_id || '',
      samples_count: d.samples_count,
      prototypes: Array.isArray(d.prototypes) ? d.prototypes : [],
    }
    return true
  } catch (e) {
    toast('Не удалось загрузить голоса: ' + (e as Error).message, 'error')
    return false
  }
}

export function snippetUrl(packId: string, protoId: string): string {
  return `/api/voicepacks/snippet?pack_id=${encodeURIComponent(packId || '')}&file=${encodeURIComponent(protoId)}.wav`
}

export async function deleteVoiceSample(name: string, prototypeId: string): Promise<boolean> {
  try {
    const d = await readJsonSafe(
      await fetch('/api/voiceprints/delete_sample', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, prototype_id: prototypeId }),
      }),
    )
    if (!d.ok) {
      toast(d.error || 'Не удалось удалить образец', 'error')
      return false
    }
    toast('Образец удалён')
    await loadPeople()
    await loadVoiceDetail(name)
    return true
  } catch (e) {
    toast('Не удалось удалить образец: ' + (e as Error).message, 'error')
    return false
  }
}
