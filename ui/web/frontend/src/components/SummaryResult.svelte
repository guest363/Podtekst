<script lang="ts">
  import { fmtClockShort } from '../lib/api'
  import type { SummaryRecord, SummaryRef } from '../lib/api'
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
    onseek: (sec: number) => void
    onregenerate: () => void
    onupdate: () => void
    onchange: () => void
    oncopy: () => void
    onexport: () => void
  }
  let {
    record,
    stale,
    sensitive = false,
    caution = '',
    regenerateDisabled = false,
    onseek,
    onregenerate,
    onupdate,
    onchange,
    oncopy,
    onexport,
  }: Props = $props()

  let warnings = $derived(summaryWarnings(record))

  function openRef(ref: SummaryRef): void {
    onseek(ref.start)
    const index = Number(ref.utterance_index)
    if (!Number.isFinite(index) || index < 0) return
    document.querySelector(`[data-utt="${index}"]`)?.scrollIntoView({
      block: 'center',
      behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
    })
  }
</script>

<div class="summary-result" data-summary-result>
  <div class="summary-result-actions">
    <button class="btn-secondary btn-sm" disabled={regenerateDisabled} onclick={onregenerate}>Собрать заново</button>
    <button class="btn-secondary btn-sm" onclick={onchange}>Сменить шаблон</button>
  </div>

  {#if stale}
    <div class="summary-stale">
      <span>Итоги собраны до правки расшифровки. Имена и формулировки могли измениться.</span>
      <button class="btn-secondary btn-sm" disabled={regenerateDisabled} onclick={onupdate}>Обновить итоги</button>
    </div>
  {/if}

  {#if warnings.length > 0}
    <div class="summary-warnings">
      {#each warnings as warning}
        <div class="summary-warning">{warning}</div>
      {/each}
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
                    — {item.task?.assignee || 'не назначен'}, {item.task?.deadline || 'срок не назван'}
                  </span>
                {/if}
                {#if item.quote && item.quote !== item.text}
                  <span class="summary-quote">«{item.quote}»</span>
                {/if}
              </span>
              {#if item.refs?.length}
                <span class="summary-refs">
                  {#each item.refs.slice(0, 3) as ref}
                    <button
                      type="button"
                      class="summary-ref"
                      aria-label={`Перейти к реплике ${fmtClockShort(ref.start)}`}
                      title="Перейти к реплике"
                      onclick={() => openRef(ref)}
                    >
                      {fmtClockShort(ref.start)}
                    </button>
                  {/each}
                </span>
              {/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/each}

  <div class="summary-meta">
    Модель: {record.model_title} · Шаблон: «{record.preset_label}» · {summaryDate(record.created_at)} · {summaryDuration(record.duration_ms)}
  </div>
  <div class="summary-result-actions">
    <button class="btn-secondary btn-sm" onclick={oncopy}>Копировать</button>
    <button class="btn-secondary btn-sm" onclick={onexport}>Скачать .md</button>
  </div>
  <p class="summary-disclaimer">
    Итоги собраны локальной моделью. {caution || 'Проверьте имена, сроки и решения по ссылкам на реплики.'}
  </p>
  {#if sensitive}
    <span class="summary-sensitive-note">Черновик: проверьте формулировки по исходным репликам.</span>
  {/if}
</div>

<style>
  .summary-result {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  .summary-result-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .summary-stale,
  .summary-warnings {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    background: var(--accent-amber-bg);
    border: 1px solid #e8d6a8;
    border-radius: var(--radius-inner);
    padding: 8px 10px;
    color: var(--accent-amber);
    font-size: 12px;
    line-height: 1.45;
  }
  .summary-warnings {
    flex-direction: column;
    align-items: flex-start;
  }
  .summary-warning::before {
    content: '· ';
    font-weight: 800;
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
  .summary-quote {
    display: block;
    color: var(--text-muted);
    font-style: italic;
  }
  .summary-refs {
    display: inline-flex;
    gap: 3px;
    flex-shrink: 0;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .summary-ref {
    border: 1px solid var(--border-card);
    background: #fff;
    color: var(--accent-blue);
    border-radius: var(--radius-pill);
    padding: 4px 7px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1;
    cursor: pointer;
    min-width: 40px;
    min-height: 28px;
    position: relative;
  }
  .summary-ref:hover {
    background: var(--bg-inset);
  }
  .summary-ref:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-meta {
    border-top: 1px solid var(--border-soft);
    padding-top: 8px;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-faint);
  }
  .summary-disclaimer {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .summary-sensitive-note {
    font-size: 11.5px;
    font-weight: 700;
    color: var(--accent-amber);
  }
  @media (max-width: 720px) {
    .summary-result-actions {
      flex-direction: column;
      align-items: stretch;
    }
    .summary-result-actions .btn-secondary {
      min-height: 40px;
    }
    .summary-item {
      flex-wrap: wrap;
    }
    .summary-refs {
      flex-basis: 100%;
      justify-content: flex-start;
      padding-left: 14px;
    }
    .summary-ref::after {
      content: '';
      position: absolute;
      inset: -6px -4px;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .summary-ref {
      transition: none;
    }
  }
</style>
