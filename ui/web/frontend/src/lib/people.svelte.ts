import type { OkResponse, Person, VoiceDetail, Voicepack, VoiceprintSummary } from './api'
import { readJsonSafe } from './api'
import { errText } from './guards.ts'
import { toast } from './toast.svelte.ts'

type PeopleResponse = {
  people?: Person[]
  voiceprints?: unknown[]
  packs?: Voicepack[]
}
type VoiceprintsResponse = { voiceprints?: VoiceprintSummary[] }
type VoiceDetailResponse = VoiceDetail & OkResponse

type PeopleState = {
  list: Person[]
  voiceprints: string[]
  voiceprintCounts: Record<string, number>
  packs: Voicepack[]
  selectedPackFilter: string
  query: string
  voiceDetail: VoiceDetail | null
  voiceDetailName: string
}

export const peopleState: PeopleState = $state({
  list: [],
  voiceprints: [],
  voiceprintCounts: {},
  packs: [],
  selectedPackFilter: 'all',
  query: '',
  voiceDetail: null,
  voiceDetailName: '',
})

export const loadPeople = async (): Promise<void> => {
  try {
    const r = await fetch('/api/people')
    const d = await readJsonSafe<PeopleResponse>(r)
    peopleState.list = d.people || []
    peopleState.voiceprints = (d.voiceprints || []).map((v) => String(v).toLowerCase().trim())
    if (Array.isArray(d.packs)) {
      peopleState.packs = d.packs
    }
  } catch {}
  // Сколько эмбеддингов у каждого: тот же /api/voiceprints, без нового контракта.
  try {
    const r = await fetch('/api/voiceprints')
    const d = await readJsonSafe<VoiceprintsResponse>(r)
    const counts: Record<string, number> = {}
    for (const v of d.voiceprints || []) {
      const key = String(v.name || '')
        .toLowerCase()
        .trim()
      if (key) counts[key] = Number(v.samples_count || 1)
    }
    peopleState.voiceprintCounts = counts
  } catch {}
}

export const hasVoiceprint = (name: string): boolean => {
  return peopleState.voiceprints.includes(
    String(name || '')
      .toLowerCase()
      .trim(),
  )
}

// Зачем: один взгляд «у кого сколько голосов», без сырого voiceprints.json.
export const voiceprintCount = (name: string): number | undefined => {
  const n =
    peopleState.voiceprintCounts[
      String(name || '')
        .toLowerCase()
        .trim()
    ]
  return typeof n === 'number' ? n : undefined
}

export const loadVoiceDetail = async (name: string): Promise<boolean> => {
  peopleState.voiceDetail = null
  peopleState.voiceDetailName = name
  try {
    const d = await readJsonSafe<VoiceDetailResponse>(
      await fetch(`/api/voiceprints/detail?name=${encodeURIComponent(name)}`),
    )
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
    toast(`Не удалось загрузить голоса: ${errText(e)}`, 'error')
    return false
  }
}

export const snippetUrl = (packId: string, protoId: string): string => {
  return `/api/voicepacks/snippet?pack_id=${encodeURIComponent(packId || '')}&file=${encodeURIComponent(protoId)}.wav`
}

export const deleteVoice = async (name: string): Promise<boolean> => {
  const clean = name.trim()
  if (!clean) return false
  try {
    const r = await fetch('/api/voiceprints/delete', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: clean }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось удалить голос', 'error')
      return false
    }
    toast(`Голос «${clean}» удалён из базы`)
    await loadPeople()
    await loadVoiceDetail(clean)
    return true
  } catch (e) {
    toast(`Не удалось удалить голос: ${errText(e)}`, 'error')
    return false
  }
}

export const deleteVoiceSample = async (
  name: string,
  prototypeId: string,
  packId?: string,
): Promise<boolean> => {
  try {
    const d = await readJsonSafe<OkResponse>(
      await fetch('/api/voiceprints/delete_sample', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, prototype_id: prototypeId, pack_id: packId || undefined }),
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
    toast(`Не удалось удалить образец: ${errText(e)}`, 'error')
    return false
  }
}

export const addPerson = async (name: string, role: string, packId?: string): Promise<boolean> => {
  const cleanName = name.trim()
  if (!cleanName) return false
  const cleanRole = role.trim() || 'Участник'
  try {
    const r = await fetch('/api/people/add', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: cleanName, role: cleanRole, pack_id: packId || 'default' }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось добавить участника', 'error')
      return false
    }
    toast(`Участник «${cleanName}» добавлен`)
    await loadPeople()
    return true
  } catch (e) {
    toast(`Не удалось добавить участника: ${errText(e)}`, 'error')
    return false
  }
}

export const movePerson = async (name: string, toPackId: string): Promise<boolean> => {
  const cleanName = name.trim()
  if (!cleanName || !toPackId) return false
  try {
    const r = await fetch('/api/people/move', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: cleanName, to_pack_id: toPackId }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось переместить участника', 'error')
      return false
    }
    toast(`Участник «${cleanName}» перемещён`)
    await loadPeople()
    return true
  } catch (e) {
    toast(`Не удалось переместить участника: ${errText(e)}`, 'error')
    return false
  }
}
