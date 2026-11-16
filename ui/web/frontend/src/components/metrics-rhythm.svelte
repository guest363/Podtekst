<script lang="ts">
import type { MetricsRow } from '../lib/metrics-model.ts'
import MetricsLine from './metrics-line.svelte'

type Props = {
  rows: MetricsRow[]
  fewTurns: boolean
  onseek?: (sec: number) => void
}
let { rows, fewTurns, onseek }: Props = $props()
</script>

<div class="metrics-section">
  <h3 class="metrics-heading">Ритм встречи</h3>
  {#each rows as item (item.key)}
    <MetricsLine {item} {onseek} />
  {/each}
  {#if fewTurns}
    <p class="metrics-muted">Мало реплик: цифры приблизительны.</p>
  {/if}
</div>

<style>
  .metrics-section {
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
  }

  .metrics-heading {
    margin: 0;
    font-size: 12.5px;
    font-weight: 800;
    letter-spacing: -0.01em;
    color: var(--text-primary);
  }

  .metrics-muted {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }
</style>
