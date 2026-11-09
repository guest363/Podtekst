import { isVideoPath } from './api'

export type AppTab = 'studio' | 'queue' | 'people' | 'dict' | 'settings'

type UiState = {
  activeTab: AppTab
  settingsOpen: boolean
  systemOpen: boolean
  donateOpen: boolean
  transcriptOpen: boolean
  currentTranscriptPath: string
  currentTranscriptIsVideo: boolean
}

export const ui: UiState = $state({
  activeTab: 'studio',
  settingsOpen: false,
  systemOpen: false,
  donateOpen: false,
  transcriptOpen: false,
  currentTranscriptPath: '',
  currentTranscriptIsVideo: false,
})

export const selectTab = (tab: AppTab): void => {
  ui.activeTab = tab
  if (ui.transcriptOpen) {
    ui.transcriptOpen = false
    ui.currentTranscriptPath = ''
  }
  if (typeof window !== 'undefined') {
    const hash = tab === 'studio' ? '#/' : `#/${tab}`
    if (window.location.hash !== hash) {
      window.location.hash = hash
    }
  }
}

export const openTranscript = (path: string, isVideo?: boolean): void => {
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

export const closeTranscript = (): void => {
  ui.transcriptOpen = false
  ui.currentTranscriptPath = ''
  if (typeof window !== 'undefined' && window.location.hash.startsWith('#/transcript')) {
    const targetHash = ui.activeTab === 'studio' ? '#/' : `#/${ui.activeTab}`
    window.location.hash = targetHash
  }
}

export const initRouting = (): void => {
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
    } else if (hash === '#/queue') {
      ui.activeTab = 'queue'
    } else if (hash === '#/people') {
      ui.activeTab = 'people'
    } else if (hash === '#/dict') {
      ui.activeTab = 'dict'
    } else if (hash === '#/settings') {
      ui.activeTab = 'settings'
    } else {
      ui.activeTab = 'studio'
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

export const on = (evt: string, fn: Handler): void => {
  const list = handlers.get(evt) ?? []
  list.push(fn)
  handlers.set(evt, list)
}

export const emit = (evt: string, data?: unknown): void => {
  for (const fn of handlers.get(evt) ?? []) fn(data)
}
