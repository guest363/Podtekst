<script lang="ts">
  import SlimPlayer from './SlimPlayer.svelte'
  import { deleteRecording, handleFileUpload, loadRecordings, recordings } from '../lib/recordings.svelte.ts'
  import { displayRecTitle, fmtClock, formatRecDate, isMobile, pluralSpeakers, readJsonSafe } from '../lib/api'
  import type { RecordingItem } from '../lib/api'
  import { on, openTranscript } from '../lib/ui.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'
  import { onMount } from 'svelte'

  let uploadInput: HTMLInputElement | null = $state(null)
  let uploadBtn: HTMLButtonElement | null = $state(null)
  let dropCard: HTMLElement | null = $state(null)
  let drawerOpen = $state(false)
  let search = $state('')
  let searchInput: HTMLInputElement | null = $state(null)
  let renaming = $state('')
  let renameValue = $state('')
  let savingName = $state(false)
  let deleteTarget = $state<RecordingItem | null>(null)
  let deleting = $state(false)

  // Активная запись для воспроизведения в общем нижнем плеере
  let activeRec = $state<RecordingItem | null>(null)
  let isPlaying = $state(false)
  let playerRef = $state<{ togglePlay: () => void } | null>(null)

  function toggleRecPlay(rec: RecordingItem): void {
    if (activeRec?.path === rec.path) {
      playerRef?.togglePlay()
    } else {
      activeRec = rec
      isPlaying = true
    }
  }
  // На телефоне видео не импортируется (нет ffmpeg) и не показывается.
  const VISIBLE = 5
  const items = $derived(isMobile ? recordings.items.filter((r) => !r.is_video) : recordings.items)
  const filtered = $derived(
    search.trim() === ''
      ? items
      : items.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase())),
  )
  const visible = $derived(
    search.trim() === ''
      ? items.slice(0, VISIBLE)
      : filtered.slice(0, VISIBLE),
  )
  const hiddenCount = $derived(
    search.trim() === ''
      ? Math.max(0, items.length - VISIBLE)
      : Math.max(0, filtered.length - VISIBLE),
  )

  // Фокус в поиск при открытии панели (вместо autofocus-атрибута).
  $effect(() => {
    if (drawerOpen) searchInput?.focus()
  })

  onMount(() => {
    on('recordings-changed', () => void loadRecordings())
    const onFocus = () => void loadRecordings()
    window.addEventListener('focus', onFocus)
    void loadRecordings()
    return () => {
      window.removeEventListener('focus', onFocus)
    }
  })

  function closeDrawer(): void {
    drawerOpen = false
    search = ''
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (e.key !== 'Escape') return
    if (deleteTarget) {
      deleteTarget = null
      return
    }
    if (drawerOpen) closeDrawer()
  }

  async function confirmDelete(): Promise<void> {
    if (!deleteTarget || deleting) return
    deleting = true
    if (activeRec?.path === deleteTarget.path) {
      activeRec = null
      isPlaying = false
    }
    const ok = await deleteRecording(deleteTarget)
    deleting = false
    if (ok) deleteTarget = null
  }

  function onDrop(e: DragEvent): void {
    e.preventDefault()
    dropCard?.classList.remove('dragover')
    const f = e.dataTransfer?.files?.[0]
    if (f && uploadBtn) void handleFileUpload(f, uploadBtn)
  }

  function startRename(rec: RecordingItem): void {
    renaming = rec.path
    renameValue = rec.name.replace(/\.[^.]+$/, '')
  }

  function cancelRename(): void {
    renaming = ''
    renameValue = ''
  }

  async function saveRename(rec: RecordingItem): Promise<void> {
    const name = renameValue.trim()
    if (!name) {
      toast('Имя не может быть пустым', 'error')
      return
    }
    savingName = true
    try {
      const r = await fetch('/api/recordings/rename', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: rec.path, name }),
      })
      const d = await readJsonSafe(r)
      if (d.ok) {
        toast('Запись переименована')
        cancelRename()
        await loadRecordings()
      } else {
        toast(d.error || 'Не удалось переименовать запись', 'error')
      }
    } catch (e) {
      toast('Не удалось переименовать запись: ' + (e as Error).message, 'error')
    } finally {
      savingName = false
    }
  }

</script>

{#snippet recRow(rec: RecordingItem)}
  {@const dt = formatRecDate(rec.date, rec.name)}
  <div class="rec-item rec-card">
    <div class="rec-date-col">
      <span class="rec-date-day">{dt.day}</span>
      {#if dt.time}
        <span class="rec-date-time">{dt.time}</span>
      {/if}
    </div>

    <div class="rec-info">
      {#if renaming === rec.path}
        <div class="rename-row">
          <input
            class="input-field rename-input"
            aria-label="Новое имя записи"
            autocomplete="off"
            bind:value={renameValue}
            onkeydown={(e) => {
              if (e.key === 'Enter') void saveRename(rec)
              if (e.key === 'Escape') cancelRename()
            }}
          />
          <button class="btn-primary btn-sm" disabled={savingName} onclick={() => void saveRename(rec)}>
            Сохранить
          </button>
          <button class="btn-secondary btn-sm" onclick={cancelRename}>Отмена</button>
        </div>
      {:else}
        <span class="rec-name-row">
          <span class="rec-name" title={rec.name}>{displayRecTitle(rec.name)}</span>
          {#if rec.is_video}
            <span class="rec-tag-video">видео</span>
          {/if}
          <button
            class="rec-rename"
            title="Переименовать запись"
            aria-label={`Переименовать запись ${rec.name}`}
            onclick={() => startRename(rec)}
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
              <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4z" />
            </svg>
          </button>
          <button
            class="rec-delete-icon"
            title={rec.owned ? 'Удалить запись' : 'Убрать из приложения'}
            aria-label={rec.owned ? `Удалить запись ${rec.name}` : `Убрать запись ${rec.name} из приложения`}
            onclick={() => (deleteTarget = rec)}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v6M14 11v6" />
            </svg>
          </button>
        </span>
      {/if}
      <span class="rec-meta">
        {#if rec.duration_sec}{fmtClock(rec.duration_sec)} • {/if}
        {#if rec.has_transcript}
          <span class="rec-state-ok">
            {#if rec.speakers_count}{rec.speakers_count} {pluralSpeakers(rec.speakers_count)}{:else}Текст готов{/if}
          </span>
        {:else}
          {rec.is_video ? 'Видео' : 'Аудио'}
        {/if}
        • {rec.size_mb} МБ
      </span>
    </div>

    <div class="rec-actions">
      <button
        class="slim-play"
        onclick={() => toggleRecPlay(rec)}
        aria-label={activeRec?.path === rec.path && isPlaying ? `Остановить ${rec.name}` : `Слушать ${rec.name}`}
        title={activeRec?.path === rec.path && isPlaying ? 'Пауза' : 'Слушать'}
      >
        {#if activeRec?.path === rec.path && isPlaying}
          <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true">
            <rect x="2.4" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
            <rect x="6.8" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
          </svg>
        {:else}
          <svg class="play-glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
            <path d="M3.6 2.1v7.8L10.2 6z" fill="currentColor" />
          </svg>
        {/if}
      </button>
      <button class="btn-secondary btn-sm" onclick={() => openTranscript(rec.path, rec.is_video)}>
        Текст и спикеры
      </button>
    </div>
  </div>
{/snippet}

<svelte:window onkeydown={onWindowKey} />

<section
  class="card"
  id="recordingsCard"
  aria-label="Записи"
  bind:this={dropCard}
  ondragover={(e) => {
    e.preventDefault()
    dropCard?.classList.add('dragover')
  }}
  ondragenter={(e) => {
    e.preventDefault()
    dropCard?.classList.add('dragover')
  }}
  ondragleave={(e) => {
    e.preventDefault()
    dropCard?.classList.remove('dragover')
  }}
  ondrop={onDrop}
>
  <div class="card-header">
    <span class="card-title">{isMobile ? 'Записи' : 'Записи и видео'} <span class="count-badge">{items.length}</span></span>
    <div class="header-actions">
      {#if items.length > 2}
        <input
          type="search"
          class="input-field card-search"
          placeholder="Поиск записи…"
          autocomplete="off"
          aria-label="Поиск записи"
          bind:value={search}
        />
      {/if}
      <input
        type="file"
        bind:this={uploadInput}
        accept={isMobile
          ? 'audio/*,.mp3,.wav,.m4a,.ogg,.flac,.aac,.opus'
          : 'audio/*,video/*,.mp4,.mkv,.mov,.avi,.webm,.m4v,.mp3,.wav,.m4a,.ogg,.flac'}
        hidden
        onchange={() => {
          const f = uploadInput?.files?.[0]
          if (f && uploadBtn) void handleFileUpload(f, uploadBtn)
        }}
      />
      <button class="btn-secondary" bind:this={uploadBtn} onclick={() => uploadInput?.click()}>
        Выбрать файл
      </button>
    </div>
  </div>
  <div class="recordings-list">
    {#if items.length === 0}
      <div class="empty-note">
        Записей пока нет
        <span class="empty-sub">
          {#if isMobile}
            Поддерживаются аудиофайлы: MP3, WAV, M4A, OGG, FLAC.
          {:else}
            Перетащите файл записи сюда или нажмите «Выбрать файл» (MP3, WAV, MP4, MKV, MOV).
          {/if}
        </span>
      </div>
    {:else}
      {#each visible as rec (rec.path)}
        {@render recRow(rec)}
      {/each}
    {/if}
  </div>
  {#if hiddenCount > 0}
    <div class="row-start">
      <button class="btn-secondary" onclick={() => (drawerOpen = true)}>
        Все записи · ещё {hiddenCount}
      </button>
    </div>
  {/if}
</section>

{#if drawerOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="app-drawer-overlay" role="presentation" onclick={closeDrawer}></div>
  <div class="app-drawer rec-drawer" role="dialog" aria-modal="true" aria-label="Все записи" tabindex="-1">
    <div class="app-drawer-top">
      <span class="card-title">Все записи · {items.length}</span>
      <button class="icon-btn" aria-label="Закрыть список записей" onclick={closeDrawer}>✕</button>
    </div>
    <div class="app-drawer-body">
      <input
        type="search"
        class="input-field"
        placeholder="Поиск по названию…"
        autocomplete="off"
        aria-label="Поиск по названию"
        bind:this={searchInput}
        bind:value={search}
      />
      <div class="recordings-list rec-drawer-list">
        {#if filtered.length === 0}
          <div class="empty-note">Ничего не найдено</div>
        {:else}
          {#each filtered as rec (rec.path)}
            {@render recRow(rec)}
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

{#if deleteTarget}
  <div
    class="modal-overlay open"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label={deleteTarget.owned ? 'Удаление записи' : 'Удаление из приложения'}
    onmousedown={(e) => {
      if (e.target === e.currentTarget && !deleting) deleteTarget = null
    }}
  >
    <div class="modal-box delete-box">
      <div class="modal-top">
        <div class="modal-title">
          {deleteTarget.owned ? 'Удалить запись?' : 'Убрать из приложения?'}
        </div>
        <button
          class="icon-btn"
          aria-label="Закрыть подтверждение"
          disabled={deleting}
          onclick={() => (deleteTarget = null)}
        >
          ✕
        </button>
      </div>
      <div class="help-text">
        {#if deleteTarget.owned}
          «{deleteTarget.name}» и её расшифровка будут удалены с диска. Это действие нельзя отменить.
        {:else}
          «{deleteTarget.name}» исчезнет из приложения. Сам файл и расшифровка останутся на диске —
          приложение удаляет только свою ссылку на него.
        {/if}
      </div>
      <div class="row-end">
        <button class="btn-secondary" disabled={deleting} onclick={() => (deleteTarget = null)}>
          Отмена
        </button>
        <button class="btn-primary" disabled={deleting} onclick={() => void confirmDelete()}>
          {deleting ? 'Удаляю…' : deleteTarget.owned ? 'Удалить' : 'Убрать из приложения'}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if activeRec}
  {@const dt = formatRecDate(activeRec.date, activeRec.name)}
  <div class="bottom-player-bar" role="region" aria-label="Плеер текущей записи">
    <div class="bottom-player-info">
      <span class="bottom-player-title" title={activeRec.name}>{displayRecTitle(activeRec.name)}</span>
      <span class="bottom-player-date">{dt.full}</span>
    </div>
    <SlimPlayer
      bind:this={playerRef}
      bind:playing={isPlaying}
      src={`/audio/${encodeURIComponent(activeRec.name)}`}
      isVideo={activeRec.is_video}
      fileName={activeRec.name}
      durationSec={activeRec.duration_sec || 0}
      autoplay={true}
    />
    <div class="bottom-player-actions">
      <button class="btn-secondary btn-sm" onclick={() => openTranscript(activeRec!.path, activeRec!.is_video)}>
        Текст и спикеры
      </button>
      <button
        class="bottom-player-close"
        title="Закрыть плеер"
        aria-label="Закрыть плеер"
        onclick={() => {
          activeRec = null
          isPlaying = false
        }}
      >
        ✕
      </button>
    </div>
  </div>
{/if}

<style>
  .card-search {
    width: 140px;
    padding: 6px 10px;
    font-size: 12px;
  }
  .rec-drawer-list { overflow-y: auto; flex: 1; min-height: 0; padding-right: 2px; }
  .rec-delete-icon {
    margin-left: auto;
    flex-shrink: 0;
    border: none; background: transparent;
    color: var(--text-faint); cursor: pointer;
    padding: 6px; border-radius: var(--radius-sm);
    min-width: 30px; min-height: 30px;
    display: inline-flex; align-items: center; justify-content: center;
    opacity: 0.55;
    transition: opacity 140ms ease, color 140ms ease, background 140ms ease;
  }
  .rec-delete-icon:hover {
    opacity: 1; color: var(--accent-rec); background: var(--bg-inset);
  }
  .delete-box { max-width: 440px; }
</style>
