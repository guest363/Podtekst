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
      <span class="summary-callout-icon" aria-hidden="true">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="9"></circle>
          <path d="M12 8v4.5"></path>
          <path d="M12 16h.01"></path>
        </svg>
      </span>
      <span class="summary-callout-text">{blockingText}</span>
    </div>
  {/if}

  {#if shortSource}
    <div class="summary-callout" role="status">
      <span class="summary-callout-icon" aria-hidden="true">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="9"></circle>
          <path d="M12 8v4.5"></path>
          <path d="M12 16h.01"></path>
        </svg>
      </span>
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
      <div class="summary-split">
        <button
          class="summary-cta-main"
          disabled={!canGenerate}
          onclick={() => void pickPreset(primaryPreset.id)}
        >
          Сделать по шаблону «{primaryPreset.label}»
        </button>
        <button
          type="button"
          class="summary-cta-caret"
          aria-label="Изменить шаблон"
          aria-haspopup="dialog"
          onclick={(e) => onopenpickerat(e)}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m6 9 6 6 6-6"></path>
          </svg>
        </button>
      </div>
    {:else if !modelMissing}
      <button class="summary-ghost" onclick={(e) => onopenpickerat(e)}>Выбрать шаблон</button>
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
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px 10px 10px;
    border: 1px solid rgba(138, 90, 0, 0.16);
    border-radius: 12px;
    background: linear-gradient(90deg, #fffaf0 0%, #fffdf8 100%);
    box-shadow: inset 3px 0 0 #d9a441;
    color: #6b4a00;
  }
  .summary-callout-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: 0 0 22px;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: #f6e6c4;
    color: #8a5a00;
  }
  .summary-callout-text {
    font-size: 12.5px;
    line-height: 1.55;
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
    gap: 9px;
    padding: 13px 14px;
    border: 1px solid rgba(15, 30, 51, 0.07);
    border-radius: 12px;
    background: linear-gradient(180deg, #fbfcfe 0%, #f8fafc 100%);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.9);
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
    gap: 10px;
    flex-wrap: wrap;
  }

  .summary-split {
    display: inline-flex;
    align-items: stretch;
    max-width: 100%;
    min-height: 38px;
    border: 1px solid #0f172a;
    border-radius: var(--radius-pill);
    background: linear-gradient(180deg, #1b2a44 0%, #0f1e33 100%);
    box-shadow:
      0 8px 20px -12px rgba(15, 30, 51, 0.55),
      inset 0 1px 0 rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .summary-cta-main {
    display: inline-flex;
    align-items: center;
    min-width: 0;
    padding: 6px 15px;
    border: 0;
    background: transparent;
    color: #fff;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    line-height: 1.35;
    text-align: left;
    cursor: pointer;
    transition: background 200ms var(--ease-spring), opacity 200ms var(--ease-spring);
  }
  .summary-cta-main:hover:not(:disabled) { background: rgba(255, 255, 255, 0.07); }
  .summary-cta-main:active:not(:disabled) { background: rgba(255, 255, 255, 0.12); }
  .summary-cta-main:disabled { opacity: 0.5; cursor: wait; }
  .summary-cta-caret {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    flex-shrink: 0;
    border: 0;
    border-left: 1px solid rgba(255, 255, 255, 0.16);
    background: transparent;
    color: #fff;
    cursor: pointer;
    transition: background 200ms var(--ease-spring);
  }
  .summary-cta-caret:hover { background: rgba(255, 255, 255, 0.08); }
  .summary-cta-caret:active { background: rgba(255, 255, 255, 0.14); }

  .summary-ghost {
    display: inline-flex;
    align-items: center;
    min-height: 38px;
    padding: 7px 14px;
    border: 1px solid rgba(15, 30, 51, 0.12);
    border-radius: var(--radius-pill);
    background: #fff;
    color: #334155;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 200ms var(--ease-spring),
      border-color 200ms var(--ease-spring),
      transform 200ms var(--ease-spring);
  }
  .summary-ghost:hover { background: #f8fafc; border-color: rgba(15, 30, 51, 0.2); }
  .summary-ghost:active { transform: scale(0.985); }

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

  .summary-cta-main:focus-visible,
  .summary-cta-caret:focus-visible,
  .summary-ghost:focus-visible,
  .summary-why summary:focus-visible,
  .summary-model-download:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }

  @media (max-width: 720px) {
    .summary-suggest { gap: 12px; }
    .summary-actions { flex-direction: column; align-items: stretch; }
    .summary-split, .summary-ghost { width: 100%; }
    .summary-cta-main { flex: 1; justify-content: flex-start; min-height: 42px; }
    .summary-cta-caret { width: 44px; }
    .summary-model > .btn-primary { width: 100%; min-height: 40px; }
  }

  @media (prefers-reduced-motion: reduce) {
    .summary-cta-main, .summary-cta-caret, .summary-ghost,
    .summary-why-chevron { transition: none; }
  }
</style>
