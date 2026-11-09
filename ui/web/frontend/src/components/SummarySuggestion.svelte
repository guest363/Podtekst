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
  <div class="summary-subhead">
    <div class="summary-subhead-left">
      <button
        type="button"
        class="summary-preset-chip"
        onclick={(e) => onopenpickerat(e)}
        title="Нажмите, чтобы выбрать другой шаблон"
      >
        <span>{primaryPreset?.label || 'Шаблон'}</span>
        <span class="arrow">▾</span>
      </button>
    </div>
  </div>

  {#if blockingText}
    <div class="summary-callout" role="status">
      <span class="summary-callout-text">{blockingText}</span>
    </div>
  {:else if shortSource}
    <p class="summary-note">Запись короткая — формулировки могут быть краткими.</p>
  {/if}

  {#if manualOnly}
    <p class="summary-hint">
      Рекомендация отключена: выберите подходящий шаблон выше.
    </p>
  {:else if !classification && !summaryState.settings?.default_preset}
    <p class="summary-hint">Тип разговора не определён. Нажмите на шаблон выше, чтобы выбрать.</p>
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
    {:else if !modelMissing}
      <button class="btn-primary" onclick={(e) => onopenpickerat(e)}>Выбрать шаблон</button>
    {/if}
  </div>

  {#if classification && !manualOnly}
    <details class="summary-why">
      <summary>Почему так решили</summary>
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
    Анализ выполняется на этом компьютере. Текст никуда не отправляется.
  </p>
</div>

<style>
  .summary-suggest {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }
  .summary-subhead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-soft);
  }
  .summary-subhead-left {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .summary-preset-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: var(--radius-pill);
    padding: 3px 10px;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
    cursor: pointer;
    transition: all 140ms ease;
    line-height: 1.35;
  }
  .summary-preset-chip:hover {
    background: #fff;
    border-color: #cbd5e1;
    color: var(--accent-blue);
  }
  .summary-preset-chip .arrow {
    font-size: 10px;
    color: var(--text-muted);
  }
  .summary-preset-chip:hover .arrow {
    color: var(--accent-blue);
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

  .summary-note {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.45;
  }

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
    border-top: 1px solid var(--border-soft);
    font-size: 12px;
    color: var(--text-muted);
  }
  .summary-why summary {
    cursor: pointer;
    font-weight: 600;
    color: var(--text-muted);
    user-select: none;
  }
  .summary-why summary:hover {
    color: var(--text-primary);
  }
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
    margin: 0;
    padding-top: 10px;
    border-top: 1px solid var(--border-soft);
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-faint);
  }

  .summary-why summary:focus-visible,
  .summary-model-download:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }

  @media (max-width: 720px) {
    .summary-suggest { gap: 12px; }
    .summary-actions { flex-direction: column; align-items: stretch; }
    .summary-actions .btn-primary {
      width: 100%;
      min-height: 40px;
      justify-content: center;
    }
    .summary-model > .btn-primary { width: 100%; min-height: 40px; }
  }
</style>
