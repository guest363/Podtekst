<script lang="ts">
  import { onMount } from 'svelte'
  import { fmtClock, isMobile } from '../lib/api'
  import { statusBadge } from '../lib/settings.svelte.ts'
  import { emit } from '../lib/ui.svelte.ts'
  import { openUpdateModal, update } from '../lib/update.svelte.ts'
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
  import type { CheckFix, JournalSession, SystemCheck } from '../lib/system.svelte.ts'

  let journalExpanded = $state(false)
  let probeAudio = $state(false)

  const snap = $derived(system.snapshot)
  const lastCrash = $derived(system.crashes.find(crashUnacknowledged) ?? null)
  const problemChecks = $derived(
    system.checks.filter((check) => check.status === 'warn' || check.status === 'error'),
  )
  const visibleJournal = $derived(
    journalExpanded ? system.journal : system.journal.slice(0, 5),
  )
  const unacknowledged = $derived(
    system.unacknowledged || snap?.crashes?.count || 0,
  )
  const journalTotal = $derived(system.journalTotal || snap?.journal?.count || 0)
  const freeBytes = $derived(snap?.disk?.free_bytes ?? 0)
  const totalBytes = $derived(snap?.disk?.total_bytes ?? 0)
  const usedBytes = $derived(totalBytes > freeBytes ? totalBytes - freeBytes : 0)
  const usedPct = $derived(
    totalBytes > 0 ? Math.min(100, Math.round((usedBytes / totalBytes) * 100)) : 0,
  )
  const freeHours = $derived(freeBytes > 0 ? Math.floor(freeBytes / (450 * 1024 * 1024)) : 0)

  const overallKind = $derived(
    unacknowledged > 0 || system.summary?.status === 'error'
      ? 'error'
      : system.summary?.status === 'warn'
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

  const MODE_LABELS: Record<string, string> = {
    split: '2 канала · наушники',
    mix: '1 канал',
  }

  onMount(() => {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
  })

  function refresh(): void {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
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

  function handleFix(fix: CheckFix | null): void {
    if (!fix) return
    if (fix.kind === 'open_folder' || fix.kind === 'open_settings') {
      void openTarget(fix.target)
    } else if (fix.kind === 'download_model' || fix.kind === 'download_ffmpeg') {
      emit('open-settings', 'models')
    } else if (fix.kind === 'open_update') {
      openUpdateModal()
    } else if (fix.kind === 'collect_diagnostics') {
      void collectDiagnostics()
    }
  }

  function crashKind(): string {
    if (!lastCrash) return ''
    return lastCrash.synthetic ? 'Завершение без корректного выхода' : 'Сбой'
  }
</script>

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

<div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Система и логи">
  <div class="system-section">
    <div class="system-card-head">
      <span class="card-title">Состояние системы</span>
      {@html statusBadge(overallKind, overallLabel)}
    </div>
    <div class="system-facts">
      <span class="system-key">Платформа</span>
      <span class="system-val">{osText() || 'Окружение'}</span>
      <span class="system-key">Версия</span>
      <span class="system-val">{update.current || '…'}</span>
      <span class="system-key">Память</span>
      <span class="system-val">{memoryText()}</span>
      {#if summaryText()}
        <span class="system-key">Проверки</span>
        <span class="system-val">{summaryText()}</span>
      {/if}
    </div>
    <div class="system-hero-actions pt-2">
      <button
        class="btn-primary btn-sm"
        disabled={system.checksLoading}
        onclick={() => void runChecks(!isMobile && probeAudio)}
      >
        {system.checksLoading ? 'Проверяю…' : 'Проверить систему'}
      </button>
      <button class="btn-secondary btn-sm" disabled={system.loading} onclick={refresh}>
        {system.loading ? '…' : 'Обновить'}
      </button>
      {#if !isMobile}
        <label class="setting-check system-probe-inline">
          <input type="checkbox" bind:checked={probeAudio} />
          <span class="setting-switch" aria-hidden="true"></span>
          <span>Тест микрофона короткой записью</span>
        </label>
      {/if}
    </div>
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

  <div class="system-section">
    <div class="system-card-head">
      <span class="card-title">Дисковое пространство</span>
      <span class="disk-meta">{diskText()}</span>
    </div>
    {#if totalBytes > 0}
      <div class="disk-gauge-track" aria-hidden="true">
        <div class="disk-gauge-fill" style="width: {usedPct}%"></div>
      </div>
    {/if}
    <div class="system-paths-block pt-2">
      <div class="system-path-row">
        <span class="system-key">Записи</span>
        <span class="system-val system-path" title={snap?.paths?.recordings}>
          {snap?.paths?.recordings || '—'}
        </span>
        {#if !isMobile}
          <button class="btn-secondary btn-xs" onclick={() => void openTarget('recordings')}>
            Открыть
          </button>
        {/if}
      </div>
      <div class="system-path-row">
        <span class="system-key">Данные</span>
        <span class="system-val system-path" title={snap?.paths?.data}>
          {snap?.paths?.data || '—'}
        </span>
        {#if !isMobile}
          <button class="btn-secondary btn-xs" onclick={() => void openTarget('data')}>
            Открыть
          </button>
        {/if}
      </div>
      {#if snap?.log?.path}
        <div class="system-path-row">
          <span class="system-key">Лог</span>
          <span class="system-val system-path" title={snap.log.path}>{snap.log.path}</span>
          {#if !isMobile}
            <button class="btn-secondary btn-xs" onclick={() => void openTarget('logs')}>
              Открыть
            </button>
          {/if}
        </div>
      {/if}
    </div>
    {#if !isMobile}
      <div class="path-chips pt-1">
        <button class="chip-btn" onclick={() => void openTarget('logs')}>Папка логов</button>
        <button class="chip-btn" onclick={() => void openTarget('models')}>Папка моделей</button>
        <button class="chip-btn" onclick={() => void openTarget('crashes')}>Папка сбоев</button>
        <button class="chip-btn" onclick={() => void openTarget('diagnostics')}>Диагностика</button>
      </div>
    {/if}
  </div>

  <div class="system-section">
    <div class="system-card-head">
      <span class="card-title">Журнал сессий и сбои</span>
      {@html lastCrash ? statusBadge('error', 'Есть сбой') : statusBadge('ok', 'Сбоев нет')}
    </div>

    {#if lastCrash}
      <div class="rec-item">
        <div class="rec-info">
          <span class="rec-name">{crashKind()} · {fmtDateTime(lastCrash.created_at) || 'без даты'}</span>
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
    {/if}

    {#if journalTotal > 0}
      <div class="journal-block">
        <div class="system-card-head">
          <span class="rec-name">Недавние сессии ({journalTotal})</span>
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
      </div>
    {/if}

    <div class="row-start pt-2">
      <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
        {isMobile ? 'Собрать и отправить отчёт' : 'Собрать отчёт для поддержки'}
      </button>
      {#if !isMobile}
        <button class="btn-secondary btn-sm" onclick={() => void openTarget('logs')}>
          Открыть папку логов
        </button>
      {/if}
    </div>
  </div>
</div>

<style>
  .system-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner);
    background: var(--bg-inset);
  }
  .system-card-head {
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
  .system-hero-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .system-probe-inline {
    margin-left: 8px;
    font-size: 12px;
  }
  .disk-meta {
    font-size: 12px;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-weight: 500;
  }
  .disk-gauge-track {
    height: 7px;
    background: var(--border-soft);
    border-radius: var(--radius-pill);
    overflow: hidden;
    width: 100%;
    margin-top: 2px;
  }
  .disk-gauge-fill {
    height: 100%;
    background: var(--text-primary);
    border-radius: var(--radius-pill);
    transition: width 0.3s var(--ease-spring);
  }
  .system-paths-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .system-path-row {
    display: grid;
    grid-template-columns: 70px minmax(0, 1fr) auto;
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
    padding: 3px 9px;
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
  }
  .problem-alert {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border-radius: var(--radius-card);
    background: var(--accent-amber-bg);
    border: 1px solid rgba(138, 90, 0, 0.15);
  }
  .problem-alert-title {
    font-size: 12.5px;
    font-weight: 700;
    color: var(--accent-amber);
  }
  .check-hint {
    color: var(--text-faint);
  }
  .journal-block {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 6px;
  }
  .pt-1 {
    padding-top: 4px;
  }
  .pt-2 {
    padding-top: 8px;
  }

  @media (max-width: 640px) {
    .system-section {
      padding: 12px 12px;
    }
    .system-path-row {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    .system-probe-inline {
      margin-left: 0;
      width: 100%;
    }
    .system-hero-actions {
      flex-direction: column;
      align-items: stretch;
    }
  }
</style>
