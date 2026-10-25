<script lang="ts">
  import { fmtClockShort, speakerColor, speakerTint } from '../lib/api'
  import type { Utterance } from '../lib/api'

  interface Props {
    utterances: Utterance[]
    markdown: string
    onseek?: (sec: number) => void
  }
  let { utterances, markdown, onseek }: Props = $props()

  let search = $state('')

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
  }

  interface Raw {
    kind: 'h1' | 'h2' | 'h3' | 'p' | 'hr' | 'ul'
    text?: string
    items?: string[]
  }

  // Структурированные блоки: utterances — источник истины, иначе лёгкий разбор md.
  let blocks = $derived.by((): (Block | Raw)[] => {
    if (utterances.length > 0) {
      return utterances.map((u) => ({
        kind: 'utt' as const,
        time: fmtClockShort(u.start),
        speaker: u.speaker || 'Спикер',
        html: escapeHtml(u.text || ''),
        search: `${fmtClockShort(u.start)} ${u.speaker || ''} ${u.text || ''}`.toLowerCase(),
        sec: u.start,
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

<div class="transcript-doc">
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
    {#each blocks as b, i (i)}
      {#if b.kind === 'utt' && (!q || (b as Block).search.includes(q))}
        <div
          class="utt utterance {(b as Block).search.includes(q) && q ? 'highlight' : ''}"
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
            <div class="utt-speaker">{(b as Block).speaker}</div>
            <div class="utt-text">{@html (b as Block).html}</div>
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
</div>
