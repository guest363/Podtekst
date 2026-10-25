<script lang="ts">
  import { baseName, fmtClock, fmtClockShort, pluralSpeakers } from '../lib/api'
  import { copyText, toast } from '../lib/toast.svelte.ts'
  import { closeTranscript, ui } from '../lib/ui.svelte.ts'
  import { emptyHint, engine, openTranscriptFile, startTranscription, transcript } from '../lib/transcript.svelte.ts'
  import { extractMp3 } from '../lib/recordings.svelte.ts'
  import { peopleState } from '../lib/people.svelte.ts'
  import SlimPlayer from './SlimPlayer.svelte'
  import SpeakersPane from './SpeakersPane.svelte'
  import TranscriptDoc from './TranscriptDoc.svelte'

  let speakersDrawerOpen = $state(false)

  // Расшифровывается ли именно эта запись (или движок занят другой).
  let thisRunning = $derived(engine.running && engine.file === ui.currentTranscriptPath)
  let otherRunning = $derived(engine.running && engine.file !== ui.currentTranscriptPath)

  // Загрузка транскрипта при переходе на страницу.
  $effect(() => {
    if (ui.transcriptOpen && ui.currentTranscriptPath) {
      speakersDrawerOpen = false
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

  function buildFullTranscriptMarkdown(): string {
    if (!transcript.utterances || transcript.utterances.length === 0) {
      return transcript.markdown?.trim() || ''
    }

    // Собираем уникальных спикеров и сопоставляем их с каталогом участников (роли/должности)
    const speakerMap = new Map<string, { name: string; role?: string }>()
    for (const u of transcript.utterances) {
      const name = (u.speaker || 'Спикер').trim()
      if (!name || speakerMap.has(name)) continue
      const person = peopleState.list.find((p) => p.name.toLowerCase() === name.toLowerCase())
      speakerMap.set(name, {
        name,
        role: person?.role?.trim() || undefined,
      })
    }

    // Идентифицированные участники (с ролями или реальными именами) идут первыми
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

    const allSpeakers = [...identified, ...generic]

    const filename = baseName(ui.currentTranscriptPath) || 'Встреча'
    const lastUtt = transcript.utterances[transcript.utterances.length - 1]
    const durStr = lastUtt && lastUtt.end ? fmtClock(lastUtt.end) : ''

    const lines: string[] = []
    lines.push(`# Расшифровка встречи: ${filename}`)
    lines.push('')
    lines.push(`**Источник:** \`${filename}\`${durStr ? ` (${durStr})` : ''}`)
    if (allSpeakers.length > 0) {
      lines.push(`**Спикеры:** ${allSpeakers.join(', ')}`)
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
    speakersDrawerOpen = false
    document
      .querySelectorAll('.transcript-page audio, .transcript-page video')
      .forEach((m) => (m as HTMLMediaElement).pause())
    closeTranscript()
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (!ui.transcriptOpen) return
    if (e.key === 'Escape') {
      if (speakersDrawerOpen) {
        speakersDrawerOpen = false
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
      <div class="transcript-sub-player">
        {#if ui.currentTranscriptIsVideo}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video
            controls
            preload="metadata"
            id="transcriptFullMedia"
            src={`/audio/${encodeURIComponent(baseName(ui.currentTranscriptPath))}`}
            aria-label={`Смотреть ${baseName(ui.currentTranscriptPath)}`}
          ></video>
        {:else}
          <SlimPlayer
            src={`/audio/${encodeURIComponent(baseName(ui.currentTranscriptPath))}`}
            isVideo={false}
            fileName={baseName(ui.currentTranscriptPath)}
            mediaId="transcriptFullMedia"
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
            class="btn-secondary btn-sm mobile-speakers-btn"
            aria-label="Спикеры встречи"
            onclick={() => (speakersDrawerOpen = true)}
          >
            Спикеры{#if transcript.speakers.length > 0}&nbsp;· {transcript.speakers.length}{/if}
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

          {#if transcript.extractVisible}
            <button class="btn-secondary btn-sm desktop-only-btn" onclick={() => void extractMp3(ui.currentTranscriptPath)}>
              Извлечь MP3
            </button>
          {/if}

          <button class="btn-secondary btn-sm" onclick={() => void copyAll()} title="Скопировать весь текст расшифровки">
            Копировать текст
          </button>
        </div>
      </div>
    </div>
  </header>

  {#if engine.running}
    <div class="transcript-progress">
      <div class="help-text">
        {#if otherRunning}
          Движок расшифровывает другую запись – «{baseName(engine.file)}». {engine.msg}
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
    <div class="transcript-grid">
      {#if speakersDrawerOpen}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="speakers-drawer-overlay"
          role="presentation"
          onclick={() => (speakersDrawerOpen = false)}
        ></div>
      {/if}

      <div class="transcript-speakers-wrap {speakersDrawerOpen ? 'drawer-open' : ''}">
        <div class="speakers-drawer-top">
          <span class="card-title">Спикеры · {transcript.speakers.length}</span>
          <button
            class="btn-close-page"
            aria-label="Закрыть панель спикеров"
            onclick={() => (speakersDrawerOpen = false)}
          >
            ✕
          </button>
        </div>
        <SpeakersPane speakers={transcript.speakers} />
      </div>

      <TranscriptDoc
        utterances={transcript.utterances}
        markdown={transcript.markdown}
        onseek={seekTo}
      />
    </div>
  {/if}
</div>
