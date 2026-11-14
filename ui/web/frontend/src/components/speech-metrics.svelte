<script lang="ts">
import type { SpeechMetrics } from '../lib/api'
import { fmtClock } from '../lib/api'

type Props = {
  metrics: SpeechMetrics
  onseek?: (sec: number) => void
}
let { metrics, onseek }: Props = $props()

const QUALITY_WARNINGS: Record<string, string> = {
  low_snr: 'Тихий или шумный микрофон: просодия может быть неточной',
  clipping: 'Перегрузка микрофона',
  narrow_band: 'Узкая полоса канала',
  channel_balance: 'Разный уровень каналов',
}

const pluralTurns = (n: number): string => {
  const rem10 = n % 10
  const rem100 = n % 100
  if (rem100 >= 11 && rem100 <= 19) return 'реплик'
  if (rem10 === 1) return 'реплика'
  if (rem10 >= 2 && rem10 <= 4) return 'реплики'
  return 'реплик'
}

let turns = $derived(metrics.speakers.reduce((sum, s) => sum + s.turns, 0))
let good = $derived(metrics.quality.grade === 'good')
let warnings = $derived(metrics.warnings.filter((code) => code in QUALITY_WARNINGS))
let longest = $derived(metrics.navigation.longest_monologue)
let firstPause = $derived(metrics.navigation.first_long_pause)
let firstInterruption = $derived(metrics.navigation.first_interruption)

// Общий темп — сумма слов на сумму речи: иначе короткая реплика весит как доклад.
let wpm = $derived.by(() => {
  if (!good) return null
  let words = 0
  let speech = 0
  for (const s of metrics.speakers) {
    words += s.words
    speech += s.speech_sec
  }
  return speech > 0 && words > 0 ? Math.round((words / speech) * 60) : null
})

let pausePct = $derived.by(() => {
  if (!good) return null
  let weighted = 0
  let speech = 0
  for (const s of metrics.speakers) {
    if (s.pause_share === null) continue
    weighted += s.pause_share * s.speech_sec
    speech += s.speech_sec
  }
  return speech > 0 ? Math.round((weighted / speech) * 100) : null
})
</script>

{#snippet jump(label: string, start: number)}
  <button
    type="button"
    class="metrics-jump"
    title={`${label} (${fmtClock(start)})`}
    aria-label={`${label} (${fmtClock(start)})`}
    onclick={() => onseek?.(start)}
  >
    <span>{label}</span>
    <span class="metrics-jump-time">({fmtClock(start)})</span>
  </button>
{/snippet}

<section class="speech-metrics" aria-label="Метрики речи">
  <p class="metrics-summary">
    <span><span class="metrics-value">{fmtClock(metrics.speech_sec)}</span> речи</span>
    {#if turns > 0}
      <span><span class="metrics-value">{turns}</span> {pluralTurns(turns)}</span>
    {/if}
    {#if wpm !== null}
      <span>темп <span class="metrics-value">{wpm}</span> слов/мин</span>
    {/if}
    {#if pausePct !== null}
      <span>паузы <span class="metrics-value">{pausePct} %</span></span>
    {/if}
  </p>

  {#if onseek && (longest || firstPause || firstInterruption)}
    <div class="metrics-jumps">
      {#if longest}
        {@render jump('К самому длинному монологу', longest.start)}
      {/if}
      {#if firstPause}
        {@render jump('К первой паузе', firstPause.start)}
      {/if}
      {#if firstInterruption}
        {@render jump('К первому перехвату', firstInterruption.start)}
      {/if}
    </div>
  {/if}

  {#if !good && warnings.length > 0}
    <ul class="metrics-warnings">
      {#each warnings as code (code)}
        <li class="metrics-warning">{QUALITY_WARNINGS[code]}</li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .speech-metrics {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding-bottom: 9px;
    border-bottom: 1px solid var(--border-soft);
    flex-shrink: 0;
  }

  .metrics-summary {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 10px;
    row-gap: 2px;
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .metrics-value {
    color: var(--text-primary);
    font-weight: 700;
  }

  .metrics-jumps {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }

  .metrics-jump {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--accent-blue);
    text-align: left;
    cursor: pointer;
  }

  .metrics-jump:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .metrics-jump:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
  }

  .metrics-jump-time {
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .metrics-warnings {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .metrics-warning {
    position: relative;
    padding-left: 10px;
    font-size: 11px;
    line-height: 1.4;
    color: var(--accent-amber);
  }

  .metrics-warning::before {
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
