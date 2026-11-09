<script lang="ts">
  import type { SummaryRecord } from '../lib/api'
  import { renderInline } from '../lib/markdown'
  import {
    summaryDate,
    summaryDuration,
    summaryWarnings,
  } from '../lib/summaryUi.ts'

  interface Props {
    record: SummaryRecord
    stale: boolean
    sensitive?: boolean
    caution?: string
    regenerateDisabled?: boolean
    onregenerate: () => void
    onupdate: () => void
    onchange: (e: MouseEvent) => void
    oncopy: () => void
    onexport: () => void
  }
  let {
    record,
    stale,
    sensitive = false,
    caution = '',
    regenerateDisabled = false,
    onregenerate,
    onupdate,
    onchange,
    oncopy,
    onexport,
  }: Props = $props()

  let warnings = $derived(summaryWarnings(record))
  const english = $derived(
    record.output_language ? record.output_language === 'en' : record.language === 'en',
  )
  const noAssignee = $derived(english ? 'unassigned' : 'не назначен')
  const noDeadline = $derived(english ? 'deadline not stated' : 'срок не назван')
</script>

<div class="summary-result" data-summary-result>
  <div class="summary-subhead">
    <div class="summary-subhead-left">
      <button
        type="button"
        class="summary-preset-chip"
        onclick={(e) => onchange(e)}
        title="Нажмите, чтобы сменить шаблон"
      >
        <span>{record.preset_label || 'Шаблон'}</span>
        <span class="arrow">▾</span>
      </button>
      {#if sensitive}
        <span class="summary-badge draft">Черновик</span>
      {/if}
      {#if stale}
        <span class="summary-badge warn">Расшифровка изменена</span>
      {/if}
    </div>
    <div class="summary-subhead-right">
      <button
        type="button"
        class="btn-secondary btn-sm"
        onclick={oncopy}
        title="Копировать итоги в буфер"
      >
        Копировать
      </button>
    </div>
  </div>

  {#if stale}
    <div class="summary-stale">
      <span>Итоги собраны до правки расшифровки.</span>
      <button class="btn-secondary btn-sm" disabled={regenerateDisabled} onclick={onupdate}>Обновить итоги</button>
    </div>
  {/if}

  {#each record.sections as section (section.id)}
    {#if section.items.length > 0}
      <section class="summary-section">
        <h3 class="summary-heading" tabindex="-1">{section.title}</h3>
        <ul class="summary-items">
          {#each section.items as item, i (i)}
            <li class="summary-item">
              <span class="summary-text">
                {#each renderInline(item.text) as seg}
                  {#if seg.kind === 'bold'}
                    <strong>{seg.value}</strong>
                  {:else if seg.kind === 'code'}
                    <code>{seg.value}</code>
                  {:else}{seg.value}{/if}
                {/each}
                {#if item.role === 'task'}
                  <span class="summary-task">
                    — {item.task?.assignee || noAssignee}, {item.task?.deadline || noDeadline}
                  </span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/each}

  {#if warnings.length > 0}
    <details class="summary-warnings-details">
      <summary class="summary-warnings-summary">
        <span>Замечания к расшифровке ({warnings.length})</span>
        <span class="summary-warnings-hint">Развернуть ▾</span>
      </summary>
      <ul class="summary-warnings-list">
        {#each warnings as warning}
          <li>{warning}</li>
        {/each}
      </ul>
    </details>
  {/if}

  <div class="summary-meta">
    <div class="summary-meta-info">
      <span>{summaryDate(record.created_at)} · {summaryDuration(record.duration_ms)}</span>
      {#if record.model_title}
        <span class="summary-meta-model">· {record.model_title}</span>
      {/if}
    </div>
    <div class="summary-footer-actions">
      <button class="btn-secondary btn-sm" disabled={regenerateDisabled} onclick={onregenerate}>
        Собрать заново
      </button>
      <button class="btn-secondary btn-sm" onclick={onexport}>
        Скачать .md
      </button>
    </div>
  </div>

  <p class="summary-disclaimer">
    Итоги собраны локальной моделью. {caution || 'Проверьте имена, сроки и решения.'}
  </p>
</div>

<style>
  .summary-result {
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
  .summary-subhead-right {
    flex-shrink: 0;
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
  .summary-badge {
    font-size: 11px;
    font-weight: 600;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-pill);
    padding: 1px 7px;
    background: #fff;
    white-space: nowrap;
  }
  .summary-badge.warn,
  .summary-badge.draft {
    color: var(--accent-amber);
    border-color: #d9b45c;
  }
  .summary-stale {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    background: var(--accent-amber-bg);
    border: 1px solid #e8d6a8;
    border-radius: var(--radius-inner);
    padding: 8px 10px;
    color: var(--accent-amber);
    font-size: 12px;
    line-height: 1.45;
  }
  .summary-warnings-details {
    border: 1px solid var(--border-soft);
    background: var(--bg-inset);
    border-radius: var(--radius-inner);
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .summary-warnings-summary {
    display: flex;
    justify-content: space-between;
    align-items: center;
    cursor: pointer;
    font-weight: 600;
    color: var(--text-muted);
    list-style: none;
    user-select: none;
  }
  .summary-warnings-summary::-webkit-details-marker {
    display: none;
  }
  .summary-warnings-summary:hover {
    color: var(--text-primary);
  }
  .summary-warnings-hint {
    font-size: 11px;
    color: var(--text-faint);
    font-weight: 400;
  }
  .summary-warnings-details[open] .summary-warnings-hint {
    display: none;
  }
  .summary-warnings-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .summary-warnings-list li::before {
    content: '· ';
    font-weight: 700;
  }
  .summary-section {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .summary-heading {
    font-size: 12.5px;
    font-weight: 800;
    color: var(--text-primary);
    margin: 0;
  }
  .summary-heading:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-items {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 0;
    padding: 0;
  }
  .summary-item {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12.5px;
    line-height: 1.55;
    color: var(--text-primary);
    min-width: 0;
  }
  .summary-item::before {
    content: '•';
    color: var(--text-faint);
    flex-shrink: 0;
  }
  .summary-text {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .summary-task {
    color: var(--text-muted);
  }
  .summary-meta {
    border-top: 1px solid var(--border-soft);
    padding-top: 10px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .summary-meta-info {
    font-variant-numeric: tabular-nums;
  }
  .summary-meta-model {
    color: var(--text-muted);
  }
  .summary-footer-actions {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .summary-disclaimer {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  @media (max-width: 720px) {
    .summary-meta {
      flex-direction: column;
      align-items: stretch;
      gap: 8px;
    }
    .summary-footer-actions {
      flex-direction: column;
      align-items: stretch;
    }
    .summary-footer-actions .btn-secondary {
      min-height: 40px;
      justify-content: center;
    }
    .summary-item {
      flex-wrap: wrap;
    }
  }
</style>
