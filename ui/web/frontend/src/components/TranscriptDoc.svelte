<script lang="ts">
  import { tick } from 'svelte'
  import { fmtClockShort, speakerColor, speakerTint } from '../lib/api'
  import type { SpeakerInfo, Utterance } from '../lib/api'
  import { editTranscript } from '../lib/transcript.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'

  interface Props {
    utterances: Utterance[]
    markdown: string
    file?: string
    speakers?: SpeakerInfo[]
    currentTime?: number
    meta?: {
      title: string
      source: string
      durStr?: string
      speakers?: string[]
    }
    onseek?: (sec: number) => void
  }
  let { utterances, markdown, file = '', speakers = [], currentTime = 0, meta, onseek }: Props = $props()

  let search = $state('')
  let follow = $state(true)
  let doc: HTMLElement | null = $state(null)
  let editingIndex = $state(-1)
  let draft = $state('')
  let editor: HTMLElement | null = $state(null)
  let saving = $state(false)
  let speakerMenu = $state<{ index: number; x: number; y: number } | null>(null)
  let renameValue = $state('')
  let menuInput: HTMLInputElement | null = $state(null)
  let speakerNames = $derived(
    [...new Set([...speakers.map((s) => s.name), ...utterances.map((u) => u.speaker || 'Спикер')])].filter(Boolean),
  )
  let menuSpeaker = $derived(speakerMenu ? utterances[speakerMenu.index]?.speaker || 'Спикер' : '')
  let activeIndex = $derived.by(() => {
    if (currentTime <= 0 || !utterances.length) return -1
    // При одновременной речи и перекрытиях каналов выбираем реплику,
    // начавшуюся позже всех (наибольший start <= currentTime).
    // Старый findIndex зависал на первой длинной реплике и отставал на 2-5 секунд.
    let bestIdx = -1
    let bestStart = -1
    for (let i = 0; i < utterances.length; i++) {
      const u = utterances[i]
      const end = u.end ?? utterances[i + 1]?.start ?? Infinity
      if (currentTime >= u.start && currentTime <= end) {
        if (u.start >= bestStart) {
          bestStart = u.start
          bestIdx = i
        }
      }
    }
    if (bestIdx >= 0) return bestIdx

    // В коротких паузах между репликами держим последнюю отзвучавшую (до 1.5 с)
    for (let i = utterances.length - 1; i >= 0; i--) {
      const u = utterances[i]
      const end = u.end ?? u.start
      if (currentTime >= end && currentTime - end <= 1.5) {
        return i
      }
    }
    return -1
  })

  $effect(() => {
    if (follow && activeIndex >= 0) scrollActive()
  })

  function scrollActive(): void {
    doc?.querySelector(`[data-utt="${activeIndex}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' })
  }

  function disableFollow(): void {
    follow = false
  }

  async function applyChange(change: { index?: number; text?: string; speaker?: string; old_speaker?: string }): Promise<boolean> {
    if (!file || saving) return false
    saving = true
    try {
      await editTranscript(file, change)
      return true
    } catch (e) {
      toast('Не удалось сохранить правку: ' + (e as Error).message, 'error')
      return false
    } finally {
      saving = false
    }
  }

  async function startEdit(index: number, point?: { x: number; y: number }): Promise<void> {
    if (saving || index < 0 || index >= utterances.length) return
    if (editingIndex >= 0 && editingIndex !== index && !(await saveText(editingIndex, false))) return
    speakerMenu = null
    editingIndex = index
    draft = utterances[index].text || ''
    await tick()
    if (!editor) return
    editor.focus({ preventScroll: true })

    let range: Range | null = null
    const d = document as Document & {
      caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null
      caretRangeFromPoint?: (x: number, y: number) => Range | null
    }
    if (point && d.caretPositionFromPoint) {
      const pos = d.caretPositionFromPoint(point.x, point.y)
      if (pos) {
        range = document.createRange()
        range.setStart(pos.offsetNode, pos.offset)
      }
    } else if (point && d.caretRangeFromPoint) {
      range = d.caretRangeFromPoint(point.x, point.y)
    }
    if (!range) {
      range = document.createRange()
      range.selectNodeContents(editor)
      range.collapse(!point)
    } else {
      range.collapse(true)
    }
    const selection = window.getSelection()
    selection?.removeAllRanges()
    selection?.addRange(range)
  }

  async function saveText(index: number, moveNext: boolean): Promise<boolean> {
    if (editingIndex !== index || saving) return false
    if (!(await applyChange({ index, text: draft }))) return false
    editingIndex = -1
    const next = utterances[index + 1]
    const query = search.trim().toLowerCase()
    const nextMatches = next && `${next.speaker || ''} ${next.text || ''}`.toLowerCase().includes(query)
    if (moveNext && next && (!query || nextMatches)) await startEdit(index + 1)
    return true
  }

  function onEditorKey(e: KeyboardEvent, index: number): void {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void saveText(index, true)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      draft = utterances[index]?.text || ''
      editingIndex = -1
    }
  }

  async function openSpeakerMenu(e: MouseEvent, index: number): Promise<void> {
    if (speakerMenu?.index === index) {
      speakerMenu = null
      return
    }
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
    speakerMenu = {
      index,
      x: Math.max(8, Math.min(rect.left, window.innerWidth - 270)),
      y: Math.max(8, Math.min(rect.bottom + 6, window.innerHeight - 330)),
    }
    renameValue = utterances[index]?.speaker || 'Спикер'
    await tick()
    menuInput?.focus()
  }

  async function renameSpeaker(): Promise<void> {
    if (!speakerMenu || !renameValue.trim()) return
    if (await applyChange({ old_speaker: menuSpeaker, speaker: renameValue.trim() })) speakerMenu = null
  }

  async function assignSpeaker(name: string): Promise<void> {
    if (!speakerMenu) return
    if (await applyChange({ index: speakerMenu.index, speaker: name })) speakerMenu = null
  }

  function addSpeaker(): void {
    let n = 1
    while (speakerNames.includes(`Спикер ${n}`)) n += 1
    void assignSpeaker(`Спикер ${n}`)
  }

  function onMenuKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      speakerMenu = null
    } else if (e.key === 'Enter' && e.target instanceof HTMLInputElement) {
      e.preventDefault()
      void renameSpeaker()
    }
  }

  function escapeHtml(s: string): string {
    return String(s).replace(/[&<>"']/g, (c) => (({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c))
  }

  function inlineMd(s: string): string {
    return escapeHtml(s)
      .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
      .replace(/`([^`]+)`/g, '<code>$1</code>')
      .replace(/\[([^\]]+)\]\((https?:[^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
  }

  interface Block {
    kind: 'utt'
    time: string
    speaker: string
    html: string
    search: string
    sec: number
    index: number
  }

  interface Raw {
    kind: 'h1' | 'h2' | 'h3' | 'p' | 'hr' | 'ul'
    text?: string
    items?: string[]
  }

  // Структурированные блоки: utterances — источник истины, иначе лёгкий разбор md.
  let blocks = $derived.by((): (Block | Raw)[] => {
    if (utterances.length > 0) {
      return utterances.map((u, index) => ({
        kind: 'utt' as const,
        time: fmtClockShort(u.start),
        speaker: u.speaker || 'Спикер',
        html: escapeHtml(u.text || ''),
        search: `${fmtClockShort(u.start)} ${u.speaker || ''} ${u.text || ''}`.toLowerCase(),
        sec: u.start,
        index,
      }))
    }
    const out: (Block | Raw)[] = []
    let list: string[] | null = null
    const flush = () => {
      if (list) {
        out.push({ kind: 'ul', items: list })
        list = null
      }
    }
    for (const line of String(markdown).split('\n')) {
      const t = line.trim()
      if (!t) {
        flush()
        continue
      }
      if (t === '---') {
        flush()
        out.push({ kind: 'hr' })
        continue
      }
      if (t.startsWith('### ')) {
        flush()
        out.push({ kind: 'h3', text: t.slice(4) })
        continue
      }
      if (t.startsWith('## ')) {
        flush()
        out.push({ kind: 'h2', text: t.slice(3) })
        continue
      }
      if (t.startsWith('# ')) {
        flush()
        out.push({ kind: 'h1', text: t.slice(2) })
        continue
      }
      const m = t.match(/^\*\*\[([^\]]+)\]\s+([^:]+):\*\*\s?(.*)$/)
      if (m) {
        flush()
        const p = m[1].split(':').map(Number)
        const sec = p.length === 2 ? p[0] * 60 + p[1] : p.length === 3 ? p[0] * 3600 + p[1] * 60 + p[2] : 0
        out.push({
          kind: 'utt',
          time: `[${m[1]}]`,
          speaker: m[2],
          html: inlineMd(m[3]),
          search: `${m[1]} ${m[2]} ${m[3]}`.toLowerCase(),
          sec,
          index: -1,
        } as Block)
        continue
      }
      const li = t.match(/^[-*]\s+(.*)$/)
      if (li) {
        ;(list ??= []).push(li[1])
        continue
      }
      flush()
      const isMeta = /^\*\*.+\*\*$/.test(t)
      out.push({ kind: 'p', text: t, items: isMeta ? ['meta'] : [] })
    }
    flush()
    return out
  })

  let q = $derived(search.trim().toLowerCase())
  let uttTotal = $derived(blocks.filter((b) => b.kind === 'utt').length)
  let hitCount = $derived(
    q ? blocks.filter((b) => b.kind === 'utt' && (b as Block).search.includes(q)).length : 0,
  )

</script>

<svelte:window onwheel={disableFollow} ontouchstart={disableFollow} onkeydown={disableFollow} />

<div class="transcript-doc" bind:this={doc}>
  <div class="transcript-doc-header">
    <div class="pane-title">Текст расшифровки</div>
    <div class="doc-search-box">
      <svg class="search-icon" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="11" cy="11" r="8"></circle>
        <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
      </svg>
      <input
        type="search"
        class="doc-search-input"
        placeholder="Поиск по тексту…"
        autocomplete="off"
        aria-label="Поиск по тексту расшифровки"
        bind:value={search}
      />
      {#if search.trim()}
        {#if uttTotal > 0}
          <span class="doc-search-count">{hitCount} из {uttTotal}</span>
        {/if}
        <button
          type="button"
          class="doc-search-clear"
          onclick={() => (search = '')}
          aria-label="Очистить поиск"
          title="Очистить"
        >
          ✕
        </button>
      {/if}
    </div>
  </div>
  <div class="markdown-body">
    {#if meta && utterances && utterances.length > 0}
      <div class="print-meta-header">
        <h1 class="print-meta-title">Расшифровка встречи: {meta.title}</h1>
        <div class="print-meta-line">
          <strong>Источник:</strong> <code>{meta.source}</code>{#if meta.durStr} ({meta.durStr}){/if}
        </div>
        {#if meta.speakers && meta.speakers.length > 0}
          <div class="print-meta-line">
            <strong>Спикеры:</strong> {meta.speakers.join(', ')}
          </div>
        {/if}
        <hr class="print-meta-divider" />
      </div>
    {/if}
    {#each blocks as b, i (i)}
      {#if b.kind === 'utt' && (!q || (b as Block).search.includes(q))}
        <div
          class="utt utterance {(b as Block).search.includes(q) && q ? 'highlight' : ''} {(b as Block).index === activeIndex ? 'active' : ''}"
          data-utt={(b as Block).index}
          style={`--spk: ${speakerColor((b as Block).speaker)}; --spk-bg: ${speakerTint((b as Block).speaker)}`}
        >
          <button
            type="button"
            class="utt-time-btn"
            title="Воспроизвести с этого момента"
            aria-label={`Воспроизвести с ${(b as Block).time}`}
            onclick={() => onseek?.((b as Block).sec)}
          >
            {(b as Block).time}
          </button>
          <div class="utt-body">
            <button
              type="button"
              class="utt-speaker speaker-chip"
              disabled={(b as Block).index < 0}
              title="Управление спикером"
              onclick={(e) => void openSpeakerMenu(e, (b as Block).index)}
            >
              {(b as Block).speaker}
            </button>
            {#if editingIndex === (b as Block).index}
              <div
                class="utt-text editing"
                contenteditable="true"
                role="textbox"
                aria-multiline="true"
                aria-label="Редактировать реплику"
                tabindex="0"
                bind:this={editor}
                bind:innerText={draft}
                onkeydown={(e) => onEditorKey(e, (b as Block).index)}
                onblur={() => {
                  if (editingIndex === (b as Block).index && !saving) void saveText((b as Block).index, false)
                }}
              ></div>
            {:else if (b as Block).index >= 0}
              <div
                class="utt-text editable"
                role="textbox"
                aria-label="Редактировать реплику"
                tabindex="0"
                onclick={(e) => void startEdit((b as Block).index, { x: e.clientX, y: e.clientY })}
                onkeydown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault()
                    void startEdit((b as Block).index)
                  }
                }}
              >{@html (b as Block).html}</div>
            {:else}
              <div class="utt-text">{@html (b as Block).html}</div>
            {/if}
          </div>
        </div>
      {:else if b.kind === 'h1'}
        <h1>{(b as Raw).text}</h1>
      {:else if b.kind === 'h2'}
        <h2>{(b as Raw).text}</h2>
      {:else if b.kind === 'h3'}
        <h3>{(b as Raw).text}</h3>
      {:else if b.kind === 'hr'}
        <hr />
      {:else if b.kind === 'ul'}
        <ul>
          {#each (b as Raw).items ?? [] as item, j (j)}
            <li>{@html inlineMd(item)}</li>
          {/each}
        </ul>
      {:else if b.kind === 'p'}
        {#if (b as Raw).items?.[0] === 'meta'}
          <p class="md-meta">{@html inlineMd((b as Raw).text ?? '')}</p>
        {:else}
          <p>{@html inlineMd((b as Raw).text ?? '')}</p>
        {/if}
      {/if}
    {/each}
    {#if q && hitCount === 0}
      <div class="empty-note">Ничего не найдено</div>
    {/if}
    {#if !follow && activeIndex >= 0}
      <button type="button" class="follow-current" onclick={() => { follow = true; scrollActive() }}>
        К текущей реплике
      </button>
    {/if}
  </div>
  {#if speakerMenu}
    <button type="button" class="speaker-popover-backdrop" aria-label="Закрыть меню спикера" onclick={() => (speakerMenu = null)}></button>
    <div
      class="speaker-popover"
      role="dialog"
      aria-label="Управление спикером"
      tabindex="-1"
      style={`left: ${speakerMenu.x}px; top: ${speakerMenu.y}px`}
      onkeydown={onMenuKey}
    >
      <label class="speaker-rename-label">
        Имя во всей встрече
        <input class="speaker-rename-input" bind:this={menuInput} bind:value={renameValue} maxlength="200" />
      </label>
      <button type="button" class="btn-secondary btn-sm" disabled={saving || !renameValue.trim()} onclick={() => void renameSpeaker()}>
        Переименовать везде
      </button>
      <div class="speaker-menu-title">Спикер этой реплики</div>
      <div class="speaker-menu-list">
        {#each speakerNames as name (name)}
          <button type="button" class="speaker-menu-item" disabled={saving || name === menuSpeaker} onclick={() => void assignSpeaker(name)}>
            {name}
          </button>
        {/each}
      </div>
      <button type="button" class="speaker-menu-item speaker-menu-add" disabled={saving} onclick={addSpeaker}>
        + Спикер
      </button>
    </div>
  {/if}
</div>
