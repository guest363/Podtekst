import { readJsonSafe } from './api'
import type { Person } from './api'

export const peopleState = $state({
  list: [] as Person[],
  voiceprints: [] as string[],
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
}

export function hasVoiceprint(name: string): boolean {
  return peopleState.voiceprints.includes(String(name || '').toLowerCase().trim())
}
