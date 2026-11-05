<script lang="ts">
  import { onMount } from 'svelte'
  import { isMobile } from '../lib/api'
  import { copyText, toast } from '../lib/toast.svelte.ts'
  import {
    ackCrash,
    collectDiagnostics,
    crashHasBacktrace,
    crashLocation,
    crashMessage,
    crashUnacknowledged,
    crashVersion,
    fmtDateTime,
    loadCrashes,
    openTarget,
    system,
  } from '../lib/system.svelte.ts'

  const crash = $derived(system.crashes.find(crashUnacknowledged) ?? null)

  onMount(() => {
    if (system.crashes.length === 0) void loadCrashes()
  })

  function summaryText(): string {
    if (!crash) return ''
    const lines = [`Подтекст ${crashVersion(crash)}`.trim()]
    lines.push(`Сбой: ${fmtDateTime(crash.created_at) || 'без даты'}`)
    lines.push(`Тип: ${crash.kind || 'неизвестен'}`)
    if (crash.thread) lines.push(`Поток: ${crash.thread}`)
    if (crashLocation(crash)) lines.push(`Место: ${crashLocation(crash)}`)
    if (crashMessage(crash)) lines.push(`Сообщение: ${crashMessage(crash)}`)
    if (crashHasBacktrace(crash)) lines.push('Есть стек вызовов')
    if (crash.synthetic) lines.push('Завершение без корректного выхода')
    return lines.join('\n')
  }

  async function copySummary(): Promise<void> {
    const ok = await copyText(summaryText())
    toast(ok ? 'Сводка скопирована' : 'Не удалось скопировать сводку', ok ? '' : 'error')
  }
</script>

{#if crash}
  <section class="card crash-banner" aria-label="Недавний сбой">
    <div class="card-header">
      <span class="card-title">
        {crash.synthetic ? 'Приложение завершилось без корректного выхода' : 'Приложение недавно завершилось с ошибкой'}
      </span>
      <span class="rec-meta">
        {fmtDateTime(crash.created_at) || 'без даты'}
        {#if crashVersion(crash)} · {crashVersion(crash)}{/if}
        {#if crash.synthetic} · завершение без корректного выхода{/if}
      </span>
    </div>
    <div class="help-text crash-message">
      {crashMessage(crash) || (crash.kind === 'unclean_exit'
        ? 'Приложение завершилось без корректного выхода.'
        : 'Сообщение не записано.')}
    </div>
    <div class="crash-actions">
      <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
        {isMobile ? 'Собрать и отправить' : 'Собрать отчёт'}
      </button>
      <button class="btn-secondary btn-sm" onclick={() => void openTarget('crashes')}>
        Открыть папку
      </button>
      <button class="btn-secondary btn-sm" onclick={() => void copySummary()}>
        Скопировать сводку
      </button>
      <a
        class="btn-secondary btn-sm"
        href="https://github.com/guest363/podtekst/issues/new?template=bug_report.yml"
        target="_blank"
        rel="noopener noreferrer"
      >Создать issue</a>
      <button class="btn-secondary btn-sm" onclick={() => void ackCrash(crash.id)}>Скрыть</button>
    </div>
    <div class="help-text">Отчёт и обращение создаются вручную: приложение ничего не отправляет само.</div>
  </section>
{/if}

<style>
  .crash-banner {
    border-color: #f3c6c6;
  }
  .crash-message {
    color: var(--text-primary);
    word-break: break-word;
  }
  .crash-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
</style>
