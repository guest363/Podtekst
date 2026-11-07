<script lang="ts">
  import { tick } from 'svelte'
  import { closeTranscript, emit } from '../lib/ui.svelte.ts'
  import {
    cancelSensitive,
    cancelSummary,
    confirmSensitive,
    retryLoad,
    retrySummary,
    saveSummarySettings,
    showPreviousResult,
    summaryState,
  } from '../lib/summary.svelte.ts'
  import { summaryElapsed } from '../lib/summaryUi.ts'
  import { toast } from '../lib/toast.svelte.ts'

  let confirmHead = $state<HTMLElement | null>(null)

  const runtimeMissing = $derived(summaryState.blockReason === 'runtime_missing')
  const hardBlock = $derived(
    summaryState.blockReason === 'too_short' ||
      summaryState.blockReason === 'unsupported' ||
      summaryState.blockReason === 'runtime_missing' ||
      summaryState.blockReason === 'low_memory',
  )
  const blockingText = $derived.by(() => {
    switch (summaryState.blockReason) {
      case 'low_memory':
        return 'Не хватает памяти. Нужно не меньше 12 ГБ ОЗУ и 6,5 ГБ свободно; рекомендуется 16 ГБ.'
      case 'runtime_missing':
        return 'Итоги недоступны: локальный движок не найден. Установите компонент или запустите нативное приложение.'
      case 'too_short':
        return 'В записи мало текста для итогов.'
      case 'unsupported':
        return 'Итоги на этом устройстве не поддерживаются.'
      default:
        return ''
    }
  })
  const errorCode = $derived(summaryState.error?.code || '')
  const shortRetry = $derived(
    errorCode === 'low_memory' ||
      errorCode === 'oom' ||
      errorCode === 'timeout' ||
      errorCode === 'output_thin' ||
      errorCode === 'output_capped',
  )
  const settingsRetry = $derived(
    errorCode === 'runtime_missing' ||
      errorCode === 'low_memory' ||
      errorCode === 'oom' ||
      errorCode === 'model_load' ||
      shortRetry,
  )

  $effect(() => {
    if (summaryState.phase === 'confirm-sensitive') {
      void tick().then(() => confirmHead?.focus())
    }
  })

  function openSettings(): void {
    emit('open-settings', 'summary')
  }

  async function retryShort(): Promise<void> {
    try {
      await saveSummarySettings({ detail: 'short' })
      retrySummary()
    } catch (e) {
      toast('Не удалось сохранить подробность: ' + (e as Error).message, 'error')
    }
  }
</script>

{#if summaryState.phase === 'generating'}
  <div class="summary-progress">
    <div class="summary-progress-head">
      <span>{summaryState.stage || 'Собираю итоги…'}</span>
      <span class="summary-pct">{summaryState.pct}%</span>
    </div>
    <div
      class="vu-track progress-track"
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={summaryState.pct}
      aria-valuetext={summaryState.stage || 'Сборка итогов'}
      aria-label="Прогресс сборки итогов"
    >
      <div class="vu-bar" style={`width: ${summaryState.pct}%`}></div>
    </div>
    <div class="summary-progress-foot">
      <span class="help-text">Итоги появятся после завершения. Прошло {summaryElapsed(summaryState.elapsedSec)}.</span>
      <button
        class="btn-secondary btn-sm"
        disabled={!summaryState.jobId}
        onclick={() => void cancelSummary()}
      >
        Остановить
      </button>
    </div>
    {#if summaryState.note}<div class="help-text">{summaryState.note}</div>{/if}
  </div>
{:else if summaryState.phase === 'confirm-sensitive'}
  <div class="summary-confirm" role="group" aria-labelledby="summary-confirm-title">
    <div
      class="summary-confirm-title"
      id="summary-confirm-title"
      tabindex="-1"
      bind:this={confirmHead}
    >
      Шаблон «{summaryState.pendingPresetId
        ? summaryState.presets.find((p) => p.id === summaryState.pendingPresetId)?.label ||
          summaryState.chosenPresetId
        : summaryState.chosenPresetId}» относится к чувствительным темам
    </div>
    <div class="help-text">
      {summaryState.presets.find((p) => p.id === summaryState.pendingPresetId)?.caution ||
        'Проверьте формулировки по исходным репликам.'}
    </div>
    <div class="summary-actions">
      <button class="btn-primary" onclick={confirmSensitive}>Собрать итоги</button>
      <button class="btn-secondary" onclick={cancelSensitive}>Отмена</button>
    </div>
  </div>
{:else if summaryState.phase === 'error' && summaryState.error}
  <div class="summary-error" role="alert">
    <div class="summary-error-text">{summaryState.error.message}</div>
    <div class="summary-actions">
      {#if summaryState.error.scope === 'load'}
        <button class="btn-secondary btn-sm" onclick={retryLoad}>Повторить</button>
      {:else}
        <button class="btn-secondary btn-sm" onclick={retrySummary}>Повторить</button>
      {/if}
      {#if shortRetry}
        <button class="btn-secondary btn-sm" onclick={() => void retryShort()}>
          Попробовать «Кратко»
        </button>
      {/if}
      {#if settingsRetry}
        <button class="btn-secondary btn-sm" onclick={openSettings}>Настроить</button>
      {/if}
      {#if summaryState.record}
        <button class="link-btn" onclick={showPreviousResult}>Открыть прежние итоги</button>
      {/if}
    </div>
  </div>
{:else if hardBlock}
  <div class="summary-blocked">
    <div>{blockingText}</div>
    <div class="summary-actions">
      {#if runtimeMissing || summaryState.blockReason === 'low_memory'}
        <button class="btn-secondary btn-sm" onclick={openSettings}>Настроить</button>
        <button class="btn-secondary btn-sm" onclick={retryLoad}>Повторить</button>
      {:else if summaryState.blockReason === 'too_short'}
        <button class="btn-secondary btn-sm" onclick={closeTranscript}>Другая запись</button>
      {/if}
    </div>
  </div>
{:else if summaryState.phase === 'loading'}
  <div class="summary-loading">
    <span>Загружаю итоги…</span>
    <div class="vu-track progress-track summary-indeterminate" aria-hidden="true">
      <div class="vu-bar"></div>
    </div>
  </div>
{/if}

<style>
  .summary-progress, .summary-confirm, .summary-error, .summary-blocked, .summary-loading {
    display: flex; flex-direction: column; gap: 8px;
    background: var(--bg-inset); border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner); padding: 10px 12px;
    font-size: 12.5px; line-height: 1.5; color: var(--text-primary);
  }
  .summary-progress-head, .summary-progress-foot {
    display: flex; align-items: center; justify-content: space-between;
    gap: 8px; flex-wrap: wrap;
  }
  .summary-pct { font-variant-numeric: tabular-nums; font-weight: 700; }
  .summary-confirm-title { font-size: 13px; font-weight: 700; }
  .summary-confirm-title:focus-visible { outline: 2px solid var(--accent-blue); outline-offset: 2px; }
  .summary-blocked {
    background: var(--accent-amber-bg); border-color: #e8d6a8;
  }
  .summary-error { background: #fdf2f2; border-color: #f0caca; }
  .summary-error-text { color: var(--accent-rec); }
  .summary-loading { background: transparent; border: none; padding: 4px 0; }
  .summary-indeterminate .vu-bar {
    width: 40%; animation: summary-slide 1.2s var(--ease-spring) infinite alternate;
  }
  .summary-actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .summary-actions button:focus-visible {
    outline: 2px solid var(--accent-blue); outline-offset: 2px;
  }
  @keyframes summary-slide { from { transform: translateX(0); } to { transform: translateX(150%); } }
  @media (max-width: 720px) {
    .summary-actions { flex-direction: column; align-items: stretch; }
    .summary-actions .btn-primary, .summary-actions .btn-secondary {
      width: 100%; min-height: 40px;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .summary-indeterminate .vu-bar { animation: none; width: 100%; }
  }
</style>
