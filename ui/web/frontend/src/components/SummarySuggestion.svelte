<script lang="ts">
  import type { SummaryClassification } from '../lib/api'
  import {
    downloadSummaryModel,
    pickPreset,
    summaryModelPct,
    summaryState,
  } from '../lib/summary.svelte.ts'
  import { transcript } from '../lib/transcript.svelte.ts'
  import {
    summaryConfidenceText,
    summaryPreset,
    summaryRecommendation,
    summarySignals,
  } from '../lib/summaryUi.ts'

  interface Props {
    classification: SummaryClassification | null
    canGenerate: boolean
    blockingText: string
    onopenpickerat: (e: MouseEvent) => void
  }
  let { classification, canGenerate, blockingText, onopenpickerat }: Props = $props()

  const manualOnly = $derived(summaryState.capabilities?.classifier === 'manual_only')
  const recommendation = $derived(summaryRecommendation(classification, summaryState.presets))
  const signals = $derived(classification ? summarySignals(classification.signals) : [])
  const primaryId = $derived(
    summaryState.chosenPresetId ||
      classification?.preset_id ||
      summaryState.settings?.default_preset ||
      '',
  )
  const primaryPreset = $derived(summaryPreset(primaryId, summaryState.presets))
  const modelMissing = $derived(
    summaryState.blockReason === 'model_missing' ||
      summaryState.blockReason === 'model_download' ||
      summaryState.model?.is_installed === false,
  )
  const modelPct = $derived(summaryModelPct())
  const modelSizeGb = $derived(
    ((summaryState.model?.total_size_bytes || 2497281120) / 1073741824).toFixed(1).replace('.', ','),
  )
  const defaultLabel = $derived(
    summaryPreset(summaryState.settings?.default_preset || '', summaryState.presets)?.label || '',
  )
  const sourceChars = $derived(
    transcript.utterances.reduce((n, u) => n + (u.text?.trim().length || 0), 0) ||
      transcript.markdown.trim().length,
  )
  const shortSource = $derived(
    Boolean(
      summaryState.limits &&
        sourceChars > 0 &&
        sourceChars < summaryState.limits.warn_chars,
    ),
  )
</script>

{#if blockingText}
  <div class="summary-blocked">{blockingText}</div>
{/if}

{#if shortSource}
  <div class="summary-blocked">В записи мало текста. Итоги могут быть неточными.</div>
{/if}

{#if manualOnly}
  <div class="summary-rec">Рекомендация отключена: недостаточно проверенных данных. Выберите шаблон.</div>
{:else if classification}
  <div class="summary-rec-row">
    <span class="summary-guess">Предположение</span>
    <div class="summary-rec">{recommendation}</div>
  </div>
{:else if summaryState.settings?.default_preset}
  <div class="summary-rec">Сделать итоги по шаблону «{defaultLabel}»?</div>
{:else}
  <div class="summary-rec">Тип разговора не определён. Выберите шаблон.</div>
{/if}

{#if modelMissing}
  <div class="summary-model">
    <div class="summary-model-title">Модель итогов не скачана</div>
    <div class="help-text">Нужно около {modelSizeGb} ГБ. Работает локально, без сети.</div>
    {#if modelPct !== null}
      <div
        class="vu-track progress-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={modelPct}
        aria-label="Скачивание модели итогов"
      >
        <div class="vu-bar" style={`width: ${modelPct}%`}></div>
      </div>
      <div class="help-text">Скачиваю модель итогов… {modelPct}%</div>
    {:else}
      <button class="btn-primary btn-sm" disabled={!summaryState.model} onclick={downloadSummaryModel}>Скачать</button>
    {/if}
  </div>
{/if}

<div class="summary-actions">
  {#if primaryPreset && !modelMissing}
    <button class="btn-primary" disabled={!canGenerate} onclick={() => void pickPreset(primaryPreset.id)}>
      {classification?.ambiguous ? `Сделать по шаблону «${primaryPreset.label}»` : 'Сделать итоги'}
    </button>
  {/if}
  <button class="btn-secondary" onclick={(e) => onopenpickerat(e)}>
    {classification ? 'Другой шаблон' : 'Выбрать шаблон'}
  </button>
</div>

{#if classification && !manualOnly}
  {#if classification.alternatives.length > 0}
    <div class="summary-alternatives">
      <span class="help-text">Сделать по шаблону:</span>
      {#each classification.alternatives.slice(0, 2) as alt}
        <button
          type="button"
          class="btn-secondary btn-sm"
          disabled={!canGenerate}
          onclick={() => void pickPreset(alt)}
        >
          {summaryPreset(alt, summaryState.presets)?.label || alt}
        </button>
      {/each}
      <button type="button" class="link-btn" onclick={(e) => onopenpickerat(e)}>Ещё…</button>
    </div>
  {/if}
  <details class="summary-why">
    <summary>Почему так решили</summary>
    <div class="help-text">
      {summaryConfidenceText(classification.confidence)}:
      {signals.length ? signals.join(', ') : 'сигналов недостаточно'}
    </div>
  </details>
{/if}
<div class="help-text summary-privacy">
  Анализ идёт на этом компьютере. Текст никуда не отправляется.
</div>

<style>
  .summary-rec-row { display: flex; flex-direction: column; gap: 4px; }
  .summary-rec { font-size: 13px; line-height: 1.5; color: var(--text-primary); max-width: 78ch; }
  .summary-guess {
    align-self: flex-start; font-size: 11.5px; font-weight: 700; color: var(--accent-blue);
    border: 1px solid currentColor; border-radius: var(--radius-pill); padding: 1px 7px;
  }
  .summary-actions, .summary-alternatives {
    display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  }
  .summary-actions .btn-primary, .summary-actions .btn-secondary { min-height: 36px; }
  .summary-actions button:focus-visible, .summary-alternatives button:focus-visible,
  .summary-model button:focus-visible {
    outline: 2px solid var(--accent-blue); outline-offset: 2px;
  }
  .summary-model {
    display: flex; flex-direction: column; gap: 8px;
    background: var(--bg-inset); border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner); padding: 10px 12px;
    font-size: 12.5px; line-height: 1.5; color: var(--text-primary);
  }
  .summary-model-title { font-size: 13px; font-weight: 700; }
  .summary-model > .btn-primary { align-self: flex-start; }
  .summary-blocked {
    display: flex; flex-direction: column; gap: 8px;
    background: var(--accent-amber-bg); border: 1px solid #e8d6a8;
    border-radius: var(--radius-inner); padding: 10px 12px;
    font-size: 12.5px; line-height: 1.5; color: var(--text-primary);
  }
  .summary-why { font-size: 12px; color: var(--text-muted); }
  .summary-why summary {
    cursor: pointer; color: var(--accent-blue); font-weight: 600; width: fit-content;
  }
  .summary-privacy { font-size: 11.5px; }
  @media (max-width: 720px) {
    .summary-actions { flex-direction: column; align-items: stretch; }
    .summary-actions .btn-primary, .summary-actions .btn-secondary {
      width: 100%; min-height: 40px;
    }
    .summary-model > .btn-primary { width: 100%; min-height: 40px; }
    .summary-alternatives { gap: 6px; }
  }
  @media (prefers-reduced-motion: reduce) {
    .summary-guess, .summary-why summary { transition: none; }
  }
</style>
