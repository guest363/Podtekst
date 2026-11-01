<script lang="ts">
  import { baseName, fmtClock, fmtClockShort, isMobile, pluralSpeakers } from '../lib/api'
  import { copyText, toast } from '../lib/toast.svelte.ts'
  import { closeTranscript, ui } from '../lib/ui.svelte.ts'
  import { emptyHint, engine, openTranscriptFile, startTranscription, transcript } from '../lib/transcript.svelte.ts'
  import { extractMp3 } from '../lib/recordings.svelte.ts'
  import { peopleState } from '../lib/people.svelte.ts'
  import { exportDocx, printTranscript } from '../lib/export.ts'
  import SlimPlayer from './SlimPlayer.svelte'
  import SpeakersPane from './SpeakersPane.svelte'
  import TranscriptDoc from './TranscriptDoc.svelte'

  // Сайдбар спикеров: на десктопе док-панель слева, на телефоне drawer. По умолчанию открыт.
  let speakersOpen = $state(
    typeof localStorage !== 'undefined'
      ? localStorage.getItem('mr_speakers_open') !== 'false'
      : true,
  )

  function toggleSpeakers(): void {
    speakersOpen = !speakersOpen
    try {
      localStorage.setItem('mr_speakers_open', String(speakersOpen))
    } catch {
      /* ignore */
    }
  }

  let videoExpanded = $state(false)
  let playbackTime = $state(0)

  // Расшифровывается ли именно эта запись (или движок занят другой).
  let thisRunning = $derived(engine.running && engine.file === ui.currentTranscriptPath)
  let otherRunning = $derived(engine.running && engine.file !== ui.currentTranscriptPath)

  // Загрузка транскрипта при переходе на страницу.
  $effect(() => {
    if (ui.transcriptOpen && ui.currentTranscriptPath) {
      videoExpanded = false
      playbackTime = 0
      void openTranscriptFile(ui.currentTranscriptPath)
    }
  })



  function pluralUtterances(n: number): string {
    const rem10 = n % 10
    const rem100 = n % 100
    if (rem100 >= 11 && rem100 <= 19) return 'реплик'
    if (rem10 === 1) return 'реплика'
    if (rem10 >= 2 && rem10 <= 4) return 'реплики'
    return 'реплик'
  }

  function getTranscriptMeta(): { title: string; source: string; durStr: string; speakers: string[] } {
    const filename = baseName(ui.currentTranscriptPath) || 'Встреча'
    const lastUtt = transcript.utterances?.[transcript.utterances.length - 1]
    const durStr = lastUtt && lastUtt.end ? fmtClock(lastUtt.end) : ''

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

  function buildFullTranscriptMarkdown(): string {
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

  async function copyAll(): Promise<void> {
    const txt = buildFullTranscriptMarkdown()
    if (!txt.trim()) {
      toast('Нечего копировать: текст пуст')
      return
    }
    if (await copyText(txt)) toast('Текст скопирован')
    else toast('Не удалось скопировать', 'error')
  }

  function seekTo(sec: number): void {
    const media = document.getElementById('transcriptFullMedia') as HTMLMediaElement | null
    if (media && isFinite(sec)) {
      media.currentTime = Math.max(0, sec)
      if (media.paused) {
        void media.play().catch(() => {})
      }
    }
  }

  function goBack(): void {
    document
      .querySelectorAll('.transcript-page audio, .transcript-page video')
      .forEach((m) => (m as HTMLMediaElement).pause())
    closeTranscript()
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (!ui.transcriptOpen) return
    if (e.key === 'Escape') {
      if (isMobile && speakersOpen) {
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
  <header class="transcript-page-header">
    <!-- Строка 1: Заголовок и кнопка закрытия на одной линии -->
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

    <!-- Строка 2: Слева полная запись, справа чипы и кнопки -->
    <div class="transcript-header-sub">
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
          <button
            class="btn-secondary btn-sm btn-speakers-toggle {speakersOpen ? 'active' : ''}"
            aria-label="Спикеры встречи"
            title={speakersOpen ? 'Скрыть панель спикеров' : 'Показать панель спикеров'}
            onclick={toggleSpeakers}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path>
              <circle cx="9" cy="7" r="4"></circle>
              <path d="M23 21v-2a4 4 0 0 0-3-3.87"></path>
              <path d="M16 3.13a4 4 0 0 1 0 7.75"></path>
            </svg>
            <span>Спикеры{#if transcript.speakers.length > 0}&nbsp;· {transcript.speakers.length}{/if}</span>
          </button>

          <button
            class="btn-primary btn-sm btn-transcribe-header"
            disabled={engine.running}
            title={otherRunning ? `Уже расшифровывается «${baseName(engine.file)}»` : ''}
            onclick={() => void startTranscription(ui.currentTranscriptPath)}
          >
            <span class="btn-text-full">Запустить расшифровку</span>
            <span class="btn-text-short">Расшифровать</span>
          </button>

          {#if transcript.extractVisible && !isMobile}
            <button class="btn-secondary btn-sm desktop-only-btn" onclick={() => void extractMp3(ui.currentTranscriptPath)}>
              Извлечь MP3
            </button>
          {/if}

          <button
            class="btn-secondary btn-sm"
            disabled={!transcript.utterances.length}
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
          Дождитесь окончания расшифровки «{baseName(engine.file)}» – затем запустите эту.
        </div>
      {:else}
        <div class="help-text">{emptyHint()}</div>
      {/if}
      <div class="row-start">
        <button
          class="btn-primary"
          disabled={engine.running}
          onclick={() => void startTranscription(ui.currentTranscriptPath)}
        >
          Запустить расшифровку
        </button>
      </div>
    </div>
  {:else}
    <div class="transcript-grid {speakersOpen ? 'sidebar-open' : ''}">
      {#if isMobile && speakersOpen}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="speakers-drawer-overlay"
          role="presentation"
          onclick={() => (speakersOpen = false)}
        ></div>
      {/if}

      <div class="transcript-speakers-wrap {speakersOpen ? 'open' : ''}">
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
        <SpeakersPane speakers={transcript.speakers} />
      </div>

      <TranscriptDoc
        utterances={transcript.utterances}
        markdown={transcript.markdown}
        file={ui.currentTranscriptPath}
        speakers={transcript.speakers}
        currentTime={playbackTime}
        meta={transcriptMeta}
        onseek={seekTo}
      />
    </div>
  {/if}
</div>
