<script lang="ts">
import { tick } from 'svelte'
import type { SummaryPreset } from '../lib/api'

interface Props {
  open: boolean
  presets: SummaryPreset[]
  recommendedId: string
  currentId: string
  anchor?: HTMLElement | null
  onpick: (id: string) => void
  onclose: () => void
}
let { open, presets, recommendedId, currentId, anchor = null, onpick, onclose }: Props = $props()

let dialog = $state<HTMLElement | null>(null)
let pos = $state({ top: 0, left: 0 })
let previous: HTMLElement | null = null

function place(): void {
  if (typeof window === 'undefined') return
  const a = anchor?.getBoundingClientRect()
  if (!a) {
    pos = { top: 72, left: Math.max(8, window.innerWidth - 368) }
    return
  }
  const width = 360
  const height = 440
  const left = Math.min(Math.max(8, a.left), Math.max(8, window.innerWidth - width - 8))
  let top = a.bottom + 8
  if (top + height > window.innerHeight - 8) {
    top = Math.max(8, a.top - height - 8)
  }
  pos = { top, left }
}

$effect(() => {
  if (!open) return
  previous = document.activeElement as HTMLElement | null
  place()
  void tick().then(() => {
    const current = dialog?.querySelector<HTMLElement>('[data-picker-current="true"]')
    const first = dialog?.querySelector<HTMLElement>('.summary-picker-item')
    ;(current || first || dialog)?.focus()
  })
  return () => previous?.focus?.()
})

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    onclose()
    return
  }
  if (e.key !== 'Tab' || !dialog) return
  const items = [...dialog.querySelectorAll<HTMLElement>('button:not(:disabled)')]
  if (items.length === 0) return
  const first = items[0]
  const last = items[items.length - 1]
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault()
    first.focus()
  }
}
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="summary-picker-backdrop" role="presentation" onclick={onclose}></div>
  <div
    class="summary-picker"
    role="dialog"
    aria-modal="true"
    aria-label="Шаблон итогов"
    tabindex="-1"
    bind:this={dialog}
    style={`top: ${pos.top}px; left: ${pos.left}px`}
    onkeydown={onKey}
  >
    <div class="summary-picker-head">
      <span class="summary-picker-title">Шаблон итогов</span>
      <button class="icon-btn" aria-label="Закрыть выбор шаблона" onclick={onclose}>✕</button>
    </div>
    <p class="help-text">Выберите шаблон. Для чувствительных тем нужно подтверждение.</p>
    <div class="summary-picker-list">
      {#each presets as p (p.id)}
        <button
          type="button"
          class="summary-picker-item"
          data-picker-current={currentId === p.id ? 'true' : 'false'}
          aria-current={currentId === p.id ? 'true' : undefined}
          onclick={() => onpick(p.id)}
        >
          <div class="summary-picker-top">
            <span class="summary-picker-name">{p.label}</span>
            <span class="summary-picker-badges">
              {#if recommendedId === p.id}
                <span class="summary-picker-badge rec">Рекомендуем</span>
              {/if}
              {#if currentId === p.id}
                <span class="summary-picker-badge current">Текущий</span>
              {/if}
              {#if p.sensitive}
                <span class="summary-picker-badge draft">Черновик</span>
              {/if}
            </span>
          </div>
          {#if p.short}
            <span class="summary-picker-short">{p.short}</span>
          {/if}
        </button>
      {/each}
      {#if presets.length === 0}
        <div class="empty-note">Шаблоны недоступны.</div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .summary-picker-backdrop {
    position: fixed;
    inset: 0;
    z-index: 69;
    background: rgba(15, 30, 51, 0.18);
  }
  .summary-picker {
    position: fixed;
    z-index: 70;
    width: 360px;
    max-width: calc(100vw - 16px);
    max-height: min(480px, calc(100dvh - 24px));
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: #fff;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-card);
    box-shadow: 0 12px 36px rgba(15, 30, 51, 0.18);
    padding: 14px;
    overflow: hidden;
  }
  .summary-picker:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-picker-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .summary-picker-title {
    font-size: 14px;
    font-weight: 800;
    color: var(--text-primary);
  }
  .summary-picker-list {
    display: flex;
    flex-direction: column;
    gap: 5px;
    overflow-y: auto;
    min-height: 0;
    padding-right: 4px;
    scrollbar-width: thin;
  }
  .summary-picker-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    text-align: left;
    border: 1px solid transparent;
    border-radius: var(--radius-inner);
    background: transparent;
    padding: 8px 10px;
    font: inherit;
    cursor: pointer;
    color: var(--text-primary);
    box-sizing: border-box;
    transition: background 120ms ease, border-color 120ms ease;
  }
  .summary-picker-item:hover,
  .summary-picker-item[data-picker-current='true'] {
    background: var(--bg-inset);
    border-color: var(--border-card);
  }
  .summary-picker-item:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-picker-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }
  .summary-picker-name {
    font-size: 13.5px;
    font-weight: 700;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .summary-picker-badges {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }
  .summary-picker-badge {
    font-size: 11px;
    font-weight: 700;
    border-radius: var(--radius-pill);
    padding: 1px 6px;
    border: 1px solid currentColor;
    white-space: nowrap;
    line-height: 1.3;
  }
  .summary-picker-badge.rec {
    color: var(--accent-ok);
    background: rgba(14, 122, 85, 0.08);
    border-color: rgba(14, 122, 85, 0.25);
  }
  .summary-picker-badge.current {
    color: var(--accent-blue);
    background: rgba(36, 86, 198, 0.08);
    border-color: rgba(36, 86, 198, 0.25);
  }
  .summary-picker-badge.draft {
    color: var(--accent-amber);
    background: rgba(138, 90, 0, 0.08);
    border-color: rgba(138, 90, 0, 0.25);
  }
  .summary-picker-short {
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-muted);
  }
  @media (max-width: 720px) {
    .summary-picker-backdrop {
      z-index: 80;
      background: rgba(15, 30, 51, 0.45);
      backdrop-filter: blur(4px);
    }
    .summary-picker {
      z-index: 81;
      left: 0 !important;
      right: 0;
      top: auto !important;
      bottom: 0;
      width: auto;
      max-height: 72dvh;
      border-radius: 16px 16px 0 0;
      padding: 14px 14px max(14px, env(safe-area-inset-bottom));
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .summary-picker,
    .summary-picker-item {
      transition: none;
    }
  }
</style>
