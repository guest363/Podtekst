<script lang="ts">
  import SlimPlayer from './SlimPlayer.svelte'
  import { deleteRecording, extractMp3, handleFileUpload, loadRecordings, recordings } from '../lib/recordings.svelte.ts'
  import { fmtClock, isMobile, pluralSpeakers, readJsonSafe } from '../lib/api'
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

  // В карточке показываем последние 5 записей. Остальные скрыты за дровером.
  const VISIBLE = 5
  const filtered = $derived(
    search.trim() === ''
      ? recordings.items
      : recordings.items.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase())),
  )
  const visible = $derived(
    search.trim() === ''
      ? recordings.items.slice(0, VISIBLE)
      : filtered.slice(0, VISIBLE),
  )
  const hiddenCount = $derived(
    search.trim() === ''
      ? Math.max(0, recordings.items.length - VISIBLE)
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
    if (drawerOpen && e.key === 'Escape') closeDrawer()
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
  <div class="rec-item">
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
          <span class="rec-name" title={rec.name}>{rec.name}</span>
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
        </span>
      {/if}
      <span class="rec-meta">
        {#if rec.duration_sec}{fmtClock(rec.duration_sec)} • {/if}{rec.size_mb} МБ • {rec.date || ''} •
        {#if rec.has_transcript}
          <span class="rec-state-ok">
            Текст{#if rec.speakers_count} • {rec.speakers_count} {pluralSpeakers(rec.speakers_count)}{/if}
          </span>
        {:else}
          {rec.is_video ? 'Видео' : 'Аудио'}
        {/if}
      </span>
    </div>
    <SlimPlayer
      src={`/audio/${encodeURIComponent(rec.name)}`}
      isVideo={rec.is_video}
      fileName={rec.name}
      durationSec={rec.duration_sec || 0}
    />
    <div class="rec-actions">
      {#if rec.is_video}
        <button class="btn-secondary btn-sm" title="Извлечь звук из видео в MP3" onclick={() => void extractMp3(rec.path)}>
          MP3
        </button>
      {/if}
      <button class="btn-secondary btn-sm" onclick={() => openTranscript(rec.path, rec.is_video)}>
        Текст и спикеры
      </button>
      <button
        class="btn-secondary btn-sm rec-delete"
        title="Удалить запись и её расшифровку"
        aria-label={`Удалить запись ${rec.name}`}
        onclick={() => void deleteRecording(rec.name, rec.path)}
      >
        Удалить
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
    <span class="card-title">Записи и видео <span class="count-badge">{recordings.items.length}</span></span>
    <div class="header-actions">
      {#if recordings.items.length > 2}
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
        accept="audio/*,video/*,.mp4,.mkv,.mov,.avi,.webm,.m4v,.mp3,.wav,.m4a,.ogg,.flac"
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
  <div class="help-text">
    Поддерживаются аудио и видео: <b>MP3, WAV, MP4, MKV, MOV, WEBM</b>.{#if !isMobile} Файл можно перетащить сюда.{/if}
  </div>
  <div class="recordings-list">
    {#if recordings.items.length === 0}
      <div class="empty-note">Записей пока нет</div>
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
      <span class="card-title">Все записи · {recordings.items.length}</span>
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

<style>
  .card-search {
    width: 140px;
    padding: 6px 10px;
    font-size: 12px;
  }
  .rec-drawer-list { overflow-y: auto; flex: 1; min-height: 0; padding-right: 2px; }
  .rec-drawer-list :global(.rec-item) {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 6px 12px;
  }
  .rec-drawer-list :global(.rec-item > .slim-player) {
    grid-column: 1;
  }
  .rec-drawer-list :global(.rec-actions) {
    grid-column: 2;
    grid-row: 1 / span 2;
    display: flex;
    gap: 6px;
    flex-wrap: nowrap;
    align-items: center;
    justify-content: flex-end;
    width: auto;
  }
  @media (max-width: 480px) {
    .rec-drawer-list :global(.rec-item) {
      grid-template-columns: 1fr;
    }
    .rec-drawer-list :global(.rec-actions) {
      grid-column: 1;
      grid-row: auto;
      width: 100%;
      justify-content: flex-start;
      flex-wrap: wrap;
    }
  }
  /* Кнопка удаления — сдержанный красный, чтобы отличалась от нейтральных действий. */
  .rec-delete { color: var(--accent-rec); }
  .rec-delete:hover { color: var(--accent-rec-hover); background: #fee2e2; }
</style>
