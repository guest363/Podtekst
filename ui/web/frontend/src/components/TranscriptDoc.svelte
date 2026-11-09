<script lang="ts">
import { tick } from 'svelte'
import type { SpeakerInfo, Utterance } from '../lib/api'
import { fmtClockShort, speakerColor, speakerTint } from '../lib/api'
import { addHotword } from '../lib/settings.svelte.ts'
import { toast } from '../lib/toast.svelte.ts'
import { editTranscript } from '../lib/transcript.svelte.ts'

type Props = {
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
let {
  utterances,
  markdown,
  file = '',
  speakers = [],
  currentTime = 0,
  meta,
  onseek,
}: Props = $props()

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
let wordMenu = $state<{ index: number; word: string; x: number; y: number } | null>(null)
let heardValue = $state('')
let correctValue = $state('')
let menuWordInput: HTMLInputElement | null = $state(null)
let wordMenuEl: HTMLElement | null = $state(null)
let speakerMenuEl: HTMLElement | null = $state(null)

function fitIntoView(
  el: HTMLElement | null,
  pos: { x: number; y: number },
  anchor: { top: number; bottom: number },
): { x: number; y: number } {
  if (!el) return pos
  const r = el.getBoundingClientRect()
  let { x, y } = pos
  if (r.right > window.innerWidth - 8) x = Math.max(8, x - (r.right - (window.innerWidth - 8)))
  if (r.bottom > window.innerHeight - 8) {
    const above = anchor.top - r.height - 6
    y = above >= 8 ? above : Math.max(8, window.innerHeight - r.height - 8)
  }
  return { x, y }
}
let menuSpeaker = $derived(speakerMenu ? utterances[speakerMenu.index]?.speaker || 'Спикер' : '')
let activeIndex = $derived.by(() => {
  if (currentTime <= 0 || !utterances.length) return -1
  // При одновременной речи и перекрытиях каналов выбираем реплику,
  // начавшуюся позже всех (наибольший start <= currentTime).
  let bestIdx = -1
  let bestStart = -1
  for (let i = 0; i < utterances.length; i++) {
    const u = utterances[i]
    const end = u.end ?? utterances[i + 1]?.start ?? Number.POSITIVE_INFINITY
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
  doc
    ?.querySelector(`[data-utt="${activeIndex}"]`)
    ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
}

function disableFollow(): void {
  follow = false
}

async function applyChange(change: {
  index?: number
  text?: string
  speaker?: string
  old_speaker?: string
}): Promise<boolean> {
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
  const nextMatches =
    next && `${next.speaker || ''} ${next.text || ''}`.toLowerCase().includes(query)
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
  const estW = 270
  speakerMenu = {
    index,
    x: Math.max(8, Math.min(rect.left, window.innerWidth - estW - 8)),
    y: rect.bottom + 6,
  }
  renameValue = utterances[index]?.speaker || 'Спикер'
  await tick()
  if (speakerMenu) {
    const fitted = fitIntoView(speakerMenuEl, { x: speakerMenu.x, y: speakerMenu.y }, rect)
    speakerMenu = { ...speakerMenu, ...fitted }
  }
  menuInput?.focus()
  menuInput?.select()
}

async function renameSpeaker(): Promise<void> {
  if (!speakerMenu || !renameValue.trim()) return
  if (await applyChange({ old_speaker: menuSpeaker, speaker: renameValue.trim() }))
    speakerMenu = null
}

async function detachSpeaker(index: number): Promise<void> {
  if (!file || saving || index < 0 || index >= utterances.length) return
  const allSpeakers = new Set(utterances.map((u) => u.speaker || ''))
  let newName = 'Новый собеседник'
  if (allSpeakers.has(newName)) {
    let num = 2
    while (allSpeakers.has(`Новый собеседник ${num}`)) num += 1
    newName = `Новый собеседник ${num}`
  }
  if (await applyChange({ index, speaker: newName })) {
    toast(`Реплика вынесена в карточку «${newName}»`)
    await tick()
    const cards = Array.from(document.querySelectorAll<HTMLElement>('.speaker-card'))
    const targetCard =
      cards.find((c) => c.querySelector('.speaker-name')?.textContent?.includes(newName)) ||
      document.querySelector<HTMLElement>('.speaker-card:not(.matched)')
    const input = targetCard?.querySelector<HTMLInputElement>('.assign-name-input')
    input?.focus()
    targetCard?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
  }
}

async function onUttTextClick(e: MouseEvent, index: number): Promise<void> {
  const t = (e.target as HTMLElement).closest?.('[data-w]') as HTMLElement | null
  if (t && index >= 0) {
    e.stopPropagation()
    await openWordMenu(e, index, t.dataset.w || '')
  }
}

async function openWordMenu(e: MouseEvent, index: number, word: string): Promise<void> {
  if (!word) return
  const anchor = (e.target as HTMLElement).getBoundingClientRect()
  const estW = 250
  wordMenu = {
    index,
    word,
    x: Math.max(8, Math.min(anchor.left, window.innerWidth - estW - 8)),
    y: anchor.bottom + 6,
  }
  heardValue = word
  correctValue = ''
  await tick()
  if (wordMenu) {
    const fitted = fitIntoView(wordMenuEl, { x: wordMenu.x, y: wordMenu.y }, anchor)
    wordMenu = { ...wordMenu, ...fitted }
  }
  menuWordInput?.focus()
}

async function saveWord(): Promise<void> {
  if (!wordMenu) return
  const heard = heardValue.trim()
  const correct = correctValue.trim()
  if (!heard) return
  const entry =
    correct && correct.toLowerCase() !== heard.toLowerCase() ? `${heard} => ${correct}` : heard
  if (await addHotword(entry)) wordMenu = null
}

function onWordMenuKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    wordMenu = null
  } else if (e.key === 'Enter' && e.target instanceof HTMLInputElement) {
    e.preventDefault()
    void saveWord()
  }
}

function onMenuKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    speakerMenu = null
  } else if (e.key === 'Enter') {
    e.preventDefault()
    void renameSpeaker()
  }
}

function escapeHtml(s: string): string {
  return String(s).replace(
    /[&<>"']/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c,
  )
}

function wordsHtml(u: Utterance): string {
  const ws = u.words
  const plain = (u.text || '').split(/\s+/).filter(Boolean)
  if (!ws || ws.length === 0 || ws.length !== plain.length) return escapeHtml(u.text || '')
  return ws
    .map((w) => {
      const cls = w.needs_review ? 'w low' : 'w'
      const conf = typeof w.conf === 'number' ? w.conf.toFixed(2) : '?'
      const title = w.needs_review
        ? `Уверенность ${conf} — клик: добавить в словарь`
        : 'Клик: добавить в словарь'
      return `<span class="${cls}" data-w="${escapeHtml(w.w)}" title="${escapeHtml(title)}">${escapeHtml(w.w)}</span>`
    })
    .join(' ')
}

function inlineMd(s: string): string {
  return escapeHtml(s)
    .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(
      /\[([^\]]+)\]\((https?:[^)]+)\)/g,
      '<a href="$2" target="_blank" rel="noopener">$1</a>',
    )
}

type Block = {
  kind: 'utt'
  time: string
  speaker: string
  html: string
  search: string
  sec: number
  index: number
}

type Raw = {
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
      html: wordsHtml(u),
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
      const sec =
        p.length === 2 ? p[0] * 60 + p[1] : p.length === 3 ? p[0] * 3600 + p[1] * 60 + p[2] : 0
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
      list ??= []
      list.push(li[1])
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
            {#if (b as Block).index >= 0 && editingIndex !== (b as Block).index}
              <div class="utt-actions">
                <button
                  type="button"
                  class="utt-detach-btn"
                  title="Вынести реплику в отдельного спикера"
                  onclick={() => void detachSpeaker((b as Block).index)}
                >
                  <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/>
                    <circle cx="9" cy="7" r="4"/>
                    <line x1="19" y1="8" x2="19" y2="14"/>
                    <line x1="22" y1="11" x2="16" y2="11"/>
                  </svg>
                  <span>Другой спикер</span>
                </button>
                <button
                  type="button"
                  class="utt-edit-btn"
                  title="Редактировать реплику"
                  aria-label="Редактировать реплику"
                  onclick={() => void startEdit((b as Block).index)}
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"></path>
                  </svg>
                </button>
              </div>
            {/if}
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
                class="utt-text"
                role="textbox"
                aria-label="Редактировать реплику"
                tabindex="0"
                onclick={(e) => void onUttTextClick(e, (b as Block).index)}
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
  </div>
  {#if !follow && activeIndex >= 0}
    <button type="button" class="follow-current" onclick={() => { follow = true; scrollActive() }}>
      К текущей реплике
    </button>
  {/if}
  {#if speakerMenu}
    <button type="button" class="speaker-popover-backdrop" aria-label="Закрыть меню спикера" onclick={() => (speakerMenu = null)}></button>
    <div
      class="speaker-popover speaker-rename-popover"
      role="dialog"
      aria-label="Переименовать спикера"
      tabindex="-1"
      bind:this={speakerMenuEl}
      style={`left: ${speakerMenu.x}px; top: ${speakerMenu.y}px`}
      onkeydown={onMenuKey}
    >
      <div class="speaker-rename-title">Переименовать во всей встрече</div>
      <div class="speaker-rename-row">
        <input
          class="speaker-rename-input"
          bind:this={menuInput}
          bind:value={renameValue}
          maxlength="200"
          placeholder="Имя спикера"
          aria-label="Новое имя спикера"
        />
        <button
          type="button"
          class="btn-primary btn-sm"
          disabled={saving || !renameValue.trim() || renameValue.trim() === menuSpeaker}
          onclick={() => void renameSpeaker()}
        >
          Готово
        </button>
      </div>
    </div>
  {/if}
  {#if wordMenu}
    <button type="button" class="speaker-popover-backdrop" aria-label="Закрыть словарь" onclick={() => (wordMenu = null)}></button>
    <div
      class="speaker-popover"
      role="dialog"
      aria-label="Добавить термин в словарь"
      tabindex="-1"
      bind:this={wordMenuEl}
      style={`left: ${wordMenu.x}px; top: ${wordMenu.y}px`}
      onkeydown={onWordMenuKey}
    >
      <label class="speaker-rename-label">
        Услышано
        <input class="speaker-rename-input" bind:this={menuWordInput} bind:value={heardValue} maxlength="64" />
      </label>
      <label class="speaker-rename-label">
        Правильно (пусто — просто бонус этому слову)
        <input
          class="speaker-rename-input"
          bind:value={correctValue}
          maxlength="64"
          placeholder="Например, фикс багов"
          aria-label="Правильное написание"
        />
      </label>
      <div class="help-text">Связка «услышано → правильно» исправляет текст точно. Без второго поля — только бонус.</div>
      <button type="button" class="btn-secondary btn-sm" onclick={() => void saveWord()}>
        В словарь
      </button>
    </div>
  {/if}
</div>

<style>
  /* Классы живут в {@html}-строках: :global, иначе svelte-check сочтёт их мёртвыми. */
  :global(.w) {
    cursor: pointer;
    border-radius: 3px;
    padding: 0 1px;
  }
  :global(.w:hover) {
    background: #faf6e9;
  }
  :global(.w.low) {
    border-bottom: 1px dotted #b08d2e;
  }
  .utt-actions {
    position: absolute;
    top: 5px;
    right: 8px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    opacity: 0;
    pointer-events: none;
    transition: opacity 140ms ease;
    z-index: 2;
  }
  .utterance:hover .utt-actions,
  .utt-actions:focus-within {
    opacity: 1;
    pointer-events: auto;
  }
  .utt-detach-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px;
    font-size: 11px;
    font-weight: 600;
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-pill);
    background: rgba(255, 255, 255, 0.92);
    color: var(--text-muted);
    cursor: pointer;
    line-height: 1.3;
    white-space: nowrap;
    box-shadow: 0 1px 3px rgba(15, 30, 51, 0.05);
    backdrop-filter: blur(4px);
    transition: all 140ms ease;
  }
  .utt-detach-btn svg {
    color: var(--text-faint);
    transition: color 140ms ease;
  }
  .utt-detach-btn:hover {
    background: #fff;
    color: var(--accent-blue);
    border-color: rgba(36, 86, 198, 0.3);
    box-shadow: 0 2px 8px rgba(36, 86, 198, 0.12);
  }
  .utt-detach-btn:hover svg {
    color: var(--accent-blue);
  }
  .utt-edit-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: #999;
    cursor: pointer;
    transition: color 140ms ease, background 140ms ease;
  }
  .utt-edit-btn:hover {
    background: #edece8;
    color: #444;
    outline: none;
  }
  @media (hover: none) {
    .utt-actions {
      opacity: 0.65;
      pointer-events: auto;
    }
  }
</style>
