<script lang="ts">
import { tick } from 'svelte'
import { baseName, fmtClock, fmtClockShort, isAndroid, isMobile, pluralSpeakers } from '../lib/api'
import { exportDocx, printTranscript } from '../lib/export.ts'
import { peopleState } from '../lib/people.svelte.ts'
import { isQueued } from '../lib/queue.svelte.ts'
import { extractMp3 } from '../lib/recordings.svelte.ts'
import { settings } from '../lib/settings.svelte.ts'
import { cancelSummary, summaryState } from '../lib/summary.svelte.ts'
import { copyText, toast, toastConfirm } from '../lib/toast.svelte.ts'
import {
  emptyHint,
  engine,
  openTranscriptFile,
  startTranscription,
  transcript,
} from '../lib/transcript.svelte.ts'
import { closeTranscript, ui } from '../lib/ui.svelte.ts'
import MetricsPanel from './metrics-panel.svelte'
import SlimPlayer from './slim-player.svelte'
import SpeakersPane from './speakers-pane.svelte'
import SummaryPanel from './summary-panel.svelte'
import TranscriptDoc from './transcript-doc.svelte'

const initialSpeakersDrawer =
  typeof window !== 'undefined' && window.matchMedia('(max-width: 720px)').matches
const initialPanelDrawer =
  typeof window !== 'undefined' && window.matchMedia('(max-width: 1200px)').matches

let speakersOpen = $state(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem('mr_speakers_open') !== 'false' && !initialSpeakersDrawer
    : true,
)

// v2: старый ключ хранил «скрыто» по умолчанию и не должен отменять новый дефолт.
const SUMMARY_OPEN_KEY = 'mr_summary_open_v2'
const storedSummaryOpen =
  typeof localStorage !== 'undefined' ? localStorage.getItem(SUMMARY_OPEN_KEY) : null

let summaryOpen = $state(
  storedSummaryOpen === null ? !initialPanelDrawer : storedSummaryOpen === 'true',
)

let metricsOpen = $state(false)
let metricsBtn = $state<HTMLButtonElement | null>(null)
let metricsPanelEl = $state<HTMLElement | null>(null)
let narrow = $state(false)
let panelDrawer = $state(false)

$effect(() => {
  const query = window.matchMedia('(max-width: 720px)')
  const sync = (): void => {
    narrow = query.matches
  }
  sync()
  query.addEventListener('change', sync)
  return () => query.removeEventListener('change', sync)
})

$effect(() => {
  const query = window.matchMedia('(max-width: 1200px)')
  const sync = (): void => {
    panelDrawer = query.matches
  }
  sync()
  query.addEventListener('change', sync)
  return () => query.removeEventListener('change', sync)
})

$effect(() => {
  if (metricsOpen) {
    void tick().then(() => {
      const scrollable = metricsPanelEl?.querySelector<HTMLElement>('.metrics-panel')
      ;(scrollable ?? metricsPanelEl)?.focus()
    })
  }
})

$effect(() => {
  try {
    localStorage.setItem(SUMMARY_OPEN_KEY, String(summaryOpen))
  } catch {
    /* ignore */
  }
})

let summaryAvailable = $derived(
  !isAndroid &&
    settings.summaryFeature !== false &&
    transcript.hasText &&
    summaryState.capabilities?.summary_supported !== false &&
    summaryState.blockReason !== 'no_text',
)
let metricsAvailable = $derived(transcript.hasText)

const toggleSpeakers = (): void => {
  speakersOpen = !speakersOpen
  try {
    localStorage.setItem('mr_speakers_open', String(speakersOpen))
  } catch {
    /* ignore */
  }
}

const toggleSummaryPanel = (): void => {
  summaryOpen = !summaryOpen
  if (summaryOpen) {
    summaryState.expanded = true
    metricsOpen = false
  }
}

const toggleMetricsPanel = (): void => {
  metricsOpen = !metricsOpen
  if (metricsOpen) {
    summaryOpen = false
  }
}

const closeMetricsPanel = (): void => {
  metricsOpen = false
  void tick().then(() => metricsBtn?.focus())
}

let videoExpanded = $state(false)
let playbackTime = $state(0)

let thisRunning = $derived(engine.running && engine.file === ui.currentTranscriptPath)
let otherRunning = $derived(engine.running && engine.file !== ui.currentTranscriptPath)

$effect(() => {
  if (ui.transcriptOpen && ui.currentTranscriptPath) {
    videoExpanded = false
    playbackTime = 0
    metricsOpen = false
    void openTranscriptFile(ui.currentTranscriptPath)
  }
})

$effect(() => {
  if (
    ui.transcriptOpen &&
    ((summaryState.generating && summaryState.file === ui.currentTranscriptPath) ||
      (engine.summaryRunning && engine.summaryFile === ui.currentTranscriptPath))
  ) {
    summaryOpen = true
    summaryState.expanded = true
    metricsOpen = false
  }
})

let hasUnmatchedSpeakers = $derived(
  Boolean(transcript.speakers?.length && transcript.speakers.some((s) => !s.matched)),
)
let allSpeakersMatched = $derived(
  Boolean(transcript.speakers?.length && transcript.speakers.every((s) => s.matched)),
)
const pluralUtterances = (n: number): string => {
  const rem10 = n % 10
  const rem100 = n % 100
  if (rem100 >= 11 && rem100 <= 19) return 'реплик'
  if (rem10 === 1) return 'реплика'
  if (rem10 >= 2 && rem10 <= 4) return 'реплики'
  return 'реплик'
}

const getTranscriptMeta = (): {
  title: string
  source: string
  durStr: string
  speakers: string[]
} => {
  const filename = baseName(ui.currentTranscriptPath) || 'Встреча'
  const lastUtt = transcript.utterances?.[transcript.utterances.length - 1]
  const durStr = lastUtt?.end ? fmtClock(lastUtt.end) : ''

  const speakerMap = new Map<string, { name: string; role?: string }>()
  for (const u of transcript.utterances || []) {
    const name = (u.speaker || 'Спикер').trim()
    if (!name || speakerMap.has(name)) continue
    const person = peopleState.list.find((p) => p.name.toLowerCase() === name.toLowerCase())
    speakerMap.set(name, {
      name,
      role: person?.role?.trim() || undefined,
    })
  }

  const isGeneric = (n: string) => /^(участник|собеседник|спикер)\s*\d*$/i.test(n)
  const identified: string[] = []
  const generic: string[] = []

  for (const [name, info] of speakerMap.entries()) {
    const label = info.role ? `${info.name} (${info.role})` : info.name
    if (info.role || !isGeneric(name)) {
      identified.push(label)
    } else {
      generic.push(label)
    }
  }

  return {
    title: filename,
    source: filename,
    durStr,
    speakers: [...identified, ...generic],
  }
}

let transcriptMeta = $derived(getTranscriptMeta())

const buildFullTranscriptMarkdown = (): string => {
  if (!transcript.utterances || transcript.utterances.length === 0) {
    return transcript.markdown?.trim() || ''
  }

  const meta = transcriptMeta
  const lines: string[] = []
  lines.push(`# Расшифровка встречи: ${meta.title}`)
  lines.push('')
  lines.push(`**Источник:** \`${meta.source}\`${meta.durStr ? ` (${meta.durStr})` : ''}`)
  if (meta.speakers.length > 0) {
    lines.push(`**Спикеры:** ${meta.speakers.join(', ')}`)
  }
  lines.push('')
  lines.push('---')
  lines.push('')

  for (const u of transcript.utterances) {
    const time = fmtClockShort(u.start)
    const speaker = u.speaker || 'Спикер'
    const text = (u.text || '').trim()
    lines.push(`**[${time}] ${speaker}:** ${text}`)
    lines.push('')
  }

  return lines.join('\n').trim()
}

const copyAll = async (): Promise<void> => {
  const txt = buildFullTranscriptMarkdown()
  if (!txt.trim()) {
    toast('Нечего копировать: текст пуст')
    return
  }
  if (await copyText(txt)) toast('Текст скопирован')
  else toast('Не удалось скопировать', 'error')
}

const seekTo = (sec: number): void => {
  const media = document.querySelector<HTMLMediaElement>('#transcriptFullMedia')
  if (media && Number.isFinite(sec)) {
    media.currentTime = Math.max(0, sec)
    if (media.paused) {
      void media.play().catch(() => {})
    }
  }
}

const requestTranscription = (): void => {
  const path = ui.currentTranscriptPath
  if (summaryState.generating && summaryState.file === path) {
    toastConfirm(
      'Идёт сборка итогов. Начать расшифровку заново? Итоги будут остановлены.',
      'Начать',
      () => {
        void cancelSummary()
        void startTranscription(path)
      },
    )
    return
  }
  void startTranscription(path)
}

const goBack = (): void => {
  document
    .querySelectorAll<HTMLMediaElement>('.transcript-page audio, .transcript-page video')
    .forEach((m) => {
      m.pause()
    })
  closeTranscript()
}

const onWindowKey = (e: KeyboardEvent): void => {
  if (!ui.transcriptOpen) return
  if (e.key === 'Tab' && panelDrawer && metricsOpen) {
    const root = metricsPanelEl
    if (root) {
      const nodes = root.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, summary, audio[controls], [contenteditable="true"], [tabindex]:not([tabindex="-1"])',
      )
      if (nodes.length === 0) {
        e.preventDefault()
        root.focus()
      } else {
        const first = nodes[0]
        const last = nodes[nodes.length - 1]
        if (e.shiftKey && (document.activeElement === first || document.activeElement === root)) {
          e.preventDefault()
          last.focus()
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault()
          first.focus()
        }
      }
    }
    return
  }
  if (e.key === 'Escape') {
    if (metricsOpen) {
      closeMetricsPanel()
      e.preventDefault()
      e.stopPropagation()
    } else if (panelDrawer && summaryOpen) {
      summaryOpen = false
      e.stopPropagation()
    } else if (narrow && speakersOpen) {
      speakersOpen = false
      e.stopPropagation()
    } else {
      goBack()
    }
  }
}
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="page transcript-page">
  <header class="transcript-page-header" inert={panelDrawer && metricsOpen}>
    <div class="transcript-header-top">
      <h1 class="transcript-page-title" title={ui.currentTranscriptPath}>
        {baseName(ui.currentTranscriptPath)}
      </h1>
      <button
        class="btn-close-page"
        onclick={goBack}
        title="Закрыть расшифровку (Esc)"
        aria-label="Закрыть расшифровку"
      >
        ✕
      </button>
    </div>

    <div class="transcript-sub-actions">
      <div class="transcript-chips">
        <span class="meta-badge">{ui.currentTranscriptIsVideo ? 'Видео' : 'Аудио'}</span>
        {#if transcript.utterances?.length}
          <span class="meta-item">{transcript.utterances.length} {pluralUtterances(transcript.utterances.length)}</span>
        {/if}
        {#if transcript.speakers?.length}
          <span class="meta-item">{transcript.speakers.length} {pluralSpeakers(transcript.speakers.length)}</span>
        {/if}
      </div>

      <div class="transcript-btns">
        {#if transcript.speakers?.length}
          <button
            class="btn-secondary btn-sm btn-speakers-toggle {speakersOpen ? 'active' : ''}"
            aria-label="Спикеры встречи"
            aria-expanded={speakersOpen}
            aria-controls="speakers-panel"
            title={speakersOpen
              ? 'Скрыть панель спикеров'
              : (hasUnmatchedSpeakers
                ? 'Показать спикеров (есть нераспознанные голоса)'
                : 'Показать панель спикеров')}
            onclick={toggleSpeakers}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path>
              <circle cx="9" cy="7" r="4"></circle>
              <path d="M23 21v-2a4 4 0 0 0-3-3.87"></path>
              <path d="M16 3.13a4 4 0 0 1 0 7.75"></path>
            </svg>
            <span>Спикеры{#if transcript.speakers.length > 0}&nbsp;· {transcript.speakers.length}{/if}</span>
            {#if hasUnmatchedSpeakers}
              <span class="spk-notify-dot warn" aria-hidden="true" title="Есть нераспознанные спикеры"></span>
            {:else if allSpeakersMatched}
              <span class="spk-notify-dot ok" aria-hidden="true" title="Все спикеры распознаны"></span>
            {/if}
          </button>
        {/if}

        {#if summaryAvailable}
          <button
            class="btn-secondary btn-sm btn-summary-toggle {summaryOpen ? 'active' : ''}"
            aria-label="Итоги встречи"
            aria-expanded={summaryOpen}
            aria-controls="summary-panel"
            title={summaryOpen ? 'Скрыть панель итогов' : 'Показать итоги встречи'}
            onclick={toggleSummaryPanel}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
              <polyline points="14 2 14 8 20 8"></polyline>
              <line x1="16" y1="13" x2="8" y2="13"></line>
              <line x1="16" y1="17" x2="8" y2="17"></line>
              <line x1="10" y1="9" x2="8" y2="9"></line>
            </svg>
            <span>Итоги</span>
            {#if summaryState.generating}
              <span class="spk-notify-dot warn" aria-hidden="true" title="Идёт формирование итогов"></span>
            {:else if summaryState.record}
              <span class="spk-notify-dot ok" aria-hidden="true" title="Итоги сформированы"></span>
            {/if}
          </button>
        {/if}

        {#if metricsAvailable}
          <button
            class="btn-secondary btn-sm btn-metrics-toggle {metricsOpen ? 'active' : ''}"
            aria-label="Метрики встречи"
            aria-expanded={metricsOpen}
            aria-controls="metrics-panel"
            bind:this={metricsBtn}
            title={metricsOpen ? 'Скрыть панель метрик' : 'Показать панель метрик'}
            onclick={toggleMetricsPanel}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="4" y1="20" x2="4" y2="11"></line>
              <line x1="10" y1="20" x2="10" y2="4"></line>
              <line x1="16" y1="20" x2="16" y2="14"></line>
              <line x1="21" y1="20" x2="3" y2="20"></line>
            </svg>
            <span>Метрики</span>
            {#if transcript.metricsLoading}
              <span class="spk-notify-dot warn" aria-hidden="true" title="Считаем метрики встречи"></span>
            {/if}
          </button>
        {/if}

        <button
          class="btn-primary btn-sm btn-transcribe-header"
          disabled={isQueued(ui.currentTranscriptPath) || thisRunning}
          title={thisRunning
            ? 'Идёт расшифровка этой записи'
            : otherRunning
              ? (isQueued(ui.currentTranscriptPath)
                ? 'Запись уже в очереди'
                : `Сейчас расшифровывается «${baseName(engine.file)}». Нажмите, чтобы поставить в очередь`)
              : transcript.hasText
                ? 'Текст будет распознан заново и заменит текущий'
                : ''}
          onclick={requestTranscription}
        >
          <span class="btn-text-full">
            {thisRunning
              ? 'Идёт расшифровка'
              : isQueued(ui.currentTranscriptPath)
                ? 'В очереди'
                : otherRunning
                  ? 'Поставить в очередь'
                  : transcript.hasText
                    ? 'Повторить расшифровку'
                    : 'Запустить расшифровку'}
          </span>
          <span class="btn-text-short">
            {thisRunning
              ? 'Расшифровка'
              : isQueued(ui.currentTranscriptPath)
                ? 'В очереди'
                : otherRunning
                  ? 'В очередь'
                  : 'Расшифровать'}
          </span>
        </button>

        {#if transcript.extractVisible && !isMobile}
          <button class="btn-secondary btn-sm desktop-only-btn" onclick={() => void extractMp3(ui.currentTranscriptPath)}>
            Извлечь MP3
          </button>
        {/if}

        <button
          class="btn-secondary btn-sm"
          disabled={!transcript.utterances.length}
          title={transcript.utterances.length ? '' : 'Экспорт появится после расшифровки'}
          onclick={() => void exportDocx(baseName(ui.currentTranscriptPath), transcript.utterances, transcriptMeta)}
        >
          DOCX
        </button>
        <button class="btn-secondary btn-sm" onclick={printTranscript} title="Печать или сохранение в PDF">
          PDF
        </button>
        <button class="btn-secondary btn-sm" onclick={() => void copyAll()} title="Скопировать весь текст расшифровки">
          Копировать текст
        </button>
      </div>
    </div>

    <div class="transcript-sub-player {videoExpanded ? 'is-expanded' : ''}">
      {#if ui.currentTranscriptIsVideo}
        <div class="video-player-container">
          <!-- svelte-ignore a11y_media_has_caption -->
          <video
            controls
            preload="metadata"
            id="transcriptFullMedia"
            src={`/audio/${encodeURIComponent(baseName(ui.currentTranscriptPath))}`}
            aria-label={`Смотреть ${baseName(ui.currentTranscriptPath)}`}
            ontimeupdate={(e) => (playbackTime = e.currentTarget.currentTime)}
            onseeked={(e) => (playbackTime = e.currentTarget.currentTime)}
          ></video>
          <button
            type="button"
            class="btn-video-expand"
            onclick={() => (videoExpanded = !videoExpanded)}
            title={videoExpanded ? 'Свернуть видео' : 'Развернуть видео для просмотра слайдов'}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              {#if videoExpanded}
                <polyline points="4 14 10 14 10 20"></polyline>
                <polyline points="20 10 14 10 14 4"></polyline>
                <line x1="14" y1="10" x2="21" y2="3"></line>
                <line x1="3" y1="21" x2="10" y2="14"></line>
              {:else}
                <polyline points="15 3 21 3 21 9"></polyline>
                <polyline points="9 21 3 21 3 15"></polyline>
                <line x1="21" y1="3" x2="14" y2="10"></line>
                <line x1="3" y1="21" x2="10" y2="14"></line>
              {/if}
            </svg>
            <span>{videoExpanded ? 'Компактно' : 'Слайды'}</span>
          </button>
        </div>
      {:else}
        <SlimPlayer
          src={`/audio/${encodeURIComponent(baseName(ui.currentTranscriptPath))}`}
          isVideo={false}
          fileName={baseName(ui.currentTranscriptPath)}
          mediaId="transcriptFullMedia"
          ontime={(sec) => (playbackTime = sec)}
        />
      {/if}
    </div>
  </header>

  {#if engine.running}
    <div class="transcript-progress" aria-live="polite">
      <div class="transcript-progress-head">
        <span class="transcript-progress-title">
          {otherRunning ? 'Движок занят другой записью' : 'Идёт расшифровка'} · {engine.pct}%
        </span>
        {#if engine.file}
          <span class="transcript-progress-file" title={engine.file}>{baseName(engine.file)}</span>
        {/if}
      </div>
      <div class="help-text">
        {#if otherRunning}
          Дождитесь окончания «{baseName(engine.file)}». {engine.msg}
        {:else}
          {engine.msg}
        {/if}
      </div>
      <div class="vu-track progress-track" role="progressbar" aria-label="Прогресс расшифровки">
        <div class="vu-bar" style={`width: ${engine.pct}%`}></div>
      </div>
    </div>
  {/if}

  {#if transcript.loadError}
    <div class="transcript-empty">
      <div class="transcript-empty-title">Не получилось загрузить</div>
      <div class="help-text">{transcript.loadError}</div>
    </div>
  {:else if !transcript.hasText && !thisRunning}
    <div class="transcript-empty">
      <div class="transcript-empty-title">Здесь пока пусто</div>
      {#if otherRunning}
        <div class="help-text">
          {#if isQueued(ui.currentTranscriptPath)}
            Запись уже ожидает очереди расшифровки (после «{baseName(engine.file)}»).
          {:else}
            Сейчас расшифровывается «{baseName(engine.file)}». Можно поставить эту запись в очередь.
          {/if}
        </div>
      {:else}
        <div class="help-text">{emptyHint()}</div>
      {/if}
      <div class="row-start">
        <button
          class="btn-primary"
          disabled={isQueued(ui.currentTranscriptPath)}
          onclick={requestTranscription}
        >
          {isQueued(ui.currentTranscriptPath)
            ? 'В очереди'
            : otherRunning
              ? 'Поставить в очередь'
              : 'Запустить расшифровку'}
        </button>
      </div>
    </div>
  {:else}
    <div
      class="transcript-grid {speakersOpen ? 'sidebar-open' : ''} {summaryOpen && summaryAvailable ? 'summary-open' : ''} {metricsOpen && metricsAvailable ? 'metrics-open' : ''}"
    >
      {#if narrow && speakersOpen}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="speakers-drawer-overlay"
          role="presentation"
          onclick={() => (speakersOpen = false)}
        ></div>
      {/if}
      {#if panelDrawer && summaryOpen && summaryAvailable}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="speakers-drawer-overlay summary-drawer-overlay"
          role="presentation"
          onclick={() => (summaryOpen = false)}
        ></div>
      {/if}
      {#if panelDrawer && metricsOpen && metricsAvailable}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="speakers-drawer-overlay metrics-drawer-overlay"
          role="presentation"
          onclick={closeMetricsPanel}
        ></div>
      {/if}

      <div
        id="speakers-panel"
        class="transcript-speakers-wrap {speakersOpen ? 'open' : ''}"
        inert={!speakersOpen || (panelDrawer && metricsOpen)}
      >
        <div class="speakers-drawer-top">
          <span class="card-title">Спикеры{#if transcript.speakers.length > 0}&nbsp;· {transcript.speakers.length}{/if}</span>
          <button
            class="btn-close-page"
            aria-label="Скрыть панель спикеров"
            title="Скрыть панель спикеров"
            onclick={() => (speakersOpen = false)}
          >
            ✕
          </button>
        </div>
        <SpeakersPane
          speakers={transcript.speakers}
          metrics={transcript.metrics}
          onseek={seekTo}
        />
      </div>

      <TranscriptDoc
        utterances={transcript.utterances}
        markdown={transcript.markdown}
        file={ui.currentTranscriptPath}
        speakers={transcript.speakers}
        currentTime={playbackTime}
        inert={panelDrawer && metricsOpen}
        meta={transcriptMeta}
        onseek={seekTo}
      />

      {#if summaryAvailable}
        <div
          id="summary-panel"
          class="transcript-summary-wrap {summaryOpen ? 'open drawer-open' : ''}"
          inert={!summaryOpen}
        >
          <div class="summary-drawer-top">
            <span class="card-title">Итоги встречи</span>
            <button
              class="btn-close-page"
              aria-label="Скрыть панель итогов"
              title="Скрыть панель итогов"
              onclick={() => (summaryOpen = false)}
            >
              ✕
            </button>
          </div>
          <SummaryPanel
            file={ui.currentTranscriptPath}
            transcriptMarkdown={buildFullTranscriptMarkdown}
          />
        </div>
      {/if}

      {#if metricsAvailable}
        <div
          id="metrics-panel"
          class="transcript-metrics-wrap {metricsOpen ? 'open drawer-open' : ''}"
          inert={!metricsOpen}
          role={panelDrawer && metricsOpen ? 'dialog' : 'region'}
          aria-modal={panelDrawer && metricsOpen ? 'true' : undefined}
          aria-labelledby="metrics-drawer-title"
          tabindex="-1"
          bind:this={metricsPanelEl}
        >
          <div class="summary-drawer-top">
            <h2 id="metrics-drawer-title" class="card-title">Метрики встречи</h2>
            <button
              class="btn-close-page"
              aria-label="Скрыть панель метрик"
              title="Скрыть панель метрик"
              onclick={closeMetricsPanel}
            >
              ✕
            </button>
          </div>
          <MetricsPanel
            metrics={transcript.metrics}
            loading={transcript.metricsLoading}
            error={transcript.metricsError}
            onseek={seekTo}
          />
        </div>
      {/if}
    </div>
  {/if}
</div>
