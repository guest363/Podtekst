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
  const ffmpegReady = $derived(snap?.ffmpeg?.available === true)
  const systemAudio = $derived(snap?.devices?.has_system_audio === true)
  const screenReady = $derived(permission('screen_capture_supported') !== false)
  const micName = $derived(defaultName(snap?.devices, snap?.devices?.default_input))
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
    return [os.name || os.family, os.arch].filter(Boolean).join(' · ')
  }

  function diskText(): string {
    const disk = snap?.disk
    if (!disk?.free_bytes) return disk?.path || '—'
    return `свободно ${fmtBytes(disk.free_bytes)}`
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
      class="modal-box system-box"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="Система"
    >
      <div class="system-head">
        <div class="system-heading">
          <div class="modal-title">Система</div>
          <div class="system-sub">
            {@html statusBadge(overallKind, overallLabel)}
            <span>{osText() || 'Проверка окружения'}</span>
          </div>
        </div>
        <button class="icon-btn" aria-label="Закрыть состояние системы" onclick={close}>✕</button>
      </div>

      <div class="system-body">
        <div class="system-hero">
          <div>
            <div class="system-hero-title">Проверка настроек</div>
            <div class="help-text">
              {#if system.checkedAt}
                Последняя проверка {fmtDateTime(system.checkedAt)}
              {:else}
                Проверка займёт несколько секунд и не мешает уже идущей записи.
              {/if}
            </div>
          </div>
          <div class="row-start">
            <button
              class="btn-primary"
              disabled={system.checksLoading}
              onclick={() => void runChecks(probeAudio)}
            >
              {system.checksLoading ? 'Проверяю…' : 'Проверить настройки'}
            </button>
            <button class="btn-secondary btn-sm" disabled={system.loading} onclick={refresh}>
              {system.loading ? 'Обновляю…' : 'Обновить'}
            </button>
          </div>
        </div>

        <label class="setting-check system-probe">
          <input type="checkbox" bind:checked={probeAudio} />
          <span class="setting-switch" aria-hidden="true"></span>
          <span>Проверять микрофон короткой записью</span>
        </label>

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
              <span class="system-val">{systemAudio ? 'доступен' : 'не найден'}</span>
              <span class="system-key">Запись экрана</span>
              <span class="system-val">{screenReady ? 'доступна' : 'нет разрешения'}</span>
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
              <span class="system-val">
                {coreReady
                  ? 'базовые установлены'
                  : `не хватает: ${missingModelsText || ' '}`}
              </span>
              <span class="system-key">ffmpeg</span>
              <span class="system-val">{ffmpegReady ? 'установлен' : 'не установлен'}</span>
            </div>
            <button class="btn-secondary btn-sm" onclick={() => openSettings('models')}>
              Открыть «Компоненты»
            </button>
          </section>

          {#if unacknowledged > 0 || journalTotal > 0}
            <section class="system-card system-card-wide">
              <div class="system-card-head">
                <span class="card-title">Сбои и журнал</span>
                {@html unacknowledged > 0
                  ? statusBadge('error', `${unacknowledged} ${crashWord(unacknowledged)}`)
                  : statusBadge('ok', 'Чисто')}
              </div>
              <div class="system-facts">
                <span class="system-key">Сбои</span>
                <span class="system-val">{unacknowledged || 'нет'}</span>
                <span class="system-key">Записей</span>
                <span class="system-val">{journalTotal || 'нет'}</span>
              </div>
              {#if lastCrash}
                <button class="btn-secondary btn-sm" onclick={() => void ackCrash(lastCrash.id)}>
                  Скрыть сбой
                </button>
              {/if}
            </section>
          {/if}
        </div>

        {#if system.checks.length}
          <section class="system-section">
            <div class="system-section-head">
              <span class="card-title">Проверки</span>
              <span class="help-text">{summaryText()}</span>
            </div>
            {#if problemChecks.length}
              <div class="recordings-list">
                {#each problemChecks as check (check.id)}
                  {@render checkRow(check)}
                {/each}
              </div>
            {/if}
            <details class="collapse">
              <summary class="collapse-head">
                <span>Все проверки ({system.checks.length})</span>
                <span class="collapse-chev" aria-hidden="true">›</span>
              </summary>
              <div class="collapse-body">
                <div class="recordings-list">
                  {#each system.checks as check (check.id)}
                    {@render checkRow(check)}
                  {/each}
                </div>
              </div>
            </details>
          </section>
        {/if}

        {#if journalTotal > 0}
        <section class="system-section">
          <div class="system-section-head">
            <span class="card-title">Последние записи</span>
            {#if system.journalTotal > 5}
              <button
                class="link-btn"
                onclick={() => (journalExpanded = !journalExpanded)}
              >
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
          {#if system.journalTruncated && !journalExpanded}
            <div class="help-text">
              Показаны последние {system.journal.length} из {system.journalTotal}.
            </div>
          {/if}
        </section>
        {/if}

        {#if lastCrash}
          <section class="system-section">
            <div class="system-section-head">
              <span class="card-title">Сбои</span>
              <span class="help-text">{crashKind()}</span>
            </div>
            <div class="rec-item">
              <div class="rec-info">
                <span class="rec-name">{fmtDateTime(lastCrash.created_at) || 'без даты'}</span>
                <span class="rec-meta">
                  {crashMessage(lastCrash) || 'Сообщение не записано.'}
                </span>
                <span class="rec-meta">
                  {#if crashVersion(lastCrash)}Версия {crashVersion(lastCrash)}{/if}
                  {#if crashHasBacktrace(lastCrash)} · есть стек вызовов{/if}
                </span>
              </div>
              <div class="rec-actions">
                <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
                  Собрать отчёт
                </button>
                <button class="btn-secondary btn-sm" onclick={() => void ackCrash(lastCrash.id)}>
                  Скрыть
                </button>
              </div>
            </div>
          </section>
        {/if}

        <details class="collapse system-details">
          <summary class="collapse-head">
            <span class="card-title">Подробности</span>
            <span class="collapse-chev" aria-hidden="true">›</span>
          </summary>
          <div class="collapse-body">
            <div class="system-facts">
              {#each pathRows as [key, value] (key)}
                <span class="system-key">{PATH_LABELS[key] || key}</span>
                <span class="system-val system-path">{value}</span>
              {/each}
            </div>
            <div class="system-facts">
              <span class="system-key">Диск</span><span class="system-val">{diskText()}</span>
              <span class="system-key">Память</span><span class="system-val">{memoryText()}</span>
              {#if snap?.log?.path}
                <span class="system-key">Лог</span>
                <span class="system-val system-path">{snap.log.path}</span>
              {/if}
            </div>
            {#if !isMobile}
              <div class="row-start">
                <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
                  Собрать отчёт
                </button>
                <button class="btn-secondary btn-sm" onclick={() => void openTarget('diagnostics')}>
                  Открыть папку
                </button>
              </div>
            {/if}
          </div>
        </details>

      </div>
    </div>
  </div>
{/if}

<style>
  .system-overlay {
    z-index: 70;
  }
  .system-box {
    max-width: 760px;
    padding: 0;
    gap: 0;
    max-height: calc(100dvh - 56px);
    overflow: hidden;
  }
  .system-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 20px 22px 14px;
    border-bottom: 1px solid var(--border-soft);
    flex-shrink: 0;
  }
  .system-heading {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .system-sub {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    color: var(--text-muted);
    font-size: 12px;
  }
  .system-body {
    overflow-y: auto;
    padding: 18px 22px 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .system-hero {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    flex-wrap: wrap;
  }
  .system-hero-title {
    font-size: 15px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .system-probe {
    margin-top: -4px;
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
    padding: 13px 14px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    min-width: 0;
  }
  .system-card-wide {
    grid-column: 1 / -1;
  }
  .system-card > .btn-secondary {
    align-self: flex-start;
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
    grid-template-columns: minmax(100px, max-content) minmax(0, 1fr);
    gap: 4px 12px;
    font-size: 12.5px;
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
  .system-details {
    border-top: 1px solid var(--border-soft);
    padding-top: 10px;
  }
  .system-box :global(.status-badge.error) {
    color: var(--accent-rec);
  }
  @media (max-width: 640px) {
    .system-box {
      max-height: calc(100dvh - 24px);
    }
    .system-head {
      padding: 16px 16px 12px;
    }
    .system-body {
      padding: 14px 16px 16px;
    }
    .system-grid {
      grid-template-columns: 1fr;
    }
    .system-hero .row-start {
      width: 100%;
    }
    .system-hero .btn-primary {
      flex: 1;
    }
  }
</style>
