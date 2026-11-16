<script lang="ts">
import type { MetricsParticipant } from '../lib/metrics-model.ts'

type Props = {
  participants: MetricsParticipant[]
}
let { participants }: Props = $props()

const evenShare = $derived(participants.length > 0 ? 100 / participants.length : 0)
</script>

<div class="metrics-section">
  <h3 class="metrics-heading">Участие</h3>
  <div class="metrics-participants">
    {#each participants as person (person.key)}
      <div class="participant" style={`--spk: ${person.color}`}>
        <div class="participant-line">
          <span class="participant-name" title={person.name}>{person.name}</span>
          <span class="participant-share">{person.sharePct} %</span>
        </div>
        <div class="participant-track">
          <span class="participant-fill" style={`width: ${person.sharePct}%`}></span>
          {#if participants.length > 1}
            <span class="participant-even" style={`left: ${evenShare}%`}></span>
          {/if}
        </div>
        <div class="participant-meta">{person.meta}</div>
      </div>
    {/each}
  </div>
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

  .metrics-participants {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .participant {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .participant-line {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
    min-width: 0;
  }

  .participant-name {
    font-size: 12.5px;
    font-weight: 700;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .participant-share {
    font-size: 12.5px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .participant-track {
    position: relative;
    height: 4px;
    border-radius: var(--radius-pill);
    background: rgba(15, 30, 51, 0.08);
    overflow: hidden;
  }

  .participant-fill {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--spk);
  }

  .participant-even {
    position: absolute;
    top: -1px;
    bottom: -1px;
    width: 1px;
    background: rgba(15, 30, 51, 0.28);
  }

  .participant-meta {
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }
</style>
