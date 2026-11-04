<script lang="ts">
  import SlimPlayer from './SlimPlayer.svelte'
  import DatePickerPopover from './DatePickerPopover.svelte'
  import { deleteRecording, handleFileUpload, loadRecordings, recordings } from '../lib/recordings.svelte.ts'
  import {
    displayRecTitle,
    fmtClock,
    formatRangeLabel,
    formatRecDate,
    isMobile,
    parseRecDate,
    pluralRecords,
    pluralSpeakers,
    readJsonSafe,
  } from '../lib/api'
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

  const now = new Date()
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())

  let mainFilter = $state<'today' | 'week' | 'all' | 'range'>('today')
  let mainRangeStart = $state<Date | null>(today)
  let mainRangeEnd = $state<Date | null>(today)
  let mainRangeLabel = $state('')
  let mainPickerOpen = $state(false)

  let drawerRangeStart = $state<Date | null>(null)
  let drawerRangeEnd = $state<Date | null>(null)
  let drawerPickerOpen = $state(false)

  // На телефоне видео не импортируется (нет ffmpeg) и не показывается.
  const VISIBLE = 5
  const items = $derived(isMobile ? recordings.items.filter((r) => !r.is_video) : recordings.items)

  const countToday = $derived(
    items.filter((r) => {
      const d = parseRecDate(r.date, r.name)
      return d && d.getTime() === today.getTime()
    }).length,
  )
  const countWeek = $derived(
    items.filter((r) => {
      const d = parseRecDate(r.date, r.name)
      if (!d) return false
      const t = d.getTime()
      return t >= today.getTime() - 6 * 86400000 && t <= today.getTime()
    }).length,
  )

  function filterByRange(list: RecordingItem[], start: Date | null, end: Date | null): RecordingItem[] {
    if (!start && !end) return list
    return list.filter((r) => {
      const d = parseRecDate(r.date, r.name)
      if (!d) return false
      const t = d.getTime()
      if (start && t < start.getTime()) return false
      if (end && t > end.getTime()) return false
      return true
    })
  }

  const mainFiltered = $derived.by(() => {
    let list = search.trim() === ''
      ? items
      : items.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()))

    if (mainFilter === 'today') {
      list = filterByRange(list, today, today)
    } else if (mainFilter === 'week') {
      const s = new Date(today.getTime() - 6 * 86400000)
      list = filterByRange(list, s, today)
    } else if (mainFilter === 'range' && mainRangeStart) {
      list = filterByRange(list, mainRangeStart, mainRangeEnd ?? mainRangeStart)
    }
    return list
  })

  const visible = $derived(mainFiltered.slice(0, VISIBLE))
  const hiddenCount = $derived(Math.max(0, items.length - VISIBLE))

  const drawerFiltered = $derived.by(() => {
    let list = search.trim() === ''
      ? items
      : items.filter((r) => r.name.toLowerCase().includes(search.trim().toLowerCase()))

    if (drawerRangeStart) {
      list = filterByRange(list, drawerRangeStart, drawerRangeEnd ?? drawerRangeStart)
    }
    return list
  })

  const isDrawerFilterActive = $derived(drawerRangeStart !== null)
  const drawerDateLabel = $derived(
    drawerRangeStart
      ? formatRangeLabel(drawerRangeStart, drawerRangeEnd ?? drawerRangeStart)
      : 'Все даты',
  )

  const drawerGroups = $derived.by(() => {
    const map = new Map<string, { label: string; isToday: boolean; recs: RecordingItem[] }>()
    const monthsGen = [
      'января', 'февраля', 'марта', 'апреля', 'мая', 'июня',
      'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря',
    ]
    const monthsNom = [
      'Январь', 'Февраль', 'Март', 'Апрель', 'Май', 'Июнь',
      'Июль', 'Август', 'Сентябрь', 'Октябрь', 'Ноябрь', 'Декабрь',
    ]
    for (const rec of drawerFiltered) {
      const d = parseRecDate(rec.date, rec.name)
      let key = 'archive'
      let label = 'Архив'
      let isGroupToday = false

      if (d) {
        const yesterday = new Date(today)
        yesterday.setDate(yesterday.getDate() - 1)
        const y = d.getFullYear()
        const ySuffix = y !== today.getFullYear() ? ` ${y}` : ''

        if (d.getTime() === today.getTime()) {
          key = 'today'
          label = `Сегодня · ${d.getDate()} ${monthsGen[d.getMonth()]}`
          isGroupToday = true
        } else if (d.getTime() === yesterday.getTime()) {
          key = 'yesterday'
          label = `Вчера · ${d.getDate()} ${monthsGen[d.getMonth()]}`
        } else {
          const diffDays = Math.round((today.getTime() - d.getTime()) / 86400000)
          if (diffDays > 0 && diffDays <= 7) {
            key = `day-${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`
            label = `${d.getDate()} ${monthsGen[d.getMonth()]}${ySuffix}`
          } else {
            key = `month-${d.getFullYear()}-${d.getMonth()}`
            label = `${monthsNom[d.getMonth()]}${ySuffix}`
          }
        }
      }

      let entry = map.get(key)
      if (!entry) {
        entry = { label, isToday: isGroupToday, recs: [] }
        map.set(key, entry)
      }
      entry.recs.push(rec)
    }
    return Array.from(map.values())
  })

  function setMainPreset(p: 'today' | 'week' | 'all'): void {
    mainFilter = p
    mainRangeStart = p === 'today' ? today : p === 'week' ? new Date(today.getTime() - 6 * 86400000) : null
    mainRangeEnd = p === 'all' ? null : today
    mainRangeLabel = ''
    mainPickerOpen = false
  }

  function onMainRangeSelect(start: Date, end: Date, label: string): void {
    mainFilter = 'range'
    mainRangeStart = start
    mainRangeEnd = end
    mainRangeLabel = label
    mainPickerOpen = false
  }

  function clearMainRange(): void {
    setMainPreset('today')
  }

  function onDrawerRangeSelect(start: Date, end: Date, label: string): void {
    drawerRangeStart = start
    drawerRangeEnd = end
    drawerPickerOpen = false
  }

  function resetDrawerFilter(): void {
    drawerRangeStart = null
    drawerRangeEnd = null
    drawerPickerOpen = false
  }

  function hasRecsOnDate(d: Date): boolean {
    const t = d.getTime()
    return items.some((r) => {
      const rd = parseRecDate(r.date, r.name)
      return rd && rd.getTime() === t
    })
  }

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
    drawerPickerOpen = false
    resetDrawerFilter()
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
  <div class="card-header rec-card-header">
    <div class="rec-header-left">
      <span class="card-title">{isMobile ? 'Записи' : 'Записи и видео'}</span>
      <div class="rec-segmented" role="tablist" aria-label="Фильтр по дате">
        <button
          type="button"
          class="rec-seg-btn {mainFilter === 'today' ? 'active' : ''}"
          onclick={() => setMainPreset('today')}
          role="tab"
          aria-selected={mainFilter === 'today'}
        >
          <span>Сегодня</span>
          {#if countToday > 0}<span class="rec-seg-count">{countToday}</span>{/if}
        </button>
        <button
          type="button"
          class="rec-seg-btn {mainFilter === 'week' ? 'active' : ''}"
          onclick={() => setMainPreset('week')}
          role="tab"
          aria-selected={mainFilter === 'week'}
        >
          <span>7 дней</span>
          {#if countWeek > 0}<span class="rec-seg-count">{countWeek}</span>{/if}
        </button>
        <button
          type="button"
          class="rec-seg-btn {mainFilter === 'all' ? 'active' : ''}"
          onclick={() => setMainPreset('all')}
          role="tab"
          aria-selected={mainFilter === 'all'}
        >
          <span>Все</span>
          <span class="rec-seg-count">{items.length}</span>
        </button>
        <div class="rec-seg-sep" aria-hidden="true"></div>
        <div class="rec-cal-wrap">
          <button
            type="button"
            class="rec-seg-btn rec-seg-cal {mainFilter === 'range' ? 'active' : ''}"
            onclick={() => (mainPickerOpen = !mainPickerOpen)}
            title="Выбрать период в календаре"
            aria-label="Выбрать период в календаре"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
              <line x1="16" y1="2" x2="16" y2="6" />
              <line x1="8" y1="2" x2="8" y2="6" />
              <line x1="3" y1="10" x2="21" y2="10" />
            </svg>
            {#if mainFilter === 'range' && mainRangeLabel}
              <span class="rec-range-text">{mainRangeLabel}</span>
            {/if}
          </button>
          {#if mainFilter === 'range'}
            <button
              type="button"
              class="rec-seg-clear-btn"
              title="Сбросить диапазон"
              aria-label="Сбросить диапазон"
              onclick={clearMainRange}
            >
              ✕
            </button>
          {/if}
          {#if mainPickerOpen}
            <DatePickerPopover
              start={mainRangeStart}
              end={mainRangeEnd}
              onSelect={onMainRangeSelect}
              onClear={clearMainRange}
              onClose={() => (mainPickerOpen = false)}
              hasRecordings={hasRecsOnDate}
            />
          {/if}
        </div>
      </div>
    </div>
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
    {:else if visible.length === 0}
      <div class="empty-note">
        {#if search.trim() !== ''}
          Ничего не найдено
        {:else if mainFilter === 'today'}
          За сегодня записей нет
          <span class="empty-sub">
            <button type="button" class="btn-link" onclick={() => setMainPreset('all')}>Показать все ({items.length})</button>
          </span>
        {:else if mainFilter === 'week'}
          За последние 7 дней записей нет
          <span class="empty-sub">
            <button type="button" class="btn-link" onclick={() => setMainPreset('all')}>Показать все ({items.length})</button>
          </span>
        {:else}
          За выбранный период записей нет
          <span class="empty-sub">
            <button type="button" class="btn-link" onclick={() => setMainPreset('all')}>Показать все ({items.length})</button>
          </span>
        {/if}
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
      <div class="drawer-toolbar-row">
        <div class="drawer-search-wrap">
          <input
            type="search"
            class="input-field drawer-search-input"
            placeholder="Поиск по названию…"
            autocomplete="off"
            aria-label="Поиск по названию"
            bind:this={searchInput}
            bind:value={search}
          />
        </div>
        <div class="drawer-picker-wrap">
          <button
            type="button"
            class="btn-secondary drawer-picker-btn {isDrawerFilterActive ? 'active' : ''}"
            onclick={() => (drawerPickerOpen = !drawerPickerOpen)}
            title="Период записей"
            aria-label="Фильтр периода записей"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
              <line x1="16" y1="2" x2="16" y2="6" />
              <line x1="8" y1="2" x2="8" y2="6" />
              <line x1="3" y1="10" x2="21" y2="10" />
            </svg>
            <span>{drawerDateLabel}</span>
            <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M6 9l6 6 6-6" />
            </svg>
          </button>
          {#if isDrawerFilterActive}
            <button
              type="button"
              class="drawer-clear-filter-btn"
              title="Сбросить дату"
              aria-label="Сбросить дату"
              onclick={resetDrawerFilter}
            >
              <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          {/if}
          {#if drawerPickerOpen}
            <DatePickerPopover
              start={drawerRangeStart}
              end={drawerRangeEnd}
              onSelect={onDrawerRangeSelect}
              onClear={resetDrawerFilter}
              onClose={() => (drawerPickerOpen = false)}
              hasRecordings={hasRecsOnDate}
            />
          {/if}
        </div>
      </div>
      <div class="recordings-list rec-drawer-list">
        {#if drawerFiltered.length === 0}
          <div class="empty-note">
            Ничего не найдено
            {#if isDrawerFilterActive || search.trim() !== ''}
              <span class="empty-sub">
                <button type="button" class="btn-link" onclick={() => { search = ''; resetDrawerFilter(); }}>
                  Сбросить поиск и фильтр
                </button>
              </span>
            {/if}
          </div>
        {:else}
          {#each drawerGroups as group}
            <div class="rec-group">
              <div class="rec-sticky-header {group.isToday ? 'is-today' : ''}">
                <div class="rec-sticky-left">
                  {#if group.isToday}
                    <span class="rec-sticky-dot" aria-hidden="true"></span>
                  {/if}
                  <span class="rec-sticky-title">{group.label}</span>
                </div>
                <span class="rec-sticky-count">{group.recs.length} {pluralRecords(group.recs.length)}</span>
              </div>
              {#each group.recs as rec (rec.path)}
                {@render recRow(rec)}
              {/each}
            </div>
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
        <button
          class={deleteTarget.owned ? 'btn-primary btn-danger' : 'btn-primary'}
          disabled={deleting}
          onclick={() => void confirmDelete()}
        >
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
  .delete-box { max-width: 440px; }
  /* Деструктивное подтверждение: красный — только здесь, запись его не занимает смысл */
  .btn-danger {
    background: var(--accent-rec);
    border: 1px solid var(--accent-rec-hover);
  }
  .btn-danger:hover { background: var(--accent-rec-hover); }
</style>
