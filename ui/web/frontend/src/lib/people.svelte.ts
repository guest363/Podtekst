import { readJsonSafe } from './api'
import type { Person, VoiceprintSummary } from './api'

export const peopleState = $state({
  list: [] as Person[],
  voiceprints: [] as string[],
  voiceprintCounts: {} as Record<string, number>,
  query: '',
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
