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

<div class="summary-suggest">
  {#if blockingText}
    <div class="summary-callout" role="status">
      <span class="summary-callout-text">{blockingText}</span>
    </div>
  {/if}

  {#if shortSource}
    <div class="summary-callout" role="status">
      <span class="summary-callout-text">
        <strong>Короткая запись.</strong> Итоги могут быть неточными.
      </span>
    </div>
  {/if}

  {#if manualOnly}
    <p class="summary-hint">
      Рекомендация отключена: недостаточно проверенных данных. Выберите шаблон.
    </p>
  {:else if !classification && !summaryState.settings?.default_preset}
    <p class="summary-hint">Тип разговора не определён. Выберите шаблон.</p>
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
        <button class="btn-primary btn-sm summary-model-download" disabled={!summaryState.model} onclick={downloadSummaryModel}>
          Скачать
        </button>
      {/if}
    </div>
  {/if}

  <div class="summary-actions">
    {#if primaryPreset && !modelMissing}
      <button
        class="btn-primary"
        disabled={!canGenerate}
        title={!canGenerate && blockingText ? blockingText : undefined}
        onclick={() => void pickPreset(primaryPreset.id)}
      >
        Собрать по шаблону «{primaryPreset.label}»
      </button>
      <button
        type="button"
        class="btn-secondary"
        onclick={(e) => onopenpickerat(e)}
      >
        Сменить шаблон
      </button>
    {:else if !modelMissing}
      <button class="btn-primary" onclick={(e) => onopenpickerat(e)}>Выбрать шаблон</button>
    {/if}
  </div>

  {#if classification && !manualOnly}
    <details class="summary-why">
      <summary>
        <span>Почему так решили</span>
        <svg class="summary-why-chevron" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="m6 9 6 6 6-6"></path>
        </svg>
      </summary>
      <p class="summary-why-text">
        <span>{recommendation}</span>
        <span class="summary-why-signals">
          {summaryConfidenceText(classification.confidence)}:
          {signals.length ? signals.join(', ') : 'сигналов недостаточно'}
        </span>
      </p>
    </details>
  {/if}

  <p class="summary-privacy">
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="4" y="10" width="16" height="10" rx="2"></rect>
      <path d="M8 10V7a4 4 0 0 1 8 0v3"></path>
    </svg>
    <span>Анализ идёт на этом компьютере. Текст никуда не отправляется.</span>
  </p>
</div>

<style>
  .summary-suggest {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-width: 0;
  }

  .summary-callout {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border: 1px solid #e8d6a8;
    border-radius: var(--radius-inner);
    background: var(--accent-amber-bg);
    color: var(--accent-amber);
    font-size: 12px;
    line-height: 1.45;
  }
  .summary-callout-text {
    font-size: 12px;
    line-height: 1.45;
  }
  .summary-callout-text strong { font-weight: 700; }

  .summary-hint {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }

  .summary-model {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner);
    background: var(--bg-inset);
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-primary);
  }
  .summary-model-title {
    font-size: 13px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .summary-model-download { align-self: flex-start; }

  .summary-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .summary-why {
    padding-top: 10px;
    border-top: 1px solid rgba(15, 30, 51, 0.06);
    font-size: 12px;
    color: var(--text-muted);
  }
  .summary-why summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: fit-content;
    list-style: none;
    color: var(--text-muted);
    font-weight: 600;
    cursor: pointer;
  }
  .summary-why summary::-webkit-details-marker { display: none; }
  .summary-why summary:hover { color: var(--accent-blue); }
  .summary-why-chevron { transition: transform 200ms var(--ease-spring); }
  .summary-why[open] .summary-why-chevron { transform: rotate(180deg); }
  .summary-why-text {
    margin: 8px 0 0;
    max-width: 70ch;
    font-size: 12px;
    line-height: 1.55;
    color: var(--text-muted);
  }
  .summary-why-signals {
    display: block;
    margin-top: 4px;
    color: var(--text-faint);
  }

  .summary-privacy {
    display: flex;
    align-items: flex-start;
    gap: 7px;
    margin: 0;
    padding-top: 10px;
    border-top: 1px solid rgba(15, 30, 51, 0.06);
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-faint);
  }
  .summary-privacy svg { flex-shrink: 0; margin-top: 1px; opacity: 0.8; }

  .summary-why summary:focus-visible,
  .summary-model-download:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }

  @media (max-width: 720px) {
    .summary-suggest { gap: 12px; }
    .summary-actions { flex-direction: column; align-items: stretch; }
    .summary-actions .btn-primary,
    .summary-actions .btn-secondary {
      width: 100%;
      min-height: 40px;
      justify-content: center;
    }
    .summary-model > .btn-primary { width: 100%; min-height: 40px; }
  }

  @media (prefers-reduced-motion: reduce) {
    .summary-why-chevron { transition: none; }
  }
</style>
