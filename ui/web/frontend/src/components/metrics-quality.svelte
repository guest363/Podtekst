<script lang="ts">
import { metricsTips } from '../lib/metrics-copy.ts'
import { signedNumber } from '../lib/metrics-format.ts'
import type { MetricsBaselineRow, MetricsRow } from '../lib/metrics-model.ts'
import InfoTip from './info-tip.svelte'
import MetricsLine from './metrics-line.svelte'
import MetricsToggle from './metrics-toggle.svelte'

type Props = {
  usable: boolean
  reason: string
  baseline: MetricsBaselineRow[]
  hasBaseline: boolean
  resetting: boolean
  onreset: () => void
  rows: MetricsRow[]
  warnings: string[]
  noAudio: boolean
  onseek?: (sec: number) => void
}
let {
  usable,
  reason,
  baseline,
  hasBaseline,
  resetting,
  onreset,
  rows,
  warnings,
  noAudio,
  onseek,
}: Props = $props()

let qualityOpen = $state(false)
</script>

<div class="metrics-section">
  <h3 class="metrics-heading">
    Отклонение от обычного уровня
    <InfoTip text={metricsTips.baseline} />
  </h3>
  {#if !usable}
    <p class="metrics-muted">{reason}</p>
  {:else if baseline.length > 0}
    <div class="metrics-baseline">
      <div class="metrics-baseline-head">
        <span>Участник</span>
        <span>Высота, полутона</span>
        <span>Громкость, дБ</span>
      </div>
      {#each baseline as person (person.key)}
        <div class="metrics-baseline-row">
          <span class="metrics-baseline-name" title={person.name}>{person.name}</span>
          <span
            class="metrics-baseline-value"
            title={person.available ? '' : `Нужно 5 прошлых встреч, сейчас ${person.meetings}.`}
          >
            {person.available && person.f0 !== null ? signedNumber(person.f0) : 'нет'}
          </span>
          <span
            class="metrics-baseline-value"
            title={person.available ? '' : `Нужно 5 прошлых встреч, сейчас ${person.meetings}.`}
          >
            {person.available && person.energy !== null ? signedNumber(person.energy) : 'нет'}
          </span>
        </div>
      {/each}
    </div>
  {:else}
    <p class="metrics-muted">Сравнение появится после 5 встреч с этим участником.</p>
  {/if}
  {#if hasBaseline}
    <button type="button" class="metrics-reset" disabled={resetting} onclick={onreset}>
      Сбросить базовую линию
    </button>
  {/if}
  {#if hasBaseline}
    <p class="metrics-muted">Профиль хранится только на этом устройстве.</p>
  {/if}
</div>

<div class="metrics-section">
  {#if rows.length > 0}
    <div class="metrics-quality-details">
      <MetricsToggle
        label="Технические показатели"
        open={qualityOpen}
        controls="metrics-quality-list"
        onclick={() => (qualityOpen = !qualityOpen)}
      />
      {#if qualityOpen}
        <div class="metrics-quality-list" id="metrics-quality-list">
          {#each rows as item (item.key)}
            <MetricsLine {item} {onseek} />
          {/each}
        </div>
      {/if}
    </div>
  {/if}
  {#if warnings.length > 0}
    <ul class="metrics-warnings" role="list">
      {#each warnings as warning (warning)}
        <li>{warning}</li>
      {/each}
    </ul>
  {/if}
  {#if noAudio}
    <p class="metrics-muted">
      Звук этой записи не подходит для анализа. Например, так бывает с импортированным
      файлом. Участие и ритм рассчитаны по расшифровке; высота голоса и громкость недоступны.
    </p>
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

  .metrics-reset {
    align-self: flex-start;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    font-size: 11.5px;
    color: var(--accent-blue);
    cursor: pointer;
  }

  .metrics-reset:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .metrics-reset:disabled {
    color: var(--text-faint);
    cursor: default;
  }

  .metrics-baseline {
    display: grid;
    grid-template-columns: minmax(90px, 1fr) auto auto;
    gap: 4px 12px;
    align-items: baseline;
  }

  .metrics-baseline-head {
    display: contents;
    font-size: 10.5px;
    color: var(--text-muted);
  }

  .metrics-baseline-head span {
    text-align: right;
  }

  .metrics-baseline-head span:first-child {
    text-align: left;
  }

  .metrics-baseline-row {
    display: contents;
    font-size: 11.5px;
  }

  .metrics-baseline-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .metrics-baseline-value {
    font-variant-numeric: tabular-nums;
    text-align: right;
    color: var(--text-muted);
  }

  .metrics-quality-details {
    font-size: 11.5px;
    color: var(--text-muted);
  }

  .metrics-quality-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 8px;
  }

  .metrics-warnings {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .metrics-warnings li {
    position: relative;
    padding-left: 10px;
    font-size: 11px;
    line-height: 1.4;
    color: var(--accent-amber);
  }

  .metrics-warnings li::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0.55em;
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--accent-amber);
  }
</style>
