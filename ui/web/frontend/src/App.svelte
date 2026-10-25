<script lang="ts">
  import { onMount } from 'svelte'
  import { loadPeople } from './lib/people.svelte.ts'
  import { loadRecordings, recordings } from './lib/recordings.svelte.ts'
  import {
    downloadAllCoreModels,
    downloadingCore,
    engine,
    openTranscriptFile,
  } from './lib/transcript.svelte.ts'
  import {
    loadAutoTranscribe,
    loadModels,
    loadRecordMode,
    loadVoicePacks,
    settings,
    statusBadge,
  } from './lib/settings.svelte.ts'
  import { dismissSupport, loadSupportInfo, support } from './lib/support.svelte.ts'
  import { initRouting, on, ui } from './lib/ui.svelte.ts'
  import { renderMarkdown } from './lib/markdown'
  import RecordingDeck from './components/RecordingDeck.svelte'
  import Recordings from './components/Recordings.svelte'
  import TranscriptPage from './components/TranscriptPage.svelte'
  import SettingsModal from './components/SettingsModal.svelte'
  import DonateModal from './components/DonateModal.svelte'
  import Toasts from './components/Toasts.svelte'
  import { isMobile } from './lib/api'

  let introOpen = $state(false)

  const coreModels = $derived(
    settings.models.filter((m) => !m.id.toLowerCase().includes('whisper'))
  )
  const coreInstalled = $derived(
    coreModels.length > 0 && coreModels.every((m) => m.is_installed)
  )
  const anyCoreDownloading = $derived(
    coreModels.some((m) => engine.downloading[m.id] !== undefined)
  )

  function closeIntro(): void {
    introOpen = false
    try {
      localStorage.setItem('mr_intro_seen', '1')
    } catch {}
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (introOpen && e.key === 'Escape') closeIntro()
  }

  const helpMarkdown = isMobile
    ? `1. Нажмите **«Начать запись»** или выберите готовый аудио- или видеофайл.
2. После остановки расшифровка запускается автоматически. Это можно выключить: «Настройки» → «Запись».
3. Для распознавания и разделения спикеров скачайте базовые модели ИИ ниже.
4. В разделе **«Текст и спикеры»** прослушайте фрагменты и привяжите голос к человеку.

Записи, расшифровки и голосовые отпечатки хранятся только на этом устройстве и никуда не отправляются.`
    : `1. Нажмите **«Начать запись»** или перетащите готовый аудио- или видеофайл. Из любого приложения запись включает и выключает **Ctrl+Shift+R** (на macOS – **Cmd+Shift+R**).
2. После остановки расшифровка запускается автоматически. Это можно выключить: «Настройки» → «Запись».
3. Для распознавания и разделения спикеров скачайте базовые модели ИИ ниже.
4. В разделе **«Текст и спикеры»** прослушайте фрагменты и привяжите голос к человеку.

Записи, расшифровки и голосовые отпечатки хранятся только на этом компьютере и никуда не отправляются.`

  onMount(() => {
    initRouting()
    if (typeof window !== 'undefined' && !localStorage.getItem('mr_intro_seen')) {
      introOpen = true
    }
    on('recordings-changed', () => void loadRecordings())
    on('support-changed', () => void loadSupportInfo())
    on('models-changed', () => void loadModels())
    on('transcript-reload', () => {
      if (ui.currentTranscriptPath) void openTranscriptFile(ui.currentTranscriptPath)
    })
    void Promise.allSettled([
      loadPeople(),
      loadRecordings(),
      loadVoicePacks(),
      loadModels(),
      loadRecordMode(),
      loadAutoTranscribe(),
      loadSupportInfo(),
    ])
  })
</script>

<svelte:window onkeydown={onWindowKey} />

{#if ui.transcriptOpen && ui.currentTranscriptPath}
  <TranscriptPage />
{:else}
  <div class="page">
    <header class="app-header">
      <div class="app-title">
        <span class="dot" aria-hidden="true"></span>
        <span class="app-title-text">Запись встреч</span>
      </div>
      <div class="header-right">
        <button
          class="icon-btn help-btn"
          title="Как это работает"
          aria-label="Справка"
          onclick={() => (introOpen = true)}
        >
          ?
        </button>
        <button
          class="icon-btn"
          title="Настройки"
          aria-label="Открыть настройки"
          onclick={() => (ui.settingsOpen = true)}
        >
          <svg
            width="17"
            height="17"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M4 6h9M18 6h2M4 12h3M12 12h8M4 18h11M20 18h0" />
            <circle cx="15.5" cy="6" r="2.5" />
            <circle cx="9.5" cy="12" r="2.5" />
            <circle cx="17.5" cy="18" r="2.5" />
          </svg>
        </button>
      </div>
    </header>

    <main class="main-column">
      <RecordingDeck />
      <Recordings />

      {#if support.visible}
        <section class="card donate">
          <div class="card-header">
            <span class="card-title">{support.title}</span>
            <button class="btn-secondary" onclick={() => void dismissSupport()}>Скрыть</button>
          </div>
          <div class="donate-text">{support.statsText}</div>
          <div class="row-start">
            <button class="btn-primary" onclick={() => (ui.donateOpen = true)}>Поддержать проект</button>
          </div>
        </section>
      {/if}
    </main>

    <footer class="page-footer">
      <button class="btn-secondary btn-sm" onclick={() => (ui.donateOpen = true)}>
        Поддержать разработку
      </button>
    </footer>
  </div>
{/if}

{#if introOpen}
  <div
    class="modal-overlay open"
    role="dialog" tabindex="-1"
    aria-modal="true"
    aria-label="Как это работает"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) closeIntro()
    }}
  >
    <div class="modal-box intro-box">
      <div class="modal-top">
        <div class="modal-title">Добро пожаловать в Запись встреч</div>
        <button class="icon-btn" aria-label="Закрыть справку" onclick={closeIntro}>✕</button>
      </div>
      <div class="help-text md-body">
        {@html renderMarkdown(helpMarkdown)}
      </div>

      <div class="intro-models-card">
        <div class="intro-models-head">
          <span class="intro-models-title">Локальные модели ИИ</span>
          <span class="intro-models-sub">
            {coreInstalled
              ? 'Все базовые модели установлены и готовы к оффлайн-работе.'
              : 'Для локального распознавания русской речи скачайте базовые модели (~300 МБ). Whisper (1.1 ГБ) можно установить позже в Настройках.'}
          </span>
        </div>

        {#if coreModels.length > 0}
          <div class="recordings-list">
            {#each coreModels as m (m.id)}
              {@const sizeMb = Math.round((m.total_size_bytes || 0) / 1048576)}
              {@const pct = engine.downloading[m.id]}
              {@const isQueued = downloadingCore.queued.includes(m.id) && pct === undefined}
              {@const badge = pct !== undefined
                ? statusBadge('info', `Скачивание ${pct}%`)
                : isQueued
                  ? statusBadge('info', 'В очереди…')
                  : m.is_installed
                    ? statusBadge('ok', '✓ Готово')
                    : statusBadge('info', `${sizeMb} МБ`)}
              <div class="rec-item">
                <div class="rec-info">
                  <span class="rec-name">{m.title}</span>
                  <span class="rec-meta">{m.description || m.id}</span>
                  {#if pct !== undefined}
                    <div
                      class="vu-track progress-track"
                      role="progressbar"
                      aria-valuenow={pct}
                      aria-label={`Прогресс скачивания ${m.title}`}
                    >
                      <div class="vu-bar" style={`width: ${pct}%`}></div>
                    </div>
                  {/if}
                </div>
                <div class="rec-actions">
                  {@html badge}
                </div>
              </div>
            {/each}
          </div>
        {:else}
          <div class="empty-note">Загрузка информации о моделях…</div>
        {/if}
      </div>

      <div class="row-end intro-actions">
        {#if coreInstalled}
          <button class="btn-primary" onclick={closeIntro}>Всё готово, начать работу</button>
        {:else if anyCoreDownloading || downloadingCore.active}
          <button class="btn-secondary" onclick={closeIntro}>Продолжить в фоне</button>
        {:else}
          <button class="btn-secondary" onclick={closeIntro}>Позже</button>
          <button class="btn-primary" onclick={() => void downloadAllCoreModels()}>
            Скачать базовые модели (~300 МБ)
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .intro-models-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-sm);
    padding: 12px 14px;
    margin-top: 4px;
  }
  .intro-models-head {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .intro-models-title {
    font-size: 13.5px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .intro-models-sub {
    font-size: 11.5px;
    color: var(--text-muted);
    line-height: 1.4;
  }
  .intro-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    flex-wrap: wrap;
    margin-top: 4px;
  }
</style>

<SettingsModal />
<DonateModal />
<Toasts />

