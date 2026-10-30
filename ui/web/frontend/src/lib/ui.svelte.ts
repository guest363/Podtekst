import { isVideoPath } from './api'

// Общее состояние модалок, шторок и текущей расшифровки.
export const ui = $state({
  settingsOpen: false,
  donateOpen: false,
  transcriptOpen: false,
  currentTranscriptPath: '',
  currentTranscriptIsVideo: false,
})

export function openTranscript(path: string, isVideo?: boolean): void {
  const vid = typeof isVideo === 'boolean' ? isVideo : isVideoPath(path)
  ui.currentTranscriptPath = path
  ui.currentTranscriptIsVideo = vid
  ui.transcriptOpen = true
  if (typeof window !== 'undefined') {
    const hash = `#/transcript?path=${encodeURIComponent(path)}&video=${vid ? '1' : '0'}`
    if (window.location.hash !== hash) {
      window.location.hash = hash
    }
  }
}

export function closeTranscript(): void {
  ui.transcriptOpen = false
  ui.currentTranscriptPath = ''
  if (typeof window !== 'undefined' && window.location.hash.startsWith('#/transcript')) {
    history.pushState('', document.title, window.location.pathname + window.location.search)
  }
}

export function initRouting(): void {
  if (typeof window === 'undefined') return

  const syncFromHash = () => {
    const hash = window.location.hash
    if (hash.startsWith('#/transcript')) {
      const qIndex = hash.indexOf('?')
      if (qIndex !== -1) {
        const params = new URLSearchParams(hash.slice(qIndex))
        const path = params.get('path')
        const isVideo = params.get('video') === '1' || (path ? isVideoPath(path) : false)
        if (path) {
          ui.currentTranscriptPath = path
          ui.currentTranscriptIsVideo = isVideo
          ui.transcriptOpen = true
          return
        }
      }
    }
    if (ui.transcriptOpen) {
      ui.transcriptOpen = false
      ui.currentTranscriptPath = ''
    }
  }

  window.addEventListener('hashchange', syncFromHash)
  window.addEventListener('popstate', syncFromHash)
  syncFromHash()
}

type Handler = (data?: unknown) => void
const handlers = new Map<string, Handler[]>()

export function on(evt: string, fn: Handler): void {
  const list = handlers.get(evt) ?? []
  list.push(fn)
  handlers.set(evt, list)
}

export function emit(evt: string, data?: unknown): void {
  for (const fn of handlers.get(evt) ?? []) fn(data)
}
