<script lang="ts">
import type { SpeechMetrics } from '../lib/api'
import { fmtClock, pluralUtterances, resetMetricsBaseline } from '../lib/api'
import { errText } from '../lib/guards.ts'
import { metricsGlossary, metricsTips } from '../lib/metrics-copy.ts'
import {
  baselineRows,
  gradeText,
  groupSpeakers,
  hasBaseline,
  metricsWpm,
  participantsFrom,
  pausePct,
  prosodyReason,
  prosodyUsable,
  qualityReason,
  qualityRows,
  qualityWarnings,
  rhythmRows,
} from '../lib/metrics-model.ts'
import { toast, toastConfirm } from '../lib/toast.svelte.ts'
import InfoTip from './info-tip.svelte'
import MetricsParticipation from './metrics-participation.svelte'
import MetricsQuality from './metrics-quality.svelte'
import MetricsRhythm from './metrics-rhythm.svelte'
import MetricsToggle from './metrics-toggle.svelte'

type Props = {
  metrics: SpeechMetrics | null
  loading: boolean
  error: string
  onseek?: (sec: number) => void
  onretry?: () => void
  onreload?: () => void
}
let { metrics, loading, error, onseek, onretry, onreload }: Props = $props()

const good = $derived(metrics?.quality.grade === 'good')
const speakers = $derived(metrics?.speakers ?? [])
const warningCodes = $derived(metrics?.warnings ?? [])
const noAudio = $derived(warningCodes.includes('no_audio'))
const fewTurns = $derived(warningCodes.includes('few_turns'))
const monoOnly = $derived(warningCodes.includes('mono_no_overlap'))
const qualityReasonText = $derived(qualityReason(warningCodes))
const usable = $derived(prosodyUsable(speakers))
const wpm = $derived(metricsWpm(speakers))
const pause = $derived(pausePct(speakers))
const grouped = $derived(groupSpeakers(speakers))
const participants = $derived(participantsFrom(grouped))

let glossaryOpen = $state(false)
let resetting = $state(false)

const doResetBaseline = async (): Promise<void> => {
  resetting = true
  try {
    await resetMetricsBaseline()
    toast('Базовая линия сброшена')
    onreload?.()
  } catch (e) {
    toast(`Не удалось сбросить базовую линию: ${errText(e)}`, 'error')
  } finally {
    resetting = false
  }
}

const resetBaseline = (): void => {
  toastConfirm('Сбросить личную базовую линию?', 'Сбросить', () => {
    void doResetBaseline()
  })
}

const statusText = $derived(
  loading
    ? 'Считаем метрики встречи'
    : metrics
      ? 'Метрики готовы'
      : error
        ? 'Не удалось посчитать метрики'
        : 'Метрики недоступны',
)
</script>

<p class="sr-only" aria-live="polite">{statusText}</p>
<section class="metrics-panel" aria-busy={loading} tabindex="-1">
  {#if loading}
    <p class="metrics-loading">Считаем метрики встречи.</p>
  {:else if !metrics}
    <p class="metrics-empty">
      {error ? 'Не удалось посчитать метрики. Попробуйте открыть запись заново.' : 'Для этой записи метрики недоступны.'}
    </p>
    {#if error}
      <p class="metrics-empty-sub">{error}</p>
    {/if}
    {#if error && onretry}
      <button type="button" class="btn-secondary btn-sm" onclick={onretry}>Повторить</button>
    {/if}
  {:else}
    <div class="metrics-stats">
      <div class="metrics-stat">
        <span class="metrics-stat-value">{fmtClock(metrics.speech_sec)}</span>
        <span class="metrics-stat-label">
          длительность речи{metrics.totals.density === null
            ? ''
            : ` · ${Math.round(metrics.totals.density * 100)} % записи`}
        </span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{metrics.totals.turns}</span>
        <span class="metrics-stat-label">{pluralUtterances(metrics.totals.turns)}</span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{wpm ?? 'нет'}</span>
        <span class="metrics-stat-label">
          слов в минуту
          <InfoTip text={metricsTips.wpm} />
        </span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{pause === null ? 'нет' : `${pause} %`}</span>
        <span class="metrics-stat-label">
          паузы внутри реплик
          <InfoTip text={metricsTips.pause} />
        </span>
      </div>
    </div>

    <p class="metrics-quality-line" class:is-bad={!good}>
      Качество записи:
      <InfoTip text={metricsTips.quality} />
      {gradeText(metrics)}
    </p>
    <p class="metrics-muted">Метрики считаются на этом устройстве, аудио никуда не отправляется.</p>

    {#if speakers.length > 0}
      <MetricsParticipation {participants} />
    {/if}

    <MetricsRhythm rows={rhythmRows(metrics, monoOnly)} {fewTurns} {onseek} />

    <MetricsQuality
      {usable}
      reason={prosodyReason(qualityReasonText)}
      baseline={baselineRows(speakers, participants, usable)}
      hasBaseline={hasBaseline(speakers)}
      {resetting}
      onreset={resetBaseline}
      rows={qualityRows(metrics)}
      warnings={qualityWarnings(warningCodes)}
      {noAudio}
      {onseek}
    />

    <div class="metrics-glossary">
      <MetricsToggle
        label="Все определения"
        open={glossaryOpen}
        controls="metrics-glossary"
        onclick={() => (glossaryOpen = !glossaryOpen)}
      />
      {#if glossaryOpen}
        <dl id="metrics-glossary">
          {#each metricsGlossary as [term, definition] (term)}
            <div>
              <dt>{term}</dt>
              <dd>{definition}</dd>
            </div>
          {/each}
        </dl>
      {/if}
    </div>
  {/if}
</section>

<style>
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  .metrics-panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-gutter: stable;
    padding-right: 10px;
    font-size: 12.5px;
    color: var(--text-primary);
  }

  .metrics-panel:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: -2px;
  }

  .metrics-loading,
  .metrics-empty {
    margin: 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }

  .metrics-empty-sub {
    margin: 4px 0 8px;
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--text-faint);
  }

  .metrics-stats {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px 16px;
    padding-bottom: 2px;
  }

  .metrics-stat {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .metrics-stat-value {
    font-size: 18px;
    font-weight: 800;
    letter-spacing: -0.02em;
    line-height: 1.15;
    font-variant-numeric: tabular-nums;
  }

  .metrics-stat-label {
    font-size: 11px;
    color: var(--text-muted);
  }

  .metrics-quality-line {
    margin: 0;
    font-size: 11.5px;
    font-weight: 600;
    line-height: 1.45;
    color: var(--text-primary);
  }

  .metrics-quality-line.is-bad {
    color: var(--accent-amber);
  }

  .metrics-muted {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
  }

  .metrics-glossary {
    margin-top: 0;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
    font-size: 11.5px;
    color: var(--text-muted);
  }

  .metrics-glossary dl {
    display: flex;
    flex-direction: column;
    gap: 7px;
    margin: 8px 0 0;
  }

  .metrics-glossary dt {
    font-weight: 700;
    color: var(--text-primary);
  }

  .metrics-glossary dd {
    margin: 1px 0 0;
    line-height: 1.45;
  }
</style>
