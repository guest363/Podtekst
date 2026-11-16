<script lang="ts">
import type { MetricsRow } from '../lib/metrics-model.ts'
import InfoTip from './info-tip.svelte'

type Props = {
  item: MetricsRow
  onseek?: (sec: number) => void
}
let { item, onseek }: Props = $props()
</script>

<div class="metrics-item">
  <div class="metrics-row">
    <span class="metrics-label">
      {item.label}
      {#if item.tip}<InfoTip text={item.tip} />{/if}
    </span>
    {#if item.seek !== undefined && onseek}
      <button
        type="button"
        class="metrics-value metrics-value-button"
        title="Перейти к этому месту в записи"
        onclick={() => onseek?.(item.seek ?? 0)}
      >
        {item.value}
      </button>
    {:else}
      <span class="metrics-value">{item.value}</span>
    {/if}
  </div>
</div>

<style>
  .metrics-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .metrics-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    min-width: 0;
    line-height: 1.45;
  }

  .metrics-label {
    color: var(--text-muted);
    min-width: 0;
  }

  .metrics-value {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    text-align: right;
    flex-shrink: 0;
  }

  .metrics-value-button {
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    font-weight: 600;
    color: var(--accent-blue);
    cursor: pointer;
  }

  .metrics-value-button:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .metrics-value-button:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
  }

  @media (max-width: 1400px) {
    .metrics-value-button {
      min-height: 44px;
      padding: 10px 8px;
      border-radius: var(--radius-sm);
    }
  }
</style>
