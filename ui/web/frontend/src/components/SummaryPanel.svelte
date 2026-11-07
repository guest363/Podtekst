<script lang="ts">
  import { tick } from 'svelte'
  import { isMobile } from '../lib/api'
  import { engine, transcript } from '../lib/transcript.svelte.ts'
  import {
    cancelSensitive,
    copySummary,
    exportSummary,
    generateSummary,
    loadSummary,
    pickPreset,
    summaryState,
    toggleSummary,
    updateStaleSummary,
  } from '../lib/summary.svelte.ts'
  import {
    summaryDuration,
    summaryPreset,
    summaryShortDate,
    summaryWarnings,
  } from '../lib/summaryUi.ts'
  import SummaryPresetPicker from './SummaryPresetPicker.svelte'
  import SummaryResult from './SummaryResult.svelte'
  import SummaryStage from './SummaryStage.svelte'
  import SummarySuggestion from './SummarySuggestion.svelte'

  interface Props {
    file: string
    onseek: (sec: number) => void
    transcriptMarkdown: () => string
  }
  let { file, onseek, transcriptMarkdown }: Props = $props()

  let panelEl = $state<HTMLElement | null>(null)
  let toggleBtn = $state<HTMLButtonElement | null>(null)
  let pickerOpen = $state(false)
  let pickerAnchor = $state<HTMLElement | null>(null)
  let loadedFor = ''
  let wasGenerating = false

  $effect(() => {
    if (file && file !== loadedFor) {
      loadedFor = file
      void loadSummary(file)
    }
  })

  const visible = $derived(
    !isMobile &&
      transcript.hasText &&
      summaryState.capabilities?.summary_supported !== false &&
      summaryState.blockReason !== 'no_text' &&
      summaryState.phase !== 'idle',
  )
  const classification = $derived(
    summaryState.phase === 'suggest' && !summaryState.record && summaryState.classification?.preset_id
      ? summaryState.classification
      : null,
  )
  const currentPreset = $derived(
    summaryPreset(
      summaryState.phase === 'confirm-sensitive'
        ? summaryState.pendingPresetId || summaryState.chosenPresetId
        : summaryState.record?.preset_id ||
            summaryState.chosenPresetId ||
            summaryState.classification?.preset_id ||
            '',
      summaryState.presets,
    ),
  )
  const warnings = $derived(summaryState.record ? summaryWarnings(summaryState.record) : [])
  const hardBlock = $derived(
    summaryState.blockReason === 'too_short' ||
      summaryState.blockReason === 'unsupported' ||
      summaryState.blockReason === 'runtime_missing' ||
      summaryState.blockReason === 'low_memory',
  )
  const softBlock = $derived(
    summaryState.blockReason === 'transcribing' ||
      summaryState.blockReason === 'recording' ||
      summaryState.blockReason === 'busy_summary' ||
      engine.running,
  )
  const modelMissing = $derived(
    summaryState.blockReason === 'model_missing' ||
      summaryState.blockReason === 'model_download' ||
      summaryState.model?.is_installed === false,
  )
  const primaryPreset = $derived(
    summaryPreset(
      summaryState.chosenPresetId ||
        summaryState.classification?.preset_id ||
        summaryState.settings?.default_preset ||
        '',
      summaryState.presets,
    ),
  )
  const canGenerate = $derived(!hardBlock && !softBlock && !modelMissing && Boolean(primaryPreset))
  const canRegenerate = $derived(
    !hardBlock && !softBlock && !modelMissing && Boolean(summaryState.record?.preset_id || primaryPreset),
  )
  const softBlockText = $derived.by(() => {
    if (summaryState.blockReason === 'transcribing') {
      return 'Идёт расшифровка. Итоги можно собрать после её окончания.'
    }
    if (summaryState.blockReason === 'recording') {
      return 'Идёт запись. Итоги можно собрать после её окончания.'
    }
    if (summaryState.blockReason === 'busy_summary') {
      return 'Итоги уже собираются для другой записи. Повторите позже.'
    }
    if (engine.running) {
      return 'Идёт расшифровка. Итоги можно собрать после её окончания.'
    }
    return ''
  })
  const barStatus = $derived.by(() => {
    if (summaryState.phase === 'loading') return 'Загружаю итоги…'
    if (summaryState.phase === 'generating') return `Собираю итоги… ${summaryState.pct}%`
    if (summaryState.phase === 'error') return 'Не удалось собрать итоги'
    if (summaryState.phase === 'confirm-sensitive') return 'Нужно подтверждение'
    if (summaryState.phase === 'result' && summaryState.record) {
      return `Готово · ${summaryShortDate(summaryState.record.created_at)} · ${summaryDuration(summaryState.record.duration_ms)}`
    }
    return ''
  })
  const liveText = $derived.by(() => {
    if (summaryState.phase === 'generating') return summaryState.stage || 'Собираю итоги'
    if (summaryState.phase === 'result' && summaryState.record) return 'Итоги готовы'
    if (summaryState.phase === 'error') return 'Не удалось собрать итоги'
    return ''
  })

  $effect(() => {
    if (summaryState.phase === 'generating') wasGenerating = true
    if (summaryState.phase === 'result' && wasGenerating) {
      const inside = panelEl?.contains(document.activeElement)
      wasGenerating = false
      if (inside) {
        void tick().then(() => panelEl?.querySelector<HTMLElement>('.summary-heading')?.focus())
      }
    }
  })

  function openPickerAt(e: MouseEvent): void {
    pickerAnchor = e.currentTarget as HTMLElement
    pickerOpen = true
  }
  function openPicker(): void {
    pickerAnchor = panelEl
    pickerOpen = true
  }
  function closePicker(): void {
    pickerOpen = false
  }
  function choose(id: string): void {
    pickerOpen = false
    void pickPreset(id)
  }
  function regenerate(): void {
    const record = summaryState.record
    if (record) void generateSummary(record.preset_id, { force: true })
  }
  function onWindowKey(e: KeyboardEvent): void {
    if (!visible || e.key !== 'Escape') return
    if (pickerOpen) {
      e.preventDefault()
      e.stopPropagation()
      closePicker()
      return
    }
    if (summaryState.phase === 'confirm-sensitive') {
      e.preventDefault()
      e.stopPropagation()
      cancelSensitive()
      return
    }
    if (summaryState.expanded && panelEl?.contains(document.activeElement)) {
      e.preventDefault()
      e.stopPropagation()
      toggleSummary()
      toggleBtn?.focus()
    }
  }
</script>

<svelte:window onkeydowncapture={onWindowKey} />

{#if visible}
  <section
    class="summary-block"
    class:is-suggest={summaryState.phase === 'suggest' || summaryState.phase === 'loading'}
    aria-labelledby="summary-title"
    aria-busy={summaryState.generating}
    bind:this={panelEl}
  >
    <div class="summary-bar">
      <div class="summary-head">
        <h2 class="summary-title" id="summary-title">Итоги встречи</h2>
        <div class="summary-badges">
          {#if currentPreset && (summaryState.record || summaryState.chosenPresetId)}
            <span class="summary-badge">{currentPreset.label}</span>
          {/if}
          {#if summaryState.phase === 'result' && summaryState.record}
            <span class="summary-badge ok">Сгенерировано</span>
          {/if}
          {#if currentPreset?.sensitive && summaryState.record}
            <span class="summary-badge draft">Черновик</span>
          {/if}
          {#if summaryState.stale}
            <span class="summary-badge warn">Расшифровка изменена</span>
          {/if}
          {#if warnings.length > 0}
            <span class="summary-badge warn">Есть замечания</span>
          {/if}
        </div>
        <button
          type="button"
          class="summary-toggle"
          aria-expanded={summaryState.expanded}
          aria-controls="summary-body"
          bind:this={toggleBtn}
          onclick={toggleSummary}
        >
          {summaryState.expanded ? 'Свернуть' : 'Открыть'}
        </button>
      </div>
      {#if barStatus}
        <div class="summary-status">{barStatus}</div>
      {/if}
    </div>

    {#if summaryState.expanded}
      <div class="summary-body" id="summary-body">
        {#if summaryState.notice && summaryState.phase !== 'generating'}
          <div class="summary-notice">{summaryState.notice}</div>
        {/if}
        {#if softBlock && summaryState.phase === 'result'}
          <div class="summary-blocked">{softBlockText}</div>
        {/if}
        {#if summaryState.phase === 'result' && summaryState.record}
          <SummaryResult
            record={summaryState.record}
            stale={summaryState.stale}
            sensitive={currentPreset?.sensitive ?? false}
            caution={currentPreset?.caution ?? ''}
            regenerateDisabled={!canRegenerate}
            {onseek}
            onregenerate={regenerate}
            onupdate={updateStaleSummary}
            onchange={openPicker}
            oncopy={() => void copySummary()}
            onexport={() => void exportSummary(transcriptMarkdown())}
          />
        {:else if summaryState.phase === 'suggest' && !hardBlock}
          <SummarySuggestion
            {classification}
            {canGenerate}
            blockingText={softBlockText}
            onopenpickerat={openPickerAt}
          />
        {:else}
          <SummaryStage />
        {/if}
      </div>
    {/if}

    <div class="sr-only" aria-live="polite">{liveText}</div>
  </section>

  <SummaryPresetPicker
    open={pickerOpen}
    presets={summaryState.presets}
    recommendedId={summaryState.classification?.preset_id || ''}
    currentId={summaryState.record?.preset_id || summaryState.chosenPresetId}
    anchor={pickerAnchor}
    onpick={choose}
    onclose={closePicker}
  />
{/if}

<style>
  .summary-block {
    display: flex; flex-direction: column; gap: 8px; flex-shrink: 0;
    background: #fff; border: 1px solid var(--border-card);
    border-radius: var(--radius-inner); padding: 10px 12px; min-width: 0;
  }
  .summary-block.is-suggest { min-height: 72px; }
  .summary-bar { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .summary-head {
    display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0;
  }
  .summary-title {
    font-size: 15px; font-weight: 800; letter-spacing: -0.01em;
    color: var(--text-primary); margin: 0;
  }
  .summary-badges {
    display: flex; align-items: center; gap: 4px; flex-wrap: wrap; min-width: 0;
  }
  .summary-badge {
    font-size: 11.5px; font-weight: 600; border: 1px solid var(--border-card);
    border-radius: var(--radius-pill); padding: 1px 7px; color: var(--text-muted);
    background: #fff; white-space: nowrap;
  }
  .summary-badge.ok { color: var(--accent-ok); border-color: var(--accent-ok); }
  .summary-badge.warn, .summary-badge.draft {
    color: var(--accent-amber); border-color: #d9b45c;
  }
  .summary-toggle {
    margin-left: auto; border: none; background: none; color: var(--accent-blue);
    font: inherit; font-size: 12px; font-weight: 600; cursor: pointer; padding: 6px 2px;
    text-decoration: underline; text-underline-offset: 2px;
  }
  .summary-toggle:focus-visible {
    outline: 2px solid var(--accent-blue); outline-offset: 2px;
  }
  .summary-status {
    font-size: 12px; color: var(--text-muted); font-variant-numeric: tabular-nums;
  }
  .summary-body {
    display: flex; flex-direction: column; gap: 10px;
    max-height: min(38vh, 420px); overflow-y: auto; padding-right: 2px; min-height: 0;
  }
  .summary-body > :global(.summary-result) { max-width: 78ch; }
  .summary-notice {
    font-size: 12px; color: var(--text-muted); background: var(--bg-inset);
    border: 1px solid var(--border-soft); border-radius: var(--radius-sm); padding: 6px 9px;
  }
  .summary-blocked {
    font-size: 12px; color: var(--accent-amber); background: var(--accent-amber-bg);
    border: 1px solid #e8d6a8; border-radius: var(--radius-sm); padding: 6px 9px;
  }
  .sr-only {
    position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px;
    overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0;
  }
  @media (max-width: 720px) {
    .summary-block { min-height: 104px; padding: 10px; }
    .summary-body { max-height: none; overflow: visible; padding-right: 0; }
    .summary-head { align-items: flex-start; }
    .summary-toggle { min-height: 40px; padding: 8px 2px; }
  }
  @media (prefers-reduced-motion: reduce) {
    .summary-toggle, .summary-badge { transition: none; }
  }
  @media print {
    .summary-block { display: none !important; }
  }
</style>
