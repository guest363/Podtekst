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
  import { loadSupportInfo } from './lib/support.svelte.ts'
  import { system } from './lib/system.svelte.ts'
  import { checkUpdate, dismissUpdate, openUpdateModal, update } from './lib/update.svelte.ts'
  import { initRouting, on, openTranscript, selectTab, ui } from './lib/ui.svelte.ts'
  import { renderMarkdown } from './lib/markdown'
  import RecordingDeck from './components/RecordingDeck.svelte'
  import Recordings from './components/Recordings.svelte'
  import People from './components/People.svelte'
  import TranscriptPage from './components/TranscriptPage.svelte'
  import SettingsModal from './components/SettingsModal.svelte'
  import DonateModal from './components/DonateModal.svelte'
  import UpdateModal from './components/UpdateModal.svelte'
  import SystemPane from './components/SystemPane.svelte'
  import CrashBanner from './components/CrashBanner.svelte'
  import Toasts from './components/Toasts.svelte'
  import {
    displayRecTitle,
    fmtClock,
    formatRecDate,
    isMobile,
    isCoreModel,
    pluralSpeakers,
    remainingMb,
  } from './lib/api'

  let introOpen = $state(false)

  const recentRecordings = $derived(
    (isMobile ? recordings.items.filter((r) => !r.is_video) : recordings.items).slice(0, 3),
  )

  $effect(() => {
    if (ui.settingsOpen) {
      selectTab('settings')
      ui.settingsOpen = false
    }
  })

  const coreModels = $derived(settings.models.filter(isCoreModel))
  const coreInstalled = $derived(
    coreModels.length > 0 && coreModels.every((m) => m.is_installed)
  )
  const anyCoreDownloading = $derived(
    coreModels.some((m) => engine.downloading[m.id] !== undefined)
  )
  const coreLeftMb = $derived(coreModels.reduce((s, m) => s + remainingMb(m), 0))
  const statusTone = $derived(
    system.unacknowledged > 0 || system.summary?.status === 'error'
      ? 'error'
      : system.summary?.status === 'warn' || system.snapshot?.models?.core_ready === false
        ? 'warn'
        : system.snapshot
          ? 'ok'
          : 'info'
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

  function buildHelp(): string {
    const steps = isMobile
      ? [
          '1. Нажмите **«Начать запись»** или выберите готовый аудиофайл.',
          '2. После остановки расшифровка запускается автоматически. Это можно выключить: «Настройки» → «Запись».',
        ]
      : [
          '1. Нажмите **«Начать запись»** или перетащите готовый аудио- или видеофайл. Из любого приложения запись включает и выключает **Ctrl+Shift+R** (на macOS – **Cmd+Shift+R**).',
          '2. После остановки расшифровка запускается автоматически. Это можно выключить: «Настройки» → «Запись».',
        ]
    if (!coreInstalled) {
      steps.push('3. Для распознавания и разделения спикеров скачайте базовые модели ИИ ниже.')
    }
    steps.push(
      `${steps.length + 1}. В разделе **«Текст и спикеры»** прослушайте фрагменты и привяжите голос к человеку.`,
    )
    steps.push('')
    steps.push(
      isMobile
        ? 'Записи, расшифровки и голосовые отпечатки хранятся только на этом устройстве и никуда не отправляются.'
        : 'Записи, расшифровки и голосовые отпечатки хранятся только на этом компьютере и никуда не отправляются.',
    )
    return steps.join('\n')
  }

  onMount(() => {
    initRouting()
    if (typeof window !== 'undefined' && !localStorage.getItem('mr_intro_seen')) {
      introOpen = true
    }
    on('recordings-changed', () => void loadRecordings())
    on('support-changed', () => void loadSupportInfo())
    on('models-changed', () => void loadModels())
    // Привязка/удаление голоса меняет содержимое пакетов — счётчики в «Голосах»
    // должны перечитаться, а не ждать следующего запуска приложения.
    on('voiceprints-changed', () => void loadVoicePacks())
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
      checkUpdate(),
    ])
  })
</script>

<svelte:window onkeydown={onWindowKey} />

{#if ui.transcriptOpen && ui.currentTranscriptPath}
  <TranscriptPage />
{:else}
  <div class="app-shell">
    <!-- Десктопный сайдбар: строгий левый фрейм, скрыт на мобильных -->
    <aside class="shell-sidebar" aria-label="Разделы приложения">
      <div class="sidebar-brand">
        <span class="app-title-text">Подтекст</span>
        <span class="app-title-dot"></span>
      </div>

      <nav class="sidebar-nav" aria-label="Основная навигация">
        <button
          type="button"
          class="sidebar-btn"
          class:active={ui.activeTab === 'studio'}
          onclick={() => selectTab('studio')}
        >
          <svg class="nav-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
            <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
            <line x1="12" y1="19" x2="12" y2="22" />
          </svg>
          <span class="sidebar-btn-label">Запись</span>
        </button>

        <button
          type="button"
          class="sidebar-btn"
          class:active={ui.activeTab === 'recordings'}
          onclick={() => selectTab('recordings')}
        >
          <svg class="nav-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M4 6h16M4 12h16M4 18h10" />
          </svg>
          <span class="sidebar-btn-label">Записи</span>
          {#if recordings.items.length > 0}
            <span class="sidebar-count">{recordings.items.length}</span>
          {/if}
        </button>

        <button
          type="button"
          class="sidebar-btn"
          class:active={ui.activeTab === 'people'}
          onclick={() => selectTab('people')}
        >
          <svg class="nav-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <circle cx="12" cy="7" r="4" />
            <path d="M5.5 21a6.5 6.5 0 0 1 13 0" />
          </svg>
          <span class="sidebar-btn-label">Участники</span>
        </button>

        <button
          type="button"
          class="sidebar-btn"
          class:active={ui.activeTab === 'settings'}
          onclick={() => selectTab('settings')}
        >
          <svg class="nav-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M4 6h9M18 6h2M4 12h3M12 12h8M4 18h11M20 18h0" />
            <circle cx="15.5" cy="6" r="2.5" />
            <circle cx="9.5" cy="12" r="2.5" />
            <circle cx="17.5" cy="18" r="2.5" />
          </svg>
          <span class="sidebar-btn-label">Настройки</span>
        </button>
      </nav>

      <div class="sidebar-footer">
        <button
          type="button"
          class="status-pill sidebar-status"
          title="Состояние системы"
          aria-label="Открыть состояние системы"
          onclick={() => (ui.systemOpen = true)}
        >
          <span class="status-dot {statusTone}" aria-hidden="true"></span>
          <span class="status-pill-text">Состояние</span>
        </button>
        <button
          type="button"
          class="icon-btn help-btn"
          title="Как это работает"
          aria-label="Справка"
          onclick={() => (introOpen = true)}
        >
          ?
        </button>
      </div>
    </aside>

    <!-- Рабочая область приложения -->
    <div class="shell-workspace">
      <!-- Мобильная шапка: отображается только на смартфонах -->
      <header class="mobile-header">
        <div class="app-title">
          <span class="app-title-text">Подтекст</span>
          <span class="app-title-dot"></span>
        </div>
        <div class="header-right">
          <button
            type="button"
            class="status-pill mobile-status-btn"
            title="Состояние системы"
            aria-label="Открыть состояние системы"
            onclick={() => (ui.systemOpen = true)}
          >
            <span class="status-dot {statusTone}" aria-hidden="true"></span>
          </button>
          <button
            type="button"
            class="icon-btn help-btn"
            title="Как это работает"
            aria-label="Справка"
            onclick={() => (introOpen = true)}
          >
            ?
          </button>
        </div>
      </header>

      <main class="main-column">
        <CrashBanner />
        {#if update.isNewer && !update.dismissed}
          <section class="card update-card" aria-label="Доступно обновление">
            <div class="card-header">
              <span class="card-title">Доступна версия {update.latest}</span>
              <button class="icon-btn" aria-label="Скрыть предложение обновления" onclick={dismissUpdate}>
                ✕
              </button>
            </div>
            <div class="help-text">
              У вас установлена версия {update.current}.
              {#if update.applying}
                Скачиваю обновление… {update.pct}%.
              {:else if update.selfUpdate}
                Обновление скачается и установится здесь.
              {:else}
                Новую версию можно скачать со страницы релиза.
              {/if}
            </div>
            <div class="row-start">
              <button class="btn-primary" onclick={openUpdateModal}>Что нового и обновить</button>
              <button class="btn-secondary" onclick={dismissUpdate}>Позже</button>
            </div>
          </section>
        {/if}

        {#if ui.activeTab === 'studio'}
          <RecordingDeck />
          {#if recentRecordings.length > 0}
            <section class="card recent-recordings-card" aria-label="Последние записи">
              <div class="card-header">
                <span class="card-title">Последние записи</span>
                <button type="button" class="link-btn" onclick={() => selectTab('recordings')}>
                  Все записи ({recordings.items.length}) →
                </button>
              </div>
              <div class="recordings-list">
                {#each recentRecordings as rec (rec.path)}
                  {@const dt = formatRecDate(rec.date, rec.name)}
                  <div class="rec-item">
                    <div class="rec-info">
                      <span class="rec-name" title={rec.name}>{displayRecTitle(rec.name)}</span>
                      <span class="rec-meta">
                        {dt.full} · {#if rec.duration_sec}{fmtClock(rec.duration_sec)} · {/if}
                        {#if rec.speakers_count}{rec.speakers_count} {pluralSpeakers(rec.speakers_count)}{:else if rec.has_transcript}Текст готов{:else}Аудио{/if}
                      </span>
                    </div>
                    <div class="rec-actions">
                      <button type="button" class="btn-secondary btn-sm" onclick={() => openTranscript(rec.path, rec.is_video)}>
                        Текст и спикеры
                      </button>
                    </div>
                  </div>
                {/each}
              </div>
            </section>
          {/if}
        {:else if ui.activeTab === 'recordings'}
          <Recordings />
        {:else if ui.activeTab === 'people'}
          <div class="card people-page-card">
            <div class="card-header">
              <span class="card-title">Участники и голоса</span>
            </div>
            <People />
          </div>
        {:else if ui.activeTab === 'settings'}
          <SettingsModal inline={true} />
        {/if}
      </main>

      <!-- Мобильный таббар: отображается только на смартфонах -->
      <nav class="mobile-bottom-nav" aria-label="Мобильная навигация">
        <button
          type="button"
          class="mobile-tab-btn"
          class:active={ui.activeTab === 'studio'}
          onclick={() => selectTab('studio')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
            <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
            <line x1="12" y1="19" x2="12" y2="22" />
          </svg>
          <span>Запись</span>
        </button>

        <button
          type="button"
          class="mobile-tab-btn"
          class:active={ui.activeTab === 'recordings'}
          onclick={() => selectTab('recordings')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M4 6h16M4 12h16M4 18h10" />
          </svg>
          <span>Записи</span>
        </button>

        <button
          type="button"
          class="mobile-tab-btn"
          class:active={ui.activeTab === 'people'}
          onclick={() => selectTab('people')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
            <circle cx="12" cy="7" r="4" />
            <path d="M5.5 21a6.5 6.5 0 0 1 13 0" />
          </svg>
          <span>Люди</span>
        </button>

        <button
          type="button"
          class="mobile-tab-btn"
          class:active={ui.activeTab === 'settings'}
          onclick={() => selectTab('settings')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M4 6h9M18 6h2M4 12h3M12 12h8M4 18h11M20 18h0" />
            <circle cx="15.5" cy="6" r="2.5" />
            <circle cx="9.5" cy="12" r="2.5" />
            <circle cx="17.5" cy="18" r="2.5" />
          </svg>
          <span>Настройки</span>
        </button>
      </nav>
    </div>
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
        <div class="modal-title">Добро пожаловать в Подтекст</div>
        <button class="icon-btn" aria-label="Закрыть справку" onclick={closeIntro}>✕</button>
      </div>
      <div class="help-text md-body">
        {@html renderMarkdown(buildHelp())}
      </div>

      {#if !coreInstalled}
      <div class="intro-models-card">
        <div class="intro-models-head">
          <span class="intro-models-title">Локальные модели ИИ</span>
          <span class="intro-models-sub">
            Для локального распознавания русской речи и разделения спикеров скачайте базовые модели (~{coreLeftMb} МБ).
            Whisper и CTC (словарь и уверенность слов) можно установить позже в Настройках.
          </span>
        </div>

        {#if coreModels.length > 0}
          <div class="recordings-list">
            {#each coreModels as m (m.id)}
              {@const sizeMb = remainingMb(m)}
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
      {/if}

      <div class="row-end intro-actions">
        {#if coreInstalled}
          <button class="btn-primary" onclick={closeIntro}>Всё готово, начать работу</button>
        {:else if anyCoreDownloading || downloadingCore.active}
          <button class="btn-secondary" onclick={closeIntro}>Продолжить в фоне</button>
        {:else}
          <button class="btn-secondary" onclick={closeIntro}>Позже</button>
          <button class="btn-primary" onclick={() => void downloadAllCoreModels()}>
            Скачать базовые модели (~{coreLeftMb} МБ)
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
  .status-pill {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 6px 11px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-pill);
    background: #fff;
    color: var(--text-muted);
    font: inherit;
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: background 180ms var(--ease-spring), border-color 180ms var(--ease-spring);
  }
  .status-pill:hover {
    background: var(--bg-inset);
    border-color: #cbd5e1;
  }
  .status-pill:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .status-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #94a3b8;
    flex-shrink: 0;
  }
  .status-dot.ok { background: var(--accent-ok); }
  .status-dot.warn { background: var(--accent-amber); }
  .status-dot.error { background: var(--accent-rec); }

  /* ---------- App Shell Desktop ---------- */
  .app-shell {
    display: flex;
    align-items: flex-start;
    gap: 24px;
    max-width: 1120px;
    width: 100%;
    margin: 0 auto;
  }
  .shell-sidebar {
    width: 200px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
    position: sticky;
    top: 12px;
    height: calc(100vh - 24px);
    height: calc(100dvh - 24px);
    padding: 4px 0 12px;
    box-sizing: border-box;
  }
  .sidebar-brand {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 4px 14px;
    border-bottom: 1px solid var(--border-soft);
  }
  .app-title-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent-ok);
    box-shadow: 0 0 0 3px rgba(14, 122, 85, 0.16);
    flex-shrink: 0;
  }
  .sidebar-nav {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .sidebar-btn {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-radius: var(--radius-inner);
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    text-align: left;
    transition: all 140ms ease;
    width: 100%;
  }
  .sidebar-btn:hover {
    background: var(--bg-inset);
    color: var(--text-primary);
  }
  .sidebar-btn.active {
    background: #fff;
    color: var(--text-primary);
    border-color: var(--border-card);
    box-shadow: 0 1px 3px rgba(15, 30, 51, 0.05);
  }
  .sidebar-btn.active .nav-icon {
    color: var(--accent-rec);
  }
  .sidebar-count {
    margin-left: auto;
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--text-faint);
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-pill);
    padding: 0 6px;
  }
  .sidebar-footer {
    margin-top: auto;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .sidebar-status {
    flex: 1;
    min-width: 0;
  }
  .sidebar-status .status-pill-text {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .shell-workspace {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .people-page-card {
    padding: 18px 20px;
  }
  .recent-recordings-card {
    padding: 16px 20px;
  }

  /* На десктопе мобильная шапка и мобильный таббар СТРОГО скрыты */
  .mobile-header {
    display: none !important;
  }
  .mobile-bottom-nav {
    display: none !important;
  }

  @media (max-width: 640px) {
    .app-shell {
      display: flex;
      flex-direction: column;
      gap: 10px;
    }
    .shell-sidebar {
      display: none !important;
    }
    .mobile-header {
      display: flex !important;
      justify-content: space-between;
      align-items: center;
      gap: 8px;
      padding: 2px 2px;
      width: 100%;
    }
    .shell-workspace {
      padding-bottom: 74px;
    }
    .mobile-bottom-nav {
      display: flex !important;
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      height: 56px;
      z-index: 40;
      background: #ffffff;
      border-top: 1px solid var(--border-card);
      align-items: stretch;
      justify-content: space-around;
      padding-bottom: env(safe-area-inset-bottom, 0px);
      box-shadow: 0 -2px 10px rgba(15, 30, 51, 0.04);
    }
    .mobile-tab-btn {
      flex: 1;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 2px;
      background: transparent;
      border: none;
      color: var(--text-faint);
      font: inherit;
      font-size: 11px;
      font-weight: 600;
      cursor: pointer;
      transition: color 140ms ease;
    }
    .mobile-tab-btn.active {
      color: var(--accent-rec);
      font-weight: 700;
    }
    .mobile-tab-btn.active svg {
      color: var(--accent-rec);
    }
    .mobile-status-btn {
      width: 38px;
      height: 38px;
      padding: 0;
      border-radius: 50%;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      flex-shrink: 0;
    }
    .mobile-status-btn .status-dot {
      width: 9px;
      height: 9px;
    }
    .status-pill {
      width: 38px;
      height: 38px;
      padding: 0;
      border-radius: 50%;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      flex-shrink: 0;
    }
    .status-pill-text { display: none; }
    .status-dot {
      width: 9px;
      height: 9px;
    }
  }
</style>

<UpdateModal />
<SystemPane />
<DonateModal />
<Toasts />

