<script lang="ts">
import type { OkResponse, Person } from '../lib/api'
import { readJsonSafe } from '../lib/api'
import { errText } from '../lib/guards.ts'
import {
  deleteVoice,
  deleteVoiceSample,
  hasVoiceprint,
  loadPeople,
  loadVoiceDetail,
  peopleState,
  snippetUrl,
  voiceprintCount,
} from '../lib/people.svelte.ts'
import { toast, toastConfirm } from '../lib/toast.svelte.ts'

let filtered = $derived(
  peopleState.list.filter((p) => {
    const q = peopleState.query.trim().toLowerCase()
    const matchQ =
      !q ||
      String(p.name || '')
        .toLowerCase()
        .includes(q)
    const matchPack =
      peopleState.selectedPackFilter === 'all' ||
      (p.pack_id || 'default') === peopleState.selectedPackFilter
    return matchQ && matchPack
  }),
)

let formOpen = $state(false)
let editingPerson = $state<Person | null>(null)
let name = $state('')
let role = $state('')
let targetPackId = $state('default')
let saving = $state(false)
let packDropdownOpen = $state(false)
let selectedPackName = $derived(
  peopleState.packs.find((p) => p.id === targetPackId)?.name || 'Основной профиль',
)

function togglePackDropdown(): void {
  packDropdownOpen = !packDropdownOpen
}

function selectPack(id: string): void {
  targetPackId = id
  packDropdownOpen = false
}

$effect(() => {
  if (!packDropdownOpen) return
  const onScroll = (e: Event) => {
    const target = e.target instanceof HTMLElement ? e.target : null
    if (target?.closest?.('.pack-dropdown-popover')) return
    packDropdownOpen = false
  }
  window.addEventListener('scroll', onScroll, { capture: true, passive: true })
  return () => {
    window.removeEventListener('scroll', onScroll, { capture: true })
  }
})

function startAdd(): void {
  editingPerson = null
  name = ''
  role = ''
  targetPackId =
    peopleState.selectedPackFilter !== 'all'
      ? peopleState.selectedPackFilter
      : peopleState.packs[0]?.id || 'default'
  packDropdownOpen = false
  formOpen = true
  peopleState.voiceDetail = null
  peopleState.voiceDetailName = ''
}

function startEdit(p: Person): void {
  editingPerson = p
  name = p.name || ''
  role = p.role || ''
  targetPackId = p.pack_id || 'default'
  packDropdownOpen = false
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
  targetPackId = 'default'
  packDropdownOpen = false
  stopVoice()
  peopleState.voiceDetail = null
  peopleState.voiceDetailName = ''
}

async function savePerson(): Promise<void> {
  const cleanName = name.trim()
  if (!cleanName) return
  const cleanRole = role.trim() || 'Участник'
  saving = true
  try {
    const oldPack = editingPerson?.pack_id || 'default'
    const r = await fetch(editingPerson ? '/api/people/update' : '/api/people/add', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(
        editingPerson
          ? {
              old_name: editingPerson.name,
              name: cleanName,
              role: cleanRole,
              from_pack_id: oldPack,
              pack_id: targetPackId,
              target_pack_id: targetPackId,
            }
          : { name: cleanName, role: cleanRole, pack_id: targetPackId },
      ),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось сохранить участника', 'error')
      return
    }
    toast(editingPerson ? `Участник «${cleanName}» сохранён` : `Участник «${cleanName}» добавлен`)
    closeForm()
    await loadPeople()
  } catch (e) {
    toast(`Не удалось сохранить участника: ${errText(e)}`, 'error')
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

async function removePerson(name: string): Promise<void> {
  try {
    const r = await fetch('/api/people/remove', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name }),
    })
    const d = await readJsonSafe<OkResponse>(r)
    if (!d.ok) {
      toast(d.error || 'Не удалось убрать участника', 'error')
      return
    }
    toast(`«${name}» убран из участников`)
    await loadPeople()
  } catch (e) {
    toast(`Не удалось убрать участника: ${errText(e)}`, 'error')
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
        + Добавить участника
      </button>
    {/if}
  </div>

  {#if formOpen}
    <div class="inline-person-form">
      <div class="form-top">
        <span class="form-title">{editingPerson ? 'Изменить участника' : 'Новый участник'}</span>
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
        {#if peopleState.packs.length > 1}
          <div class="input-row">
            <span class="input-label" id="packLabel">Голосовой пакет</span>
            <div class="custom-select-wrap">
              <button
                type="button"
                class="input-field custom-select-trigger"
                aria-haspopup="listbox"
                aria-expanded={packDropdownOpen}
                aria-labelledby="packLabel"
                onclick={togglePackDropdown}
                onkeydown={(e) => {
                  if (e.key === 'Escape' && packDropdownOpen) {
                    e.stopPropagation()
                    packDropdownOpen = false
                  }
                }}
              >
                <span class="custom-select-val">{selectedPackName}</span>
                <span class="custom-select-chevron {packDropdownOpen ? 'open' : ''}" aria-hidden="true">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9"></polyline>
                  </svg>
                </span>
              </button>

              {#if packDropdownOpen}
                <button
                  type="button"
                  class="speaker-popover-backdrop"
                  style="z-index: 65;"
                  aria-label="Закрыть список"
                  onclick={() => (packDropdownOpen = false)}
                  onwheel={(e) => e.preventDefault()}
                ></button>
                <div
                  class="speaker-select-popover pack-dropdown-popover"
                  role="listbox"
                  aria-label="Выбор голосового пакета"
                >
                  {#each peopleState.packs as pk (pk.id)}
                    <button
                      type="button"
                      class="speaker-select-item"
                      style={targetPackId === pk.id ? 'background: #f1f5f9; font-weight: 600;' : ''}
                      onclick={() => selectPack(pk.id)}
                    >
                      <span class="speaker-select-item-left">
                        <span
                          class="speaker-select-item-dot"
                          style={`background: ${targetPackId === pk.id ? 'var(--accent-blue)' : '#94a3b8'}`}
                        ></span>
                        <span class="speaker-select-item-name">{pk.name}</span>
                      </span>
                      {#if typeof pk.people_count === 'number'}
                        <span class="speaker-select-item-sub">{pk.people_count} уч.{pk.id === 'default' ? ' · по умолчанию' : ''}</span>
                      {:else if pk.id === 'default'}
                        <span class="speaker-select-item-sub">по умолчанию</span>
                      {/if}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>
      {#if editingPerson && editingPerson.name && peopleState.voiceDetailName === editingPerson.name}
        <div class="voice-list">
          <div class="voice-list-header">
            <span class="voice-list-title">
              Голоса в базе: {peopleState.voiceDetail?.prototypes?.length || 0}
            </span>
            {#if (peopleState.voiceDetail?.prototypes?.length || 0) > 0}
              <button
                type="button"
                class="link-danger"
                onclick={() =>
                  toastConfirm(
                    `Удалить все образцы голоса у «${editingPerson?.name}»? Участник останется в списке.`,
                    'Удалить',
                    () => void deleteVoice(String(editingPerson?.name || '')),
                  )}
              >
                Удалить голос
              </button>
            {/if}
          </div>
          {#if !peopleState.voiceDetail}
            <div class="empty-note">Загружаю образцы…</div>
          {:else if !(peopleState.voiceDetail.prototypes?.length || 0)}
            <div class="empty-note">Образцов нет - голос запоминается из расшифровки кнопкой «Запомнить».</div>
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
                      class="voice-btn play {playingProto === pid ? 'playing' : ''}"
                      title={playingProto === pid ? 'Остановить образец' : 'Прослушать образец'}
                      aria-label={`Прослушать голос ${i + 1}`}
                      onclick={() => playVoice(String(peopleState.voiceDetail?.pack_id || ''), pid)}
                    >
                      {#if playingProto === pid}
                        <svg width="8" height="8" viewBox="0 0 12 12" aria-hidden="true">
                          <rect x="2" y="2" width="8" height="8" rx="1" fill="currentColor" />
                        </svg>
                      {:else}
                        <svg width="9" height="9" viewBox="0 0 12 12" aria-hidden="true">
                          <path d="M3.6 2.1v7.8L10.2 6z" fill="currentColor" />
                        </svg>
                      {/if}
                    </button>
                  {/if}
                  <button
                    type="button"
                    class="voice-btn del"
                    title="Удалить этот образец"
                    aria-label={`Удалить голос ${i + 1}`}
                    onclick={() =>
                      toastConfirm(
                        `Удалить образец «Голос ${i + 1}» у «${editingPerson?.name}»? Остальные голоса сохранятся.`,
                        'Удалить',
                        () => void deleteVoiceSample(String(editingPerson?.name || ''), pid, peopleState.voiceDetail?.pack_id),
                      )}
                  >
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                      <line x1="18" y1="6" x2="6" y2="18"></line>
                      <line x1="6" y1="6" x2="18" y2="18"></line>
                    </svg>
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
    Каталог участников: имена и роли. Образцы голоса хранятся во вкладке «Голоса».
    Они привязываются к людям кнопкой «Запомнить» на странице расшифровки.
    Чтобы посмотреть или удалить образцы голоса, нажмите на значок карандаша.
  </div>

  {#if peopleState.packs.length > 1}
    <div class="pack-filter-bar" role="tablist" aria-label="Фильтр по голосовым пакетам">
      <button
        class="pack-filter-btn {peopleState.selectedPackFilter === 'all' ? 'active' : ''}"
        onclick={() => (peopleState.selectedPackFilter = 'all')}
      >
        Все ({peopleState.list.length})
      </button>
      {#each peopleState.packs as pk (pk.id)}
        {@const count = peopleState.list.filter((p) => (p.pack_id || 'default') === pk.id).length}
        <button
          class="pack-filter-btn {peopleState.selectedPackFilter === pk.id ? 'active' : ''}"
          onclick={() => (peopleState.selectedPackFilter = pk.id)}
        >
          {pk.name} ({count})
        </button>
      {/each}
    </div>
  {/if}

  <div class="row-start">
    <input
      type="search"
      class="input-field search-field"
      placeholder="Найти участника…"
      autocomplete="off"
      aria-label="Найти участника"
      bind:value={peopleState.query}
    />
  </div>
  <div class="people-chips">
    {#if filtered.length === 0}
      <div class="empty-note">
        {peopleState.list.length ? 'Ничего не найдено' : 'Участников пока нет. Добавьте первого кнопкой выше.'}
      </div>
    {:else}
      {#each filtered as p (p.name)}
        <div
          class="chip {p.is_external ? 'external' : ''} {hasVoiceprint(p.name) ? 'has-voiceprint' : ''}"
          title={p.role ? `${p.name} - ${p.role}` : p.name}
        >
          {#if hasVoiceprint(p.name)}
            <span class="voice-dot" title="Голос сохранён в базе"></span>
          {/if}
          <div class="chip-main">
            <div class="chip-title-row">
              <span class="chip-name">{p.name}</span>
              {#if voiceprintCount(p.name)}
                <span class="chip-voice-count">{voiceprintCount(p.name)} гол.</span>
              {/if}
            </div>
            <span class="chip-role">
              {p.role || (p.is_external ? 'Гость' : 'Участник')}{p.pack_name ? ` · ${p.pack_name}` : ''}
            </span>
          </div>
          <div class="chip-actions">
            <button
              class="chip-edit"
              title="Изменить имя и роль"
              aria-label={`Изменить участника ${p.name}`}
              onclick={() => startEdit(p)}
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
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
    width: 100%;
    min-width: 0;
  }
  :global(.chip) {
    box-sizing: border-box;
    max-width: 100%;
    min-width: 0;
  }
  :global(.chip-title-row) {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  :global(.chip-voice-count) {
    font-size: 10.5px;
    color: var(--accent-ok);
    background: var(--accent-ok-bg);
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    font-family: var(--font-mono);
    flex-shrink: 0;
    font-weight: 600;
  }
  :global(.chip-actions) {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    margin-left: auto;
  }
  @media (max-width: 640px) {
    :global(.people-chips) {
      grid-template-columns: minmax(0, 1fr) !important;
      gap: 6px;
      width: 100%;
      min-width: 0;
    }
    :global(.chip) {
      padding: 8px 10px;
      min-height: 48px;
      gap: 8px;
    }
    :global(.chip-name) {
      font-size: 13px;
      font-weight: 600;
    }
    :global(.chip-actions .chip-edit),
    :global(.chip-actions .chip-del) {
      opacity: 0.85;
      width: 28px;
      height: 28px;
      border-radius: var(--radius-sm);
      background: var(--bg-inset);
    }
    :global(.chip-actions .chip-del) {
      font-size: 16px;
    }
  }
  .pack-filter-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .pack-filter-btn {
    border: 1px solid var(--border-card);
    background: #fff;
    color: var(--text-muted);
    padding: 4px 10px;
    border-radius: var(--radius-pill);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    transition: all 140ms ease;
  }
  .pack-filter-btn:hover {
    color: var(--text-primary);
    background: var(--bg-inset);
  }
  .pack-filter-btn.active {
    background: #16233a;
    color: #fff;
    border-color: #16233a;
  }
  .custom-select-wrap {
    position: relative;
    width: 100%;
  }
  .custom-select-trigger {
    display: flex;
    align-items: center;
    justify-content: space-between;
    text-align: left;
    cursor: pointer;
    box-sizing: border-box;
  }
  .custom-select-val {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .custom-select-chevron {
    display: inline-flex;
    align-items: center;
    color: var(--text-faint);
    transition: transform 160ms var(--ease-spring);
    flex-shrink: 0;
    margin-left: 8px;
  }
  .custom-select-chevron.open {
    transform: rotate(180deg);
  }
  .pack-dropdown-popover {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    width: 100%;
    min-width: 0;
    max-width: 100%;
    max-height: 220px;
    z-index: 70;
    box-sizing: border-box;
  }
  :global(.app-drawer:has(.pack-dropdown-popover) .app-drawer-body) {
    overflow-y: hidden;
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
  .form-top .icon-btn {
    width: 24px;
    height: 24px;
    font-size: 12px;
    border: none;
    background: transparent;
    color: var(--text-faint);
  }
  .form-top .icon-btn:hover {
    background: rgba(15, 30, 51, 0.06);
    color: var(--text-primary);
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
  .voice-list-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
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
    padding: 2px 0;
  }
  .voice-name {
    font-size: 12.5px;
    color: var(--text-secondary);
  }
  .voice-actions {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-shrink: 0;
  }
  .voice-btn {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    flex-shrink: 0;
    border: none;
    background: transparent;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    transition: transform 160ms var(--ease-spring), background 160ms var(--ease-spring), color 160ms var(--ease-spring);
  }
  .voice-btn.play {
    background: #e8effc;
    color: var(--accent-blue);
  }
  .voice-btn.play:hover {
    background: #d8e5fb;
    transform: scale(1.08);
  }
  .voice-btn.play:active {
    transform: scale(0.95);
  }
  .voice-btn.play.playing {
    background: var(--accent-blue);
    color: #fff;
  }
  .voice-btn.del {
    color: var(--text-faint);
  }
  .voice-btn.del:hover {
    background: #fee2e2;
    color: var(--accent-rec);
    transform: scale(1.08);
  }
  .voice-btn.del:active {
    transform: scale(0.95);
  }
</style>
