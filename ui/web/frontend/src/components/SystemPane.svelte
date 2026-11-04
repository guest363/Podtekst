<script lang="ts">
  import { onMount } from 'svelte'
  import { fmtClock, isMobile } from '../lib/api'
  import { statusBadge } from '../lib/settings.svelte.ts'
  import { emit, ui } from '../lib/ui.svelte.ts'
  import { openUpdateModal } from '../lib/update.svelte.ts'
  import {
    ackCrash,
    collectDiagnostics,
    crashHasBacktrace,
    crashMessage,
    crashUnacknowledged,
    crashVersion,
    fmtBytes,
    fmtDateTime,
    loadCrashes,
    loadJournal,
    loadSystem,
    openTarget,
    runChecks,
    system,
  } from '../lib/system.svelte.ts'
  import type {
    CheckFix,
    DevicesInfo,
    JournalSession,
    SystemCheck,
  } from '../lib/system.svelte.ts'

  type SystemTab = 'overview' | 'audio' | 'models' | 'storage' | 'logs'

  let currentTab = $state<SystemTab>('overview')
  let probeAudio = $state(true)
  let journalExpanded = $state(false)

  const snap = $derived(system.snapshot)
  const lastCrash = $derived(system.crashes.find(crashUnacknowledged) ?? null)
  const problemChecks = $derived(
    system.checks.filter((check) => check.status === 'warn' || check.status === 'error'),
  )
  const visibleJournal = $derived(
    journalExpanded ? system.journal : system.journal.slice(0, 5),
  )
  const pathRows = $derived(Object.entries(snap?.paths ?? {}))
  const unacknowledged = $derived(
    system.unacknowledged || snap?.crashes?.count || 0,
  )
  const journalTotal = $derived(system.journalTotal || snap?.journal?.count || 0)
  const coreReady = $derived(snap?.models?.core_ready === true)
  const missingModels = $derived(snap?.models?.missing_core ?? [])
  const missingModelsText = $derived(
    missingModels.length > 2
      ? `${missingModels.slice(0, 2).join(', ')} и ещё ${missingModels.length - 2}`
      : missingModels.join(', '),
  )
  const allInstalled = $derived(
    Boolean(
      snap?.models?.items?.length &&
      snap.models.items.every((m) => m.is_installed),
    ),
  )
  const modelsBadgeKind = $derived(
    allInstalled ? 'ok' : coreReady ? 'ok' : 'warn',
  )
  const modelsBadgeLabel = $derived(
    allInstalled ? 'Все готовы' : coreReady ? 'Базовые готовы' : 'Не всё',
  )

  function getModel(id: string) {
    return snap?.models?.items?.find((m) => m.id === id)
  }

  function modelStatusText(id: string, isOptional = false): { label: string; ok: boolean } {
    const item = getModel(id)
    if (item?.is_installed) return { label: 'установлена', ok: true }
    if (isOptional) return { label: 'не установлена', ok: false }
    return { label: 'не установлена', ok: false }
  }
  const ffmpegReady = $derived(snap?.ffmpeg?.available === true)
  const systemAudio = $derived(snap?.devices?.has_system_audio === true)
  const screenReady = $derived(permission('screen_capture_supported') !== false)
  const micName = $derived(defaultName(snap?.devices, snap?.devices?.default_input))
  const freeBytes = $derived(snap?.disk?.free_bytes ?? 0)
  const totalBytes = $derived(snap?.disk?.total_bytes ?? 0)
  const usedBytes = $derived(totalBytes > freeBytes ? totalBytes - freeBytes : 0)
  const usedPct = $derived(totalBytes > 0 ? Math.min(100, Math.round((usedBytes / totalBytes) * 100)) : 0)
  const freeHours = $derived(freeBytes > 0 ? Math.floor(freeBytes / (450 * 1024 * 1024)) : 0)
  const systemAudioText = $derived(
    systemAudio
      ? 'доступен'
      : snap?.os && (typeof snap.os === 'object' ? snap.os.name : snap.os) === 'macos'
        ? 'нужен BlackHole'
        : 'не найден',
  )
  const overallKind = $derived(
    unacknowledged > 0 || system.summary?.status === 'error'
      ? 'error'
      : system.summary?.status === 'warn' || (snap && !coreReady)
        ? 'warn'
        : system.summary?.status === 'ok' || snap
          ? 'ok'
          : 'info',
  )
  const overallLabel = $derived(
    overallKind === 'error'
      ? 'Есть проблема'
      : overallKind === 'warn'
        ? 'Есть замечания'
        : overallKind === 'ok'
          ? 'Всё в порядке'
          : 'Проверки не запускались',
  )

  const PATH_LABELS: Record<string, string> = {
    data: 'Данные',
    recordings: 'Записи',
    logs: 'Логи',
    models: 'Модели',
    crashes: 'Сбои',
    diagnostics: 'Диагностика',
  }

  const MODE_LABELS: Record<string, string> = {
    split: '2 канала',
    mix: '1 канал',
  }

  onMount(() => {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
  })

  function close(): void {
    ui.systemOpen = false
  }

  function onWindowKey(event: KeyboardEvent): void {
    if (ui.systemOpen && event.key === 'Escape') close()
  }

  function refresh(): void {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
  }

  function permission(key: string): boolean | string | undefined {
    const permissions = snap?.permissions
    if (!permissions) return undefined
    if (Array.isArray(permissions)) {
      return permissions.find((item) => item.id === key)?.granted
    }
    return permissions[key]
  }

  function defaultName(devices: DevicesInfo | undefined, id?: string | null): string {
    if (!id) return ''
    const all = [...(devices?.inputs || []), ...(devices?.system || [])]
    return all.find((device) => device.id === id)?.name || id
  }

  function osText(): string {
    const os = snap?.os
    if (!os) return ''
    if (typeof os === 'string') return os
    let name = os.name || os.family || ''
    if (name === 'macos') name = 'macOS'
    else if (name === 'windows') name = 'Windows'
    else if (name === 'linux') name = 'Linux'
    return [name, os.arch].filter(Boolean).join(' · ')
  }

  function diskText(): string {
    if (!freeBytes) return snap?.disk?.path || '—'
    const free = fmtBytes(freeBytes)
    const hours = freeHours > 0 ? ` (~${freeHours} ч)` : ''
    if (totalBytes > 0) {
      return `свободно ${free} из ${fmtBytes(totalBytes)}${hours}`
    }
    return `свободно ${free}${hours}`
  }

  function memoryText(): string {
    const available = snap?.memory?.available_bytes
    return available ? `доступно ${fmtBytes(available)}` : '—'
  }

  function checkBadge(status: string): string {
    if (status === 'ok') return statusBadge('ok', 'Норма')
    if (status === 'warn') return statusBadge('warn', 'Замечание')
    if (status === 'error') return statusBadge('error', 'Ошибка')
    return statusBadge('info', 'Пропущено')
  }

  function summaryText(): string {
    const summary = system.summary
    if (!summary) return ''
    if (summary.warn === 0 && summary.error === 0) {
      return `Проверок: ${system.checks.length} · всё в порядке`
    }
    const parts = [
      `норма ${summary.ok}`,
      `замечания ${summary.warn}`,
      `ошибки ${summary.error}`,
    ]
    if (summary.skip) parts.push(`пропущено ${summary.skip}`)
    return parts.join(' · ')
  }

  function journalFile(session: JournalSession): string {
    if (!session.file) return session.id
    if (typeof session.file === 'string') return session.file.split(/[\\/]/).pop() || session.file
    return session.file.name || session.file.id || session.id
  }

  function journalMeta(session: JournalSession): string {
    const parts: string[] = []
    if (session.started_at) parts.push(fmtDateTime(session.started_at))
    if (session.duration_sec) parts.push(fmtClock(session.duration_sec))
    if (session.mode) parts.push(MODE_LABELS[session.mode] || session.mode)
    if (session.silent) parts.push('тихая запись')
    if (session.errors?.length) parts.push(`ошибок: ${session.errors.length}`)
    return parts.join(' · ')
  }

  function journalBadge(session: JournalSession): string {
    if (session.status === 'recording') return statusBadge('info', 'Идёт')
    if (session.status === 'ok' || session.status === 'completed' || session.status === 'done') {
      return statusBadge('ok', 'Готова')
    }
    if (session.status === 'interrupted' || session.status === 'aborted') {
      return statusBadge('warn', 'Прервана')
    }
    if (session.status === 'error') return statusBadge('error', 'Ошибка')
    return statusBadge('info', session.status || 'Без статуса')
  }

  function openSettings(tab: 'record' | 'people' | 'voices' | 'dict' | 'models'): void {
    close()
    emit('open-settings', tab)
  }

  function openUpdates(): void {
    close()
    openUpdateModal()
  }

  function handleFix(fix: CheckFix | null): void {
    if (!fix) return
    if (fix.kind === 'open_folder' || fix.kind === 'open_settings') {
      void openTarget(fix.target)
    } else if (fix.kind === 'download_model') {
      openSettings('models')
    } else if (fix.kind === 'download_ffmpeg') {
      openSettings('models')
    } else if (fix.kind === 'open_update') {
      openUpdates()
    } else if (fix.kind === 'collect_diagnostics') {
      void collectDiagnostics()
    }
  }

  function crashKind(): string {
    if (!lastCrash) return ''
    return lastCrash.synthetic ? 'Завершение без корректного выхода' : 'Сбой'
  }

  function crashWord(count: number): string {
    const tail = count % 100
    if (tail >= 11 && tail <= 14) return 'сбоев'
    const digit = count % 10
    if (digit === 1) return 'сбой'
    if (digit >= 2 && digit <= 4) return 'сбоя'
    return 'сбоев'
  }
</script>

<svelte:window onkeydown={onWindowKey} />

{#snippet checkRow(check: SystemCheck)}
  <div class="rec-item">
    <div class="rec-info">
      <span class="rec-name">{check.title}</span>
      {#if check.detail}<span class="rec-meta">{check.detail}</span>{/if}
      {#if check.hint}<span class="rec-meta check-hint">{check.hint}</span>{/if}
    </div>
    <div class="rec-actions">
      {@html checkBadge(check.status)}
      {#if check.fix}
        <button class="btn-secondary btn-sm" onclick={() => handleFix(check.fix)}>
          {check.fix.label || 'Исправить'}
        </button>
      {/if}
    </div>
  </div>
{/snippet}

{#if ui.systemOpen}
  <div
    class="modal-overlay open system-overlay"
    role="presentation"
    onmousedown={(event) => {
      if (event.target === event.currentTarget) close()
    }}
  >
    <div
      class="modal-box system-box md-layout"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="Система"
    >
      <aside class="md-sidebar">
        <div class="md-sidebar-head">
          <div class="md-sidebar-title-row">
            <div class="modal-title">Система</div>
            <div class="system-sub">
              {@html statusBadge(overallKind, overallLabel)}
            </div>
          </div>
          <div class="md-os-tag">{osText() || 'Окружение'}</div>
          <button class="icon-btn md-mobile-close" aria-label="Закрыть" onclick={close}>✕</button>
        </div>

        <nav class="md-nav" aria-label="Разделы диагностики">
          <button
            class="md-nav-item"
            class:active={currentTab === 'overview'}
            onclick={() => (currentTab = 'overview')}
          >
            <span class="md-nav-label">Обзор готовности</span>
            <span class="md-dot {overallKind}"></span>
          </button>

          <button
            class="md-nav-item"
            class:active={currentTab === 'audio'}
            onclick={() => (currentTab = 'audio')}
          >
            <span class="md-nav-label">Звуковой тракт</span>
            <span class="md-dot {systemAudio && micName ? 'ok' : 'warn'}"></span>
          </button>

          <button
            class="md-nav-item"
            class:active={currentTab === 'models'}
            onclick={() => (currentTab = 'models')}
          >
            <span class="md-nav-label">ИИ и модели</span>
            <span class="md-dot {coreReady && ffmpegReady ? 'ok' : 'warn'}"></span>
          </button>

          <button
            class="md-nav-item"
            class:active={currentTab === 'storage'}
            onclick={() => (currentTab = 'storage')}
          >
            <span class="md-nav-label">Диск и память</span>
            {#if freeBytes > 0}
              <span class="md-tag font-mono">{fmtBytes(freeBytes)}</span>
            {/if}
          </button>

          <button
            class="md-nav-item"
            class:active={currentTab === 'logs'}
            onclick={() => (currentTab = 'logs')}
          >
            <span class="md-nav-label">Журнал и сбои</span>
            {#if unacknowledged > 0}
              <span class="md-tag error font-mono">{unacknowledged}</span>
            {:else}
              <span class="md-tag ok font-mono">0</span>
            {/if}
          </button>
        </nav>

        <nav class="md-mobile-nav" aria-label="Разделы системы">
          <div class="md-mobile-nav-row">
            <button
              type="button"
              class="md-mobile-tab"
              class:active={currentTab === 'overview'}
              onclick={() => (currentTab = 'overview')}
            >
              <span>Обзор</span>
              <span class="md-dot {overallKind}"></span>
            </button>
            <button
              type="button"
              class="md-mobile-tab"
              class:active={currentTab === 'audio'}
              onclick={() => (currentTab = 'audio')}
            >
              <span>Звук</span>
              <span class="md-dot {systemAudio && micName ? 'ok' : 'warn'}"></span>
            </button>
            <button
              type="button"
              class="md-mobile-tab"
              class:active={currentTab === 'models'}
              onclick={() => (currentTab = 'models')}
            >
              <span>Модели</span>
              <span class="md-dot {coreReady && ffmpegReady ? 'ok' : 'warn'}"></span>
            </button>
          </div>
          <div class="md-mobile-nav-row">
            <button
              type="button"
              class="md-mobile-tab"
              class:active={currentTab === 'storage'}
              onclick={() => (currentTab = 'storage')}
            >
              <span>Диск и память</span>
              {#if freeBytes > 0}
                <span class="md-tag font-mono">{fmtBytes(freeBytes).split(' ')[0]} ГБ</span>
              {/if}
            </button>
            <button
              type="button"
              class="md-mobile-tab"
              class:active={currentTab === 'logs'}
              onclick={() => (currentTab = 'logs')}
            >
              <span>Журнал и сбои</span>
              {#if unacknowledged > 0}
                <span class="md-tag error font-mono">{unacknowledged}</span>
              {:else}
                <span class="md-tag ok font-mono">0</span>
              {/if}
            </button>
          </div>
        </nav>

        <div class="md-sidebar-foot">
          {#if !isMobile}
            <button class="btn-secondary btn-sm md-diag-btn" onclick={() => void collectDiagnostics()}>
              Собрать отчёт
            </button>
          {/if}
        </div>
      </aside>

      <main class="md-content">
        <header class="md-content-head">
          <div>
            <h3 class="md-content-title">
              {#if currentTab === 'overview'}Обзор готовности
              {:else if currentTab === 'audio'}Звуковой тракт
              {:else if currentTab === 'models'}ИИ и модели
              {:else if currentTab === 'storage'}Диск и память
              {:else if currentTab === 'logs'}Журнал и сбои
              {/if}
            </h3>
            <p class="help-text">
              {#if currentTab === 'overview'}
                {#if system.checkedAt}Последняя проверка {fmtDateTime(system.checkedAt)}{:else}Ключевые узлы перед началом записи{/if}
              {:else if currentTab === 'audio'}Конфигурация микрофона и системного звука
              {:else if currentTab === 'models'}Локальные нейросети для расшифровки
              {:else if currentTab === 'storage'}Каталоги и емкость дискового хранилища
              {:else if currentTab === 'logs'}История сессий и аварийных завершений
              {/if}
            </p>
          </div>
          <button class="icon-btn md-desktop-close" aria-label="Закрыть состояние системы" onclick={close}>✕</button>
        </header>

        <div class="md-content-body">
          {#if currentTab === 'overview'}
            <div class="system-hero">
              <div class="system-hero-actions">
                <button
                  class="btn-primary"
                  disabled={system.checksLoading}
                  onclick={() => void runChecks(probeAudio)}
                >
                  {system.checksLoading ? 'Проверяю…' : 'Проверить систему'}
                </button>
                <button class="btn-secondary btn-sm" disabled={system.loading} onclick={refresh}>
                  {system.loading ? '…' : 'Обновить'}
                </button>
              </div>

              <label class="setting-check system-probe">
                <input type="checkbox" bind:checked={probeAudio} />
                <span class="setting-switch" aria-hidden="true"></span>
                <span>Проверять микрофон короткой записью</span>
              </label>
            </div>

            <div class="system-grid">
              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Запись</span>
                  {@html snap?.recording?.active
                    ? statusBadge('info', 'Идёт')
                    : statusBadge(systemAudio ? 'ok' : 'warn', systemAudio ? 'Готова' : 'Без системы')}
                </div>
                <div class="system-facts">
                  <span class="system-key">Микрофон</span>
                  <span class="system-val">{micName || 'не выбран'}</span>
                  <span class="system-key">Системный звук</span>
                  <span class="system-val">{systemAudioText}</span>
                </div>
              </section>

              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Компоненты</span>
                  {@html coreReady && ffmpegReady
                    ? statusBadge('ok', 'Готовы')
                    : statusBadge('warn', 'Не всё')}
                </div>
                <div class="system-facts">
                  <span class="system-key">Модели</span>
                  <span class="system-val">{coreReady ? 'базовые установлены' : (missingModelsText || 'не всё')}</span>
                  <span class="system-key">ffmpeg</span>
                  <span class="system-val">{ffmpegReady ? 'установлен' : 'не установлен'}</span>
                </div>
              </section>

              <section class="system-card system-card-wide">
                <div class="system-card-head">
                  <span class="card-title">Дисковое пространство</span>
                  <span class="disk-meta">{diskText()}</span>
                </div>
                {#if totalBytes > 0}
                  <div class="disk-gauge-track" aria-hidden="true">
                    <div class="disk-gauge-fill" style="width: {usedPct}%"></div>
                  </div>
                {/if}
                <div class="system-card-footer">
                  <span class="system-path" title={snap?.disk?.path || snap?.paths?.recordings}>
                    {snap?.paths?.recordings || snap?.disk?.path || '—'}
                  </span>
                  {#if !isMobile}
                    <button class="btn-secondary btn-sm" onclick={() => void openTarget('recordings')}>
                      Открыть папку записей
                    </button>
                  {/if}
                </div>
              </section>
            </div>

            {#if problemChecks.length}
              <div class="problem-alert">
                <div class="problem-alert-title">Есть замечания проверок ({problemChecks.length})</div>
                <div class="recordings-list">
                  {#each problemChecks as check (check.id)}
                    {@render checkRow(check)}
                  {/each}
                </div>
              </div>
            {/if}

          {:else if currentTab === 'audio'}
            <div class="tab-pane-content">
              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Микрофон</span>
                  {@html statusBadge(micName ? 'ok' : 'warn', micName ? 'Подключен' : 'Не выбран')}
                </div>
                <div class="system-facts">
                  <span class="system-key">Устройство</span>
                  <span class="system-val">{micName || 'не выбрано'}</span>
                  <span class="system-key">Платформа</span>
                  <span class="system-val">{snap?.devices?.platform || 'Нативная'}</span>
                </div>
              </section>

              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Системный звук (собеседники)</span>
                  {@html statusBadge(systemAudio ? 'ok' : 'warn', systemAudio ? 'Доступен' : 'Отсутствует')}
                </div>
                <div class="system-facts">
                  <span class="system-key">Захват звука</span>
                  <span class="system-val">{systemAudioText}</span>
                  <span class="system-key">Запись экрана</span>
                  <span class="system-val">{screenReady ? 'доступна' : 'нет разрешения'}</span>
                </div>
              </section>

              <div class="row-start pt-2">
                <button class="btn-primary btn-sm" disabled={system.checksLoading} onclick={() => void runChecks(true)}>
                  Проверить микрофон
                </button>
                <button class="btn-secondary btn-sm" onclick={() => openSettings('record')}>
                  Настройки аудио ↗
                </button>
              </div>
            </div>

          {:else if currentTab === 'models'}
            {#snippet modelRow(key: string, name: string, id: string, optional = false)}
              {@const st = modelStatusText(id, optional)}
              {@const isInstalled = getModel(id)?.is_installed}
              {@const kind = isInstalled ? 'ok' : (optional ? 'optional' : 'missing')}
              <span class="system-key">{key}</span>
              <span class="system-val model-val">
                <span class="model-name">{name}</span>
                <span class="model-status-indicator {kind}" title="{name}: {st.label}">
                  <span class="model-status-dot {kind}"></span>
                  <span class="model-status-text">{st.label}</span>
                </span>
              </span>
            {/snippet}

            <div class="tab-pane-content">
              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Локальные нейросети</span>
                  {@html statusBadge(modelsBadgeKind, modelsBadgeLabel)}
                </div>
                <div class="system-facts">
                  {@render modelRow('Русский (SOTA)', 'GigaAM-v3 RNNT int8', 'asr-gigaam-v3-rnnt-int8')}
                  {@render modelRow('Мультиязычный', 'Whisper large-v3-turbo', 'asr-whisper-large-v3-turbo-q', true)}
                  {@render modelRow('Детекция речи', 'Silero VAD v6', 'vad-silero-v6')}
                  {@render modelRow('Диаризация', 'PyAnnote Segmentation 3.0', 'diar-pyannote-segmentation-3')}
                  {@render modelRow('Эмбеддинги', 'ERes2NetV2 192-dim', 'diar-eres2netv2-emb')}
                  {#if getModel('asr-gigaam-v3-e2e-ctc-int8')}
                    {@render modelRow('Словарь терминов', 'GigaAM-v3 CTC int8', 'asr-gigaam-v3-e2e-ctc-int8', true)}
                  {/if}
                </div>
              </section>

              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Медиа-утилиты</span>
                  {@html statusBadge(ffmpegReady ? 'ok' : 'warn', ffmpegReady ? 'Готов' : 'Не установлен')}
                </div>
                <div class="system-facts">
                  <span class="system-key">FFmpeg</span>
                  <span class="system-val">{ffmpegReady ? 'установлен и доступен' : 'требуется для импорта медиа'}</span>
                </div>
              </section>

              <div class="row-start pt-2">
                <button class="btn-secondary btn-sm" onclick={() => openSettings('models')}>
                  Управление моделями и весами ↗
                </button>
                {#if !isMobile}
                  <button class="btn-secondary btn-sm" onclick={() => void openTarget('models')}>
                    Открыть папку моделей
                  </button>
                {/if}
              </div>
            </div>

          {:else if currentTab === 'storage'}
            <div class="tab-pane-content">
              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Емкость накопителя</span>
                  <span class="disk-meta">{diskText()}</span>
                </div>
                {#if totalBytes > 0}
                  <div class="disk-gauge-track" aria-hidden="true">
                    <div class="disk-gauge-fill" style="width: {usedPct}%"></div>
                  </div>
                {/if}
              </section>

              <section class="system-card">
                <div class="system-card-head">
                  <span class="card-title">Каталоги файловой системы</span>
                </div>
                <div class="system-paths-block">
                  <div class="system-path-row">
                    <span class="system-key">Записи</span>
                    <span class="system-val system-path" title={snap?.paths?.recordings}>{snap?.paths?.recordings || '—'}</span>
                    {#if !isMobile}
                      <button class="btn-secondary btn-xs" onclick={() => void openTarget('recordings')}>Открыть</button>
                    {/if}
                  </div>
                  <div class="system-path-row">
                    <span class="system-key">Данные</span>
                    <span class="system-val system-path" title={snap?.paths?.data}>{snap?.paths?.data || '—'}</span>
                    {#if !isMobile}
                      <button class="btn-secondary btn-xs" onclick={() => void openTarget('data')}>Открыть</button>
                    {/if}
                  </div>
                  {#if snap?.log?.path}
                    <div class="system-path-row">
                      <span class="system-key">Лог</span>
                      <span class="system-val system-path" title={snap.log.path}>{snap.log.path}</span>
                      {#if !isMobile}
                        <button class="btn-secondary btn-xs" onclick={() => void openTarget('logs')}>Открыть</button>
                      {/if}
                    </div>
                  {/if}
                </div>

                {#if !isMobile}
                  <div class="system-facts pt-2">
                    <span class="system-key">Папки</span>
                    <div class="path-chips">
                      <button class="chip-btn" onclick={() => void openTarget('logs')}>Логи</button>
                      <button class="chip-btn" onclick={() => void openTarget('models')}>Модели</button>
                      <button class="chip-btn" onclick={() => void openTarget('crashes')}>Сбои</button>
                      <button class="chip-btn" onclick={() => void openTarget('diagnostics')}>Диагностика</button>
                    </div>
                  </div>
                {/if}
              </section>

              <section class="system-card">
                <div class="system-facts">
                  <span class="system-key">Память</span>
                  <span class="system-val">{memoryText()}</span>
                </div>
              </section>
            </div>

          {:else if currentTab === 'logs'}
            <div class="tab-pane-content">
              {#if lastCrash}
                <section class="system-card">
                  <div class="system-card-head">
                    <span class="card-title">Последний сбой</span>
                    <span class="help-text">{crashKind()}</span>
                  </div>
                  <div class="rec-item">
                    <div class="rec-info">
                      <span class="rec-name">{fmtDateTime(lastCrash.created_at) || 'без даты'}</span>
                      <span class="rec-meta">{crashMessage(lastCrash) || 'Сообщение не записано.'}</span>
                      <span class="rec-meta">
                        {#if crashVersion(lastCrash)}Версия {crashVersion(lastCrash)}{/if}
                        {#if crashHasBacktrace(lastCrash)} · есть стек вызовов{/if}
                      </span>
                    </div>
                    <div class="rec-actions">
                      <button class="btn-secondary btn-sm" onclick={() => void ackCrash(lastCrash.id)}>
                        Скрыть
                      </button>
                    </div>
                  </div>
                </section>
              {:else}
                <section class="system-card">
                  <div class="system-card-head">
                    <span class="card-title">Состояние системы</span>
                    {@html statusBadge('ok', 'Сбоев нет')}
                  </div>
                  <div class="system-facts">
                    <span class="system-key">Сбои</span>
                    <span class="system-val">Аварийных завершений не зафиксировано</span>
                    {#if snap?.paths?.logs}
                      <span class="system-key">Папка логов</span>
                      <span class="system-val system-path">{snap.paths.logs}</span>
                    {/if}
                  </div>
                </section>
              {/if}

              {#if journalTotal > 0}
                <section class="system-section">
                  <div class="system-section-head">
                    <span class="card-title">Журнал записей ({journalTotal})</span>
                    {#if system.journalTotal > 5}
                      <button class="link-btn" onclick={() => (journalExpanded = !journalExpanded)}>
                        {journalExpanded ? 'Свернуть' : `Показать все (${system.journalTotal})`}
                      </button>
                    {/if}
                  </div>
                  <div class="recordings-list">
                    {#each visibleJournal as session (session.id)}
                      <div class="rec-item">
                        <div class="rec-info">
                          <span class="rec-name">{journalFile(session)}</span>
                          <span class="rec-meta">{journalMeta(session)}</span>
                        </div>
                        <div class="rec-actions">{@html journalBadge(session)}</div>
                      </div>
                    {/each}
                  </div>
                </section>
              {/if}

              <div class="row-start pt-2">
                {#if isMobile}
                  <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
                    Собрать отчёт
                  </button>
                {/if}
                {#if !isMobile}
                  <button class="btn-secondary btn-sm" onclick={() => void openTarget('logs')}>
                    Открыть папку логов
                  </button>
                {/if}
              </div>
            </div>
          {/if}
        </div>
      </main>
    </div>
  </div>
{/if}

<style>
  .system-overlay {
    z-index: 70;
    padding: 0;
    align-items: stretch;
    justify-content: stretch;
  }
  .system-box.md-layout {
    width: 100vw;
    height: 100dvh;
    max-width: 100vw;
    max-height: 100dvh;
    padding: 0;
    gap: 0;
    border-radius: 0;
    border: none;
    box-shadow: none;
    margin: 0;
    overflow: hidden;
    display: flex;
    flex-direction: row;
    animation: fadeIn 160ms var(--ease-spring);
  }
  .md-sidebar {
    width: 220px;
    flex-shrink: 0;
    background: var(--bg-inset);
    border-right: 1px solid var(--border-soft);
    display: flex;
    flex-direction: column;
    padding: 20px 14px 16px;
    box-sizing: border-box;
  }
  .md-sidebar-head {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--border-soft);
  }
  .md-sidebar-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .md-mobile-close {
    display: none;
  }
  .md-mobile-nav {
    display: none;
  }
  .md-os-tag {
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--text-faint);
  }
  .md-nav {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 12px;
  }
  .md-nav-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border-radius: var(--radius-inner);
    font-size: 13px;
    font-weight: 500;
    color: var(--text-muted);
    background: transparent;
    border: 1px solid transparent;
    cursor: pointer;
    text-align: left;
    transition: all 0.15s var(--ease-spring);
  }
  .md-nav-item:hover {
    background: rgba(15, 30, 51, 0.04);
    color: var(--text-primary);
  }
  .md-nav-item.active {
    background: #fff;
    color: var(--text-primary);
    font-weight: 600;
    border-color: var(--border-card);
    box-shadow: 0 1px 3px rgba(15, 30, 51, 0.06);
  }
  .md-nav-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .md-dot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .md-dot.ok { background: var(--accent-ok); }
  .md-dot.warn { background: var(--accent-amber); }
  .md-dot.error { background: var(--accent-rec); }
  .md-dot.info { background: var(--accent-blue); }

  .md-tag {
    font-size: 10.5px;
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    background: rgba(15, 30, 51, 0.05);
    color: var(--text-muted);
  }
  .md-tag.ok { color: var(--accent-ok); background: var(--accent-ok-bg); }
  .md-tag.error { color: var(--accent-rec); background: #fde8e8; font-weight: 700; }

  .md-sidebar-foot {
    margin-top: auto;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
  }
  .md-diag-btn {
    width: 100%;
    justify-content: center;
  }

  .md-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    background: #fff;
    height: 100%;
  }
  .md-content-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding: 20px 24px 14px;
    border-bottom: 1px solid var(--border-soft);
    flex-shrink: 0;
  }
  .md-content-title {
    font-size: 15px;
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }
  .md-content-body {
    flex: 1;
    overflow-y: auto;
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .tab-pane-content {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .tab-pane-content,
  .system-hero,
  .system-grid,
  .system-section,
  .problem-alert {
    max-width: 760px;
    width: 100%;
  }

  .problem-alert {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-radius: var(--radius-inner);
    background: var(--accent-amber-bg);
    border: 1px solid rgba(138, 90, 0, 0.15);
  }
  .problem-alert-title {
    font-size: 12.5px;
    font-weight: 700;
    color: var(--accent-amber);
  }

  .model-val {
    display: inline-flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }
  .model-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .model-status-indicator {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    padding: 1px 7px;
    border-radius: var(--radius-sm);
    font-weight: 500;
    flex-shrink: 0;
  }
  .model-status-indicator.ok {
    background: var(--accent-ok-bg);
    color: var(--accent-ok);
  }
  .model-status-indicator.missing {
    background: #fde8e8;
    color: var(--accent-rec);
  }
  .model-status-indicator.optional {
    background: var(--bg-inset);
    color: var(--text-faint);
  }
  .model-status-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    flex-shrink: 0;
  }
  .model-status-dot.ok {
    background: var(--accent-ok);
  }
  .model-status-dot.missing {
    background: var(--accent-rec);
  }
  .model-status-dot.optional {
    background: #94a3b8;
  }

  .system-sub {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    color: var(--text-muted);
    font-size: 12px;
  }
  .system-hero {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }
  .system-hero-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .system-probe {
    margin-top: -6px;
    margin-bottom: 2px;
  }
  .system-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px;
  }
  .system-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    min-width: 0;
  }
  .system-card-wide {
    grid-column: 1 / -1;
  }
  .disk-meta {
    font-size: 12px;
    color: var(--text-muted);
    font-weight: 500;
    font-family: var(--font-mono);
  }
  .disk-gauge-track {
    height: 7px;
    background: var(--border-card);
    border-radius: var(--radius-pill);
    overflow: hidden;
    display: flex;
    width: 100%;
    margin-top: 2px;
  }
  .disk-gauge-fill {
    background: var(--text-primary);
    border-radius: var(--radius-pill);
    transition: width 0.3s var(--ease-spring);
  }
  .system-card-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    margin-top: 2px;
    flex-wrap: wrap;
  }
  .system-card-head,
  .system-section-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .system-facts {
    display: grid;
    grid-template-columns: minmax(110px, max-content) minmax(0, 1fr);
    gap: 4px 12px;
    font-size: 12.5px;
    word-break: normal;
    overflow-wrap: break-word;
  }
  .system-paths-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-bottom: 4px;
  }
  .system-path-row {
    display: grid;
    grid-template-columns: 80px minmax(0, 1fr) auto;
    gap: 8px;
    align-items: center;
    font-size: 12px;
  }
  .btn-xs {
    padding: 2px 8px;
    font-size: 11px;
    height: 24px;
    border-radius: var(--radius-sm);
  }
  .path-chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .chip-btn {
    font-size: 11.5px;
    padding: 2px 9px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-sm);
    background: #fff;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .chip-btn:hover {
    background: var(--bg-inset);
    color: var(--text-primary);
    border-color: var(--text-muted);
  }
  .pt-2 {
    padding-top: 8px;
  }
  .system-key {
    color: var(--text-muted);
  }
  .system-val {
    color: var(--text-primary);
    word-break: break-word;
  }
  .system-path {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .system-section {
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding-top: 2px;
  }
  .check-hint {
    color: var(--text-faint);
  }

  @media (max-width: 900px) {
    .md-sidebar {
      width: 190px;
      min-width: 190px;
      padding: 16px 10px 14px;
    }
    .system-grid {
      grid-template-columns: 1fr;
    }
  }

  @media (max-width: 640px) {
    .system-box.md-layout {
      flex-direction: column;
      height: 100dvh;
      max-height: 100dvh;
    }
    .md-sidebar {
      width: 100%;
      min-width: 0;
      border-right: none;
      border-bottom: 1px solid var(--border-soft);
      padding: 12px 14px 10px;
      flex-shrink: 0;
    }
    .md-sidebar-head {
      display: flex;
      flex-direction: row;
      align-items: center;
      justify-content: space-between;
      width: 100%;
      padding-bottom: 6px;
      border-bottom: none;
    }
    .md-sidebar-title-row {
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .md-os-tag {
      display: none;
    }
    .md-mobile-close {
      display: inline-flex;
      margin-left: auto;
    }
    .md-desktop-close {
      display: none;
    }
    .md-nav {
      display: none;
    }
    .md-mobile-nav {
      display: flex;
      flex-direction: column;
      gap: 4px;
      width: 100%;
      margin-top: 6px;
    }
    .md-mobile-nav-row {
      display: grid;
      gap: 4px;
      width: 100%;
    }
    .md-mobile-nav-row:first-child {
      grid-template-columns: repeat(3, 1fr);
    }
    .md-mobile-nav-row:last-child {
      grid-template-columns: repeat(2, 1fr);
    }
    .md-mobile-tab {
      display: flex;
      align-items: center;
      justify-content: center;
      gap: 5px;
      padding: 6px 4px;
      border-radius: var(--radius-inner);
      font-size: 11.5px;
      font-weight: 500;
      color: var(--text-muted);
      background: rgba(15, 30, 51, 0.03);
      border: 1px solid transparent;
      cursor: pointer;
      transition: all 0.15s var(--ease-spring);
      text-align: center;
      white-space: nowrap;
    }
    .md-mobile-tab:hover {
      background: rgba(15, 30, 51, 0.06);
      color: var(--text-primary);
    }
    .md-mobile-tab.active {
      background: #fff;
      color: var(--text-primary);
      font-weight: 600;
      border-color: var(--border-card);
      box-shadow: 0 1px 2px rgba(15, 30, 51, 0.05);
    }
    .model-status-text {
      display: none;
    }
    .model-status-indicator {
      padding: 0;
      background: transparent !important;
    }
    .model-status-dot {
      width: 8px;
      height: 8px;
    }
    .md-sidebar-foot {
      display: none;
    }
    .md-content-head {
      padding: 10px 14px 8px;
    }
    .md-content-body {
      padding: 12px 14px;
    }
    .system-grid {
      grid-template-columns: 1fr;
    }
    .system-hero-actions {
      width: 100%;
    }
    .system-hero-actions .btn-primary {
      flex: 1;
    }
    .system-path-row {
      grid-template-columns: 60px minmax(0, 1fr) auto;
    }
  }
</style>
