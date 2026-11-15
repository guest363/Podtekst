<script lang="ts">
import type { SpeechMetrics } from '../lib/api'
import { fmtClock, speakerColor } from '../lib/api'
import InfoTip from './info-tip.svelte'

type Props = {
  metrics: SpeechMetrics | null
  loading: boolean
  error: string
  onseek?: (sec: number) => void
}
let { metrics, loading, error, onseek }: Props = $props()

const pluralTurns = (n: number): string => {
  const rem10 = n % 10
  const rem100 = n % 100
  if (rem100 >= 11 && rem100 <= 19) return 'реплик'
  if (rem10 === 1) return 'реплика'
  if (rem10 >= 2 && rem10 <= 4) return 'реплики'
  return 'реплик'
}

const pluralWords = (n: number): string => {
  const rem10 = n % 10
  const rem100 = n % 100
  if (rem100 >= 11 && rem100 <= 19) return 'слов'
  if (rem10 === 1) return 'слово'
  if (rem10 >= 2 && rem10 <= 4) return 'слова'
  return 'слов'
}

const fmtSpan = (sec: number | null): string => {
  if (sec === null) return 'нет'
  if (sec < 60) return `${sec.toFixed(1).replace('.', ',')} с`
  const minutes = Math.floor(sec / 60)
  const seconds = Math.round(sec % 60)
  return `${minutes} мин ${seconds} с`
}

const fmtMs = (ms: number | null): string => {
  if (ms === null) return 'нет'
  const sec = ms / 1000
  if (sec < 0.1) return 'меньше 0,1 с'
  return `${sec.toFixed(1).replace('.', ',')} с`
}

const signedNumber = (value: number): string =>
  `${value > 0 ? '+' : ''}${value.toFixed(1).replace('.', ',')}`

const pct = (value: number): string => `${Math.round(value)} %`

const good = $derived(metrics?.quality.grade === 'good')
const speakers = $derived(metrics?.speakers ?? [])
const warningCodes = $derived(metrics?.warnings ?? [])
const noAudio = $derived(warningCodes.includes('no_audio'))
const fewTurns = $derived(warningCodes.includes('few_turns'))
const prosodyUnavailable = $derived(warningCodes.includes('prosody_unavailable'))
const qualityUnavailable = $derived(warningCodes.includes('quality_unavailable'))
const clippingOnly = $derived(
  warningCodes.includes('clipping') &&
    !warningCodes.some((code) =>
      ['low_snr', 'narrow_band', 'no_audio', 'prosody_unavailable', 'quality_unavailable'].includes(
        code,
      ),
    ) &&
    (metrics?.quality.snr_db ?? 0) >= 20 &&
    (metrics?.quality.bandwidth_hz ?? 0) >= 6400,
)
const prosodyUsable = $derived(good || clippingOnly)
const qualityReason = $derived.by(() => {
  if (noAudio) return 'звук не читается'
  if (prosodyUnavailable) return 'не удалось выделить голос'
  if (qualityUnavailable) return 'не удалось оценить звук'
  if (warningCodes.includes('low_snr')) return 'много шума'
  if (warningCodes.includes('narrow_band')) return 'узкий диапазон частот'
  return 'качество ниже порога'
})
const prosodyReason = $derived(`Темп и высота голоса недоступны: ${qualityReason}.`)

const wpm = $derived.by(() => {
  if (!prosodyUsable) return null
  let words = 0
  let speech = 0
  for (const speaker of speakers) {
    words += speaker.words
    speech += speaker.speech_sec
  }
  return speech > 0 && words > 0 ? Math.round((words / speech) * 60) : null
})

const pausePct = $derived.by(() => {
  if (!prosodyUsable) return null
  let weighted = 0
  let speech = 0
  for (const speaker of speakers) {
    if (speaker.pause_share === null) continue
    weighted += speaker.pause_share * speaker.speech_sec
    speech += speaker.speech_sec
  }
  return speech > 0 ? Math.round((weighted / speech) * 100) : null
})

type Group = {
  name: string
  share: number
  turns: number
  turnWeighted: number
  turnWeight: number
  maxMonologue: number | null
  interruptions: number
  suffered: number
  words: number
  speech: number
}

const grouped = $derived.by((): Group[] => {
  const map = new Map<string, Group>()
  for (const speaker of speakers) {
    const key = speaker.name.trim().toLowerCase() || speaker.id
    const group = map.get(key) ?? {
      name: speaker.name,
      share: 0,
      turns: 0,
      turnWeighted: 0,
      turnWeight: 0,
      maxMonologue: null,
      interruptions: 0,
      suffered: 0,
      words: 0,
      speech: 0,
    }
    group.share += speaker.share
    group.turns += speaker.turns
    if (speaker.turn_sec_median !== null) {
      group.turnWeighted += speaker.turn_sec_median * speaker.turns
      group.turnWeight += speaker.turns
    }
    if (speaker.monologue_max_sec !== null) {
      group.maxMonologue = Math.max(group.maxMonologue ?? 0, speaker.monologue_max_sec)
    }
    group.interruptions += speaker.interruptions_made
    group.suffered += speaker.interruptions_suffered
    group.words += speaker.words
    group.speech += speaker.speech_sec
    map.set(key, group)
  }
  return [...map.values()].sort((left, right) => right.share - left.share)
})

// Хвост диаризации: кластеры с долей меньше 0,5 % в панель не пускаем.
const participants = $derived.by(() =>
  grouped
    .filter((group) => group.share >= 0.005)
    .map((group) => {
      const line: string[] = [`${group.turns} ${pluralTurns(group.turns)}`]
      if (group.maxMonologue !== null) {
        line.push(`монолог ${fmtSpan(group.maxMonologue)}`)
      }
      if (prosodyUsable && group.speech > 0) {
        line.push(
          `${Math.round((group.words / group.speech) * 60)} ${pluralWords(Math.round((group.words / group.speech) * 60))} в минуту`,
        )
      }
      return {
        key: group.name,
        name: group.name,
        sharePct: Math.round(group.share * 100),
        color: speakerColor(group.name),
        meta: line.join(' · '),
      }
    }),
)

const evenShare = $derived(participants.length > 0 ? 100 / participants.length : 0)

type Row = { key: string; label: string; value: string; seek?: number; tip?: string }

const rhythmRows = $derived.by((): Row[] => {
  if (!metrics) return []
  const rows: Row[] = []
  if (metrics.totals.turn_sec_median !== null) {
    rows.push({
      key: 'turn',
      label: 'Типичная реплика',
      value: fmtSpan(metrics.totals.turn_sec_median),
      tip: 'Половина реплик короче, половина длиннее.',
    })
  }
  if (metrics.navigation.longest_monologue) {
    const monologue = metrics.navigation.longest_monologue
    rows.push({
      key: 'monologue',
      label: 'Самый длинный монолог',
      value: `${fmtSpan(monologue.end - monologue.start)} · с ${fmtClock(monologue.start)}`,
      tip: 'Непрерывная речь одного участника. Пауза до 1,5 секунды её не прерывает.',
      seek: monologue.start,
    })
  }
  rows.push({
    key: 'interruptions',
    label: 'Перебивания',
    value: `${metrics.totals.interruptions}, из них ${metrics.totals.interruptions_successful} удались`,
    tip: 'Один начал говорить, пока другой не закончил. Если первый сразу замолкал, перебивание удалось.',
  })
  if (metrics.totals.response_latency_ms_median !== null) {
    rows.push({
      key: 'latency',
      label: 'Пауза перед репликой',
      value: fmtMs(metrics.totals.response_latency_ms_median),
      tip: 'Сколько времени проходит от конца реплики до начала реплики другого участника.',
    })
  }
  rows.push({
    key: 'deadAir',
    label: 'Долгие паузы',
    value:
      metrics.navigation.pauses_over_5s.length === 0
        ? 'нет'
        : `${metrics.navigation.pauses_over_5s.length}, всего ${fmtSpan(metrics.totals.dead_air_sec)}`,
  })
  return rows
})

const baselineRows = $derived.by(() => {
  if (!prosodyUsable) return []
  const visible = new Set(participants.map((person) => person.name.trim().toLowerCase()))
  const seen = new Set<string>()
  return speakers
    .filter((speaker) => {
      const key = speaker.name.trim().toLowerCase()
      if (!speaker.baseline || !visible.has(key) || seen.has(key)) return false
      seen.add(key)
      return true
    })
    .map((speaker) => ({
      key: `baseline-${speaker.id}`,
      name: speaker.name,
      available: speaker.baseline?.available === true,
      meetings: speaker.baseline?.n_meetings ?? 0,
      f0: speaker.baseline?.f0_delta_st ?? null,
      energy: speaker.baseline?.energy_delta_db ?? null,
    }))
})

const qualityRows = $derived.by((): Row[] => {
  if (!metrics) return []
  const quality = metrics.quality
  const rows: Row[] = []
  if (quality.snr_db !== null) {
    rows.push({
      key: 'snr',
      label: 'Чистота записи',
      value: `${Math.round(quality.snr_db)} дБ`,
    })
  }
  if (quality.bandwidth_hz !== null) {
    rows.push({
      key: 'band',
      label: 'Диапазон частот',
      value: `${(quality.bandwidth_hz / 1000).toFixed(1).replace('.', ',')} кГц`,
    })
  }
  if (quality.clipping_ratio !== null) {
    rows.push({
      key: 'clip',
      label: 'Перегрузка микрофона',
      value: quality.clipping_ratio > 0.001 ? pct(quality.clipping_ratio * 100) : 'нет',
    })
  }
  if (quality.channel_balance_db !== null) {
    rows.push({
      key: 'balance',
      label: 'Разница громкости микрофона и системного звука',
      value: `${Math.abs(quality.channel_balance_db).toFixed(1).replace('.', ',')} дБ`,
    })
  }
  return rows
})

const qualityWarnings = $derived(
  (metrics?.warnings ?? [])
    .filter((code) => ['low_snr', 'clipping', 'narrow_band', 'channel_balance'].includes(code))
    .map((code) => {
      if (code === 'low_snr')
        return 'Запись тихая или шумная: темп и высота голоса могут быть неточными.'
      if (code === 'clipping')
        return 'Громкие места звучат с искажением: микрофон не выдерживает громкость.'
      if (code === 'narrow_band')
        return 'Узкий диапазон звука: запись звучит как телефонный звонок.'
      return 'Микрофон и системный звук сильно различаются по громкости.'
    }),
)

const gradeText = $derived(
  good
    ? 'темпу и высоте голоса можно доверять'
    : clippingOnly
      ? 'есть перегрузка микрофона, значения могут быть неточными'
      : `темп и высота голоса недоступны: ${qualityReason}`,
)

const statusText = $derived(
  loading
    ? 'Считаем метрики встречи'
    : metrics
      ? 'Метрики готовы'
      : error
        ? 'Не удалось посчитать метрики'
        : 'Метрики недоступны',
)

const glossary: Array<[string, string]> = [
  [
    'Доля речи',
    'Сколько времени из всей речи занял участник. Отметка на полосе показывает равную долю. Это не оценка вклада.',
  ],
  ['Типичная реплика', 'Половина реплик короче, половина длиннее.'],
  ['Монолог', 'Непрерывная речь одного участника. Пауза до 1,5 секунды её не прерывает.'],
  [
    'Перебивание',
    'Один начал говорить, пока другой не закончил. Если первый сразу замолкал, перебивание удалось.',
  ],
  [
    'Пауза перед репликой',
    'Сколько времени проходит от конца одной реплики до начала реплики другого участника.',
  ],
  ['Долгие паузы', 'Паузы от 5 секунд: сколько их и сколько времени они заняли.'],
  ['Темп речи', 'Сколько слов участники говорят за минуту речи. Паузы между репликами не входят.'],
  ['Паузы внутри реплик', 'Какая часть времени внутри реплик приходится на молчание.'],
  ['Высота голоса', 'Сравнение с обычной высотой участника. В полутонах.'],
  ['Громкость', 'Сравнение с обычной громкостью участника. В децибелах.'],
  ['Чистота записи', 'На сколько децибел голос громче фонового шума.'],
  ['Диапазон частот', 'До какой частоты запись сохраняет звук. В килогерцах.'],
  [
    'Перегрузка микрофона',
    'Доля звука с искажением. Так бывает, когда микрофон не выдерживает громкость.',
  ],
  [
    'Разница громкости микрофона и системного звука',
    'На сколько децибел они различаются. Большая разница мешает слушать запись.',
  ],
  ['Базовая линия', 'Обычный уровень участника по прошлым встречам. Нужно минимум 5 встреч.'],
]
</script>

{#snippet row(item: Row)}
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
{/snippet}

<p class="sr-only" aria-live="polite">{statusText}</p>
<section class="metrics-panel" aria-busy={loading} tabindex="-1">
  {#if loading}
    <p class="metrics-loading">Считаем метрики встречи.</p>
  {:else if !metrics}
    <p class="metrics-empty">
      {error ? 'Не удалось посчитать метрики. Откройте запись заново.' : 'Для этой записи метрики недоступны.'}
    </p>
  {:else}
    <div class="metrics-stats">
      <div class="metrics-stat">
        <span class="metrics-stat-value">{fmtClock(metrics.speech_sec)}</span>
        <span class="metrics-stat-label">длительность речи</span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{metrics.totals.turns}</span>
        <span class="metrics-stat-label">{pluralTurns(metrics.totals.turns)}</span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{wpm ?? 'нет'}</span>
        <span class="metrics-stat-label">
          слов в минуту
          <InfoTip text="Сколько слов участники говорят за минуту речи. Паузы между репликами не входят." />
        </span>
      </div>
      <div class="metrics-stat">
        <span class="metrics-stat-value">{pausePct === null ? 'нет' : `${pausePct} %`}</span>
        <span class="metrics-stat-label">
          паузы внутри реплик
          <InfoTip text="Какая часть времени внутри реплик приходится на молчание." />
        </span>
      </div>
    </div>

    <p class="metrics-quality-line" class:is-bad={!good}>
      Качество записи<InfoTip text="От качества записи зависит, можно ли доверять темпу и высоте голоса." />:
      {gradeText}
    </p>


    {#if speakers.length > 0}
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
    {/if}

    <div class="metrics-section">
      <h3 class="metrics-heading">Ритм встречи</h3>
      {#each rhythmRows as item (item.key)}
        {@render row(item)}
      {/each}
      {#if fewTurns}
        <p class="metrics-muted">Мало реплик: цифры приблизительны.</p>
      {/if}
    </div>

    <div class="metrics-section">
      <h3 class="metrics-heading">
        Отклонение от обычного уровня
        <InfoTip text="Обычный уровень участника по прошлым встречам. Нужно минимум 5 встреч. Полутон - шаг высоты голоса, децибел - единица громкости." />
      </h3>
      {#if !prosodyUsable}
        <p class="metrics-muted">{prosodyReason}</p>
      {:else if baselineRows.length > 0}
        <div class="metrics-baseline">
          <div class="metrics-baseline-head">
            <span>Участник</span>
            <span>Высота, полутона</span>
            <span>Громкость, дБ</span>
          </div>
          {#each baselineRows as person (person.key)}
            <div class="metrics-baseline-row">
              <span class="metrics-baseline-name" title={person.name}>{person.name}</span>
              <span
                class="metrics-baseline-value"
                title={person.available ? '' : `Нужно 5 встреч, сейчас ${person.meetings}.`}
              >
                {person.available && person.f0 !== null ? signedNumber(person.f0) : 'нет'}
              </span>
              <span
                class="metrics-baseline-value"
                title={person.available ? '' : `Нужно 5 встреч, сейчас ${person.meetings}.`}
              >
                {person.available && person.energy !== null ? signedNumber(person.energy) : 'нет'}
              </span>
            </div>
          {/each}
        </div>
      {:else}
        <p class="metrics-muted">Сравнение появится после 5 встреч с этим участником.</p>
      {/if}
    </div>

    <div class="metrics-section">
      {#if qualityRows.length > 0}
        <details class="metrics-quality-details">
          <summary>Технические показатели</summary>
          <div class="metrics-quality-list">
            {#each qualityRows as item (item.key)}
              {@render row(item)}
            {/each}
          </div>
        </details>
      {/if}
      {#if qualityWarnings.length > 0}
        <ul class="metrics-warnings" role="list">
          {#each qualityWarnings as warning (warning)}
            <li>{warning}</li>
          {/each}
        </ul>
      {/if}
      {#if noAudio}
        <p class="metrics-muted">
          Звук этой записи не подходит для анализа. Например, так бывает с импортированным
          файлом. Участие и ритм рассчитаны по расшифровке. Темп и высота голоса недоступны.
        </p>
      {/if}
    </div>

    <details class="metrics-glossary">
      <summary>Все определения</summary>
      <dl>
        {#each glossary as [term, definition] (term)}
          <div>
            <dt>{term}</dt>
            <dd>{definition}</dd>
          </div>
        {/each}
      </dl>
    </details>

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
    gap: 8px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-right: 2px;
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

  .metrics-stats {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px 16px;
    padding-bottom: 0;
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
    margin: -6px 0 0;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .metrics-quality-line.is-bad {
    color: var(--accent-amber);
  }

  .metrics-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 8px;
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
    gap: 9px;
  }

  .participant {
    display: flex;
    flex-direction: column;
    gap: 3px;
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
    line-height: 1.4;
    color: var(--text-muted);
  }

  .metrics-item {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .metrics-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    min-width: 0;
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

  .metrics-muted {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--text-muted);
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

  .metrics-quality-details summary {
    cursor: pointer;
    font-weight: 700;
    color: var(--text-primary);
  }

  .metrics-quality-details summary:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
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

  .metrics-glossary {
    margin-top: -2px;
    font-size: 11.5px;
    color: var(--text-muted);
  }

  .metrics-glossary summary {
    cursor: pointer;
    font-weight: 700;
    color: var(--text-primary);
  }

  .metrics-glossary summary:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
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

  @media (max-width: 1200px) {
    .metrics-value-button {
      min-height: 44px;
      padding: 10px 8px;
      border-radius: var(--radius-sm);
    }
  }
</style>
