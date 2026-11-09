import type { InlineSegment } from './api'

function escapeHtml(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c,
  )
}

function inline(s: string): string {
  return escapeHtml(s)
    .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(
      /\[([^\]]+)\]\((https?:[^)]+)\)/g,
      '<a href="$2" target="_blank" rel="noopener">$1</a>',
    )
}

export function renderMarkdown(src: string): string {
  const lines = String(src).replace(/\r/g, '').split('\n')
  const out: string[] = []
  let list: 'ul' | 'ol' | null = null
  let para: string[] = []

  const flushPara = () => {
    if (para.length) {
      out.push(`<p>${inline(para.join(' '))}</p>`)
      para = []
    }
  }
  const flushList = () => {
    if (list) {
      out.push(`</${list}>`)
      list = null
    }
  }
  const openList = (kind: 'ul' | 'ol') => {
    flushPara()
    if (list !== kind) {
      flushList()
      out.push(`<${kind}>`)
      list = kind
    }
  }

  for (const raw of lines) {
    const line = raw.trim()
    if (!line) {
      flushPara()
      flushList()
      continue
    }
    if (line === '---') {
      flushPara()
      flushList()
      out.push('<hr>')
      continue
    }
    const heading = line.match(/^(#{1,3})\s+(.+)$/)
    if (heading) {
      flushPara()
      flushList()
      const level = heading[1].length
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`)
      continue
    }
    const ordered = line.match(/^\d+\.\s+(.+)$/)
    if (ordered) {
      openList('ol')
      out.push(`<li>${inline(ordered[1])}</li>`)
      continue
    }
    const bullet = line.match(/^[-*]\s+(.+)$/)
    if (bullet) {
      openList('ul')
      out.push(`<li>${inline(bullet[1])}</li>`)
      continue
    }
    flushList()
    para.push(line)
  }
  flushPara()
  flushList()
  return out.join('')
}

/** Безопасные inline-сегменты для итогов: модель не может вставить HTML,
 *  ссылку или обработчик. Svelte рендерит только текст/strong/code. */
export function renderInline(text: string): InlineSegment[] {
  const src = String(text ?? '')
  const out: InlineSegment[] = []
  let buf = ''
  let i = 0
  const flush = () => {
    if (buf) {
      out.push({ kind: 'text', value: buf })
      buf = ''
    }
  }
  while (i < src.length) {
    if (src.startsWith('**', i)) {
      const end = src.indexOf('**', i + 2)
      if (end > i + 2) {
        flush()
        out.push({ kind: 'bold', value: src.slice(i + 2, end) })
        i = end + 2
        continue
      }
    }
    if (src[i] === '`') {
      const end = src.indexOf('`', i + 1)
      if (end > i + 1) {
        flush()
        out.push({ kind: 'code', value: src.slice(i + 1, end) })
        i = end + 1
        continue
      }
    }
    if (src[i] === '[') {
      const m = /^\[([^\]]*)\]\(([^)]*)\)/.exec(src.slice(i))
      if (m) {
        flush()
        if (m[1]) out.push({ kind: 'text', value: m[1] })
        i += m[0].length
        continue
      }
    }
    buf += src[i]
    i += 1
  }
  flush()
  return out
}
