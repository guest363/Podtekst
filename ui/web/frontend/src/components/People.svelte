<script lang="ts">
  import { readJsonSafe } from '../lib/api'
  import {
    deleteVoiceSample,
    hasVoiceprint,
    loadPeople,
    loadVoiceDetail,
    peopleState,
    snippetUrl,
    voiceprintCount,
  } from '../lib/people.svelte.ts'
  import { toast, toastConfirm } from '../lib/toast.svelte.ts'
  import type { Person } from '../lib/api'

  let filtered = $derived(
    peopleState.list.filter((p) => {
      const q = peopleState.query.trim().toLowerCase()
      return !q || String(p.name || '').toLowerCase().includes(q)
    }),
  )

  let formOpen = $state(false)
  let editingPerson = $state<Person | null>(null)
  let name = $state('')
  let role = $state('')
  let saving = $state(false)

  function startAdd(): void {
    editingPerson = null
    name = ''
    role = ''
    formOpen = true
    peopleState.voiceDetail = null
    peopleState.voiceDetailName = ''
  }

  function startEdit(p: Person): void {
    editingPerson = p
    name = p.name || ''
    role = p.role || ''
    formOpen = true
    peopleState.voiceDetail = null
    peopleState.voiceDetailName = ''
    if (p.name && hasVoiceprint(p.name)) void loadVoiceDetail(p.name)
  }

  function closeForm(): void {
    formOpen = false
    editingPerson = null
    name = ''
    role = ''
    stopVoice()
    peopleState.voiceDetail = null
    peopleState.voiceDetailName = ''
  }

  async function savePerson(): Promise<void> {
    const cleanName = name.trim()
    if (!cleanName) return
    const cleanRole = role.trim() || 'Гость'
    saving = true
    try {
      const r = await fetch(editingPerson ? '/api/people/update' : '/api/people/add_external', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(
          editingPerson
            ? { old_name: editingPerson.name, name: cleanName, role: cleanRole }
            : { name: cleanName, role: cleanRole },
        ),
      })
      const d = await readJsonSafe(r)
      if (!d.ok) {
        toast(d.error || 'Не удалось сохранить участника', 'error')
        return
      }
      toast(editingPerson ? `Участник «${cleanName}» обновлён` : `Гость «${cleanName}» добавлен`)
      closeForm()
      await loadPeople()
    } catch (e) {
      toast('Не удалось сохранить участника: ' + (e as Error).message, 'error')
    } finally {
      saving = false
    }
  }

  let voiceAudio: HTMLAudioElement | null = $state(null)
  let playingProto = $state('')

  function stopVoice(): void {
    voiceAudio?.pause()
    voiceAudio = null
    playingProto = ''
  }

  function playVoice(packId: string, protoId: string): void {
    if (playingProto === protoId) {
      stopVoice()
      return
    }
    stopVoice()
    const audio = new Audio(snippetUrl(packId, protoId))
    voiceAudio = audio
    playingProto = protoId
    audio.onended = () => {
      if (playingProto === protoId) stopVoice()
    }
    audio.onerror = () => {
      if (playingProto === protoId) stopVoice()
      toast('Не удалось воспроизвести образец', 'error')
    }
    void audio.play().catch(() => {
      stopVoice()
      toast('Не удалось воспроизвести образец', 'error')
    })
  }

  function fmtVoiceDate(iso: unknown): string {
    if (typeof iso !== 'string' || !iso) return ''
    const t = Date.parse(iso)
    if (Number.isNaN(t)) return ''
    try {
      return new Date(t).toLocaleDateString('ru-RU', { day: 'numeric', month: 'short' })
    } catch {
      return ''
    }
  }

  async function removePerson(name: string): Promise<void> {    try {
      const r = await fetch('/api/people/remove', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name }),
      })
      const d = await readJsonSafe(r)
      if (!d.ok) {
        toast(d.error || 'Не удалось убрать участника', 'error')
        return
      }
      toast(`«${name}» убран из участников`)
      await loadPeople()
    } catch (e) {
      toast('Не удалось убрать участника: ' + (e as Error).message, 'error')
    }
  }
</script>

<div class="people-pane" aria-label="Участники встречи">
  <div class="row-between">
    <div class="card-title">
      Участники
      {#if peopleState.list.length > 0}
        <span class="count-badge">{peopleState.list.length}</span>
      {/if}
    </div>
    {#if !formOpen}
      <button class="btn-secondary" onclick={startAdd}>
        + Добавить гостя
      </button>
    {/if}
  </div>

  {#if formOpen}
    <div class="inline-person-form">
      <div class="form-top">
        <span class="form-title">{editingPerson ? 'Изменить участника' : 'Новый гость'}</span>
        <button class="icon-btn btn-sm" aria-label="Закрыть форму" onclick={closeForm}>✕</button>
      </div>
      <div class="form-inputs">
        <div class="input-row">
          <label for="personName">Имя</label>
          <input
            id="personName"
            type="text"
            class="input-field"
            placeholder="Например, Алексей Смирнов"
            autocomplete="off"
            bind:value={name}
            onkeydown={(e) => {
              if (e.key === 'Enter') void savePerson()
              if (e.key === 'Escape') closeForm()
            }}
          />
        </div>
        <div class="input-row">
          <label for="personRole">Организация, роль</label>
          <input
            id="personRole"
            type="text"
            class="input-field"
            placeholder="Например, клиент"
            autocomplete="off"
            bind:value={role}
            onkeydown={(e) => {
              if (e.key === 'Enter') void savePerson()
              if (e.key === 'Escape') closeForm()
            }}
          />
        </div>
      </div>
      {#if editingPerson && editingPerson.name && peopleState.voiceDetailName === editingPerson.name}
        <div class="voice-list">
          <div class="voice-list-title">
            Голоса в базе: {peopleState.voiceDetail?.prototypes?.length || 0}
          </div>
          {#if !peopleState.voiceDetail}
            <div class="empty-note">Загружаю образцы…</div>
          {:else if !(peopleState.voiceDetail.prototypes?.length || 0)}
            <div class="empty-note">Образцов нет — голос запомнится из расшифровки кнопкой «Запомнить».</div>
          {:else}
            {#each peopleState.voiceDetail.prototypes || [] as proto, i (proto.id || i)}
              {@const pid = String(proto.id || '')}
              <div class="voice-row">
                <span class="voice-name">
                  Голос {i + 1}{fmtVoiceDate(proto.created_at) ? ` · ${fmtVoiceDate(proto.created_at)}` : ''}{typeof proto.duration_sec === 'number' ? ` · ${proto.duration_sec.toFixed(1)} c` : ''}
                </span>
                <span class="voice-actions">
                  {#if proto.has_snippet && pid}
                    <button
                      type="button"
                      class="btn-secondary btn-sm"
                      title="Прослушать образец"
                      aria-label={`Прослушать голос ${i + 1}`}
                      onclick={() => playVoice(String(peopleState.voiceDetail?.pack_id || ''), pid)}
                    >
                      {playingProto === pid ? '■' : '▶'}
                    </button>
                  {/if}
                  <button
                    type="button"
                    class="btn-secondary btn-sm"
                    title="Удалить этот образец"
                    aria-label={`Удалить голос ${i + 1}`}
                    onclick={() =>
                      toastConfirm(
                        `Удалить образец «Голос ${i + 1}» у «${editingPerson?.name}»? Остальные голоса сохранятся.`,
                        'Удалить',
                        () => void deleteVoiceSample(String(editingPerson?.name || ''), pid),
                      )}
                  >
                    ✕
                  </button>
                </span>
              </div>
            {/each}
          {/if}
        </div>
      {/if}
      <div class="row-end form-actions">
        <button class="btn-secondary btn-sm" onclick={closeForm}>Отмена</button>
        <button
          class="btn-primary btn-sm"
          disabled={saving || !name.trim()}
          onclick={() => void savePerson()}
        >
          {editingPerson ? 'Сохранить' : 'Добавить'}
        </button>
      </div>
    </div>
  {/if}

  <div class="help-text">
    Каталог людей: имена и роли. Голоса хранятся отдельно, во вкладке «Голоса»,
    и привязываются к участникам при расшифровке кнопкой «Запомнить».
    Нажмите карандаш — там видно все образцы голоса человека.
  </div>
  <div class="row-start">
    <input
      type="search"
      class="input-field search-field"
      placeholder="Найти человека…"
      autocomplete="off"
      aria-label="Найти человека"
      bind:value={peopleState.query}
    />
  </div>
  <div class="people-chips">
    {#if filtered.length === 0}
      <div class="empty-note">
        {peopleState.list.length ? 'Никого не найдено' : 'Пока никого нет. Добавьте первого участника кнопкой выше.'}
      </div>
    {:else}
      {#each filtered as p (p.name)}
        <div
          class="chip {p.is_external ? 'external' : ''} {hasVoiceprint(p.name) ? 'has-voiceprint' : ''}"
          title={p.role ? `${p.name} – ${p.role}` : p.name}
        >
          {#if hasVoiceprint(p.name)}
            <span class="voice-dot" title="Голос сохранён в базе"></span>
          {/if}
          <span class="chip-main">
            <span class="chip-name">{p.name}{p.is_external ? ' · гость' : ''}{voiceprintCount(p.name) ? ` · голосов: ${voiceprintCount(p.name)}` : ''}</span>
            {#if p.role}<span class="chip-role">{p.role}</span>{/if}
          </span>
          <button
            class="chip-edit"
            title="Изменить имя и роль"
            aria-label={`Изменить участника ${p.name}`}
            onclick={() => startEdit(p)}
          >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
              <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4z" />
            </svg>
          </button>
          <button
            class="chip-del"
            title="Убрать из участников"
            aria-label={`Убрать участника ${p.name}`}
            onclick={() =>
              toastConfirm(`Убрать «${p.name}» из участников?`, 'Убрать', () => void removePerson(p.name))}
          >
            ×
          </button>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .people-pane {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .inline-person-form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    animation: fadeIn 180ms var(--ease-spring);
  }
  .form-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .form-title {
    font-size: 13.5px;
    font-weight: 800;
    color: var(--text-primary);
  }
  .form-inputs {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .form-actions {
    gap: 8px;
    margin-top: 4px;
  }
  .voice-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
  }
  .voice-list-title {
    font-size: 12.5px;
    font-weight: 800;
    color: var(--text-primary);
  }
  .voice-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .voice-name {
    font-size: 12.5px;
    color: var(--text-secondary);
  }
  .voice-actions {
    display: flex;
    gap: 6px;
  }
</style>
