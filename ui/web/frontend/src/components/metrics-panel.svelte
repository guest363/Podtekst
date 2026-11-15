<script lang="ts">
import type { SpeechMetrics } from '../lib/api'
import { fmtClock, resetMetricsBaseline, speakerColor } from '../lib/api'
import { errText } from '../lib/guards.ts'
import { toast, toastConfirm } from '../lib/toast.svelte.ts'
import InfoTip from './info-tip.svelte'

type Props = {
  metrics: SpeechMetrics | null
  loading: boolean
  error: string
  onseek?: (sec: number) => void
  onretry?: () => void
  onreload?: () => void
}
let { metrics, loading, error, onseek, onretry, onreload }: Props = $props()

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
  const total = Math.round(sec)
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
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

const clippingText = (ratio: number): string => {
  const value = ratio * 100
  if (value < 0.1) return '<0,1 %'
  return `${value.toFixed(1).replace('.', ',')} %`
}

const good = $derived(metrics?.quality.grade === 'good')
const speakers = $derived(metrics?.speakers ?? [])
const warningCodes = $derived(metrics?.warnings ?? [])
const noAudio = $derived(warningCodes.includes('no_audio'))
const fewTurns = $derived(warningCodes.includes('few_turns'))
const monoOnly = $derived(warningCodes.includes('mono_no_overlap'))
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
// Гейт решает бэкенд: при плохом звуке L2-поля приходят null.
const prosodyUsable = $derived(speakers.some((speaker) => speaker.voiced_sec !== null))
const qualityReason = $derived.by(() => {
  if (noAudio) return 'звук не читается'
  if (qualityUnavailable) return 'не удалось оценить звук'
  if (warningCodes.includes('low_snr')) return 'много шума'
  if (warningCodes.includes('narrow_band')) return 'узкий диапазон частот'
  if (prosodyUnavailable) return 'не удалось выделить голос'
  return 'качество ниже порога'
})
const prosodyReason = $derived(`Высота голоса и громкость недоступны: ${qualityReason}.`)

const wpm = $derived.by(() => {
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
      if (group.speech > 0) {
        const groupWpm = Math.round((group.words / group.speech) * 60)
        line.push(`${groupWpm} ${pluralWords(groupWpm)} в минуту`)
      }
      if (group.interruptions > 0) {
        line.push(`перехваты: ${group.interruptions}`)
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
      value: `${fmtSpan(monologue.end - monologue.start)} · начало ${fmtClock(monologue.start)}`,
      tip: `${monologue.speaker}: непрерывная речь одного участника. Пауза до 1,5 секунды её не прерывает.`,
      seek: monologue.start,
    })
  }
  if (metrics.totals.monologue_sec_median !== null && metrics.totals.monologue_sec_p90 !== null) {
    rows.push({
      key: 'monologues',
      label: 'Типичный монолог',
      value: `медиана ${fmtSpan(metrics.totals.monologue_sec_median)} · p90 ${fmtSpan(metrics.totals.monologue_sec_p90)}`,
      tip: 'Медиана и 90-й перцентиль длительности непрерывных монологов.',
    })
  }
  rows.push({
    key: 'interruptions',
    label: 'Перехваты',
    value: monoOnly
      ? 'недоступно: запись одним каналом'
      : `${metrics.totals.interruptions}, из них ${metrics.totals.interruptions_successful} с быстрым ответом`,
    tip: 'Один начал говорить, пока другой не закончил. Быстрый ответ: предыдущий замолчал в течение 1 секунды. Считается только при раздельной записи микрофона и системного звука.',
  })
  if (metrics.navigation.first_interruption && !monoOnly) {
    rows.push({
      key: 'firstInterruption',
      label: 'Первый перехват',
      value: `в ${fmtClock(metrics.navigation.first_interruption.start)}`,
      tip: 'Перейти к первому месту, где участники заговорили одновременно.',
      seek: metrics.navigation.first_interruption.start,
    })
  }
  if (metrics.totals.switches_per_min !== null) {
    rows.push({
      key: 'switches',
      label: 'Смены говорящего',
      value: `${metrics.totals.switches_per_min.toFixed(1).replace('.', ',')} в минуту речи`,
      tip: 'Сколько раз менялся говорящий на минуту речи.',
    })
  }
  rows.push({
    key: 'overlap',
    label: 'Одновременная речь',
    value: monoOnly
      ? 'недоступно: запись одним каналом'
      : `${fmtSpan(metrics.totals.overlap_sec)} · ${pct((metrics.totals.overlap_share ?? 0) * 100)} речи`,
    tip: 'Сколько времени участники говорили одновременно. Доли участия выше считаются по речи каждого и могут пересекаться.',
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
      metrics.totals.dead_air_n === 0
        ? 'нет'
        : `${metrics.totals.dead_air_n}, всего ${fmtSpan(metrics.totals.dead_air_sec)}${
            metrics.totals.dead_air_max_sec === null
              ? ''
              : `, максимум ${fmtSpan(metrics.totals.dead_air_max_sec)}`
          }${
            metrics.totals.dead_air_share === null
              ? ''
              : ` · ${pct(metrics.totals.dead_air_share * 100)} записи`
          }`,
    tip: 'Паузы от 5 секунд: сколько их, сколько времени они заняли и самая длинная.',
    seek: metrics.navigation.first_long_pause?.start,
  })
  return rows
})

const hasBaseline = $derived(speakers.some((speaker) => speaker.baseline !== null))
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
      value: quality.clipping_ratio > 0.001 ? clippingText(quality.clipping_ratio) : 'нет',
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
        return 'Запись тихая или шумная: высота голоса и громкость не показываются.'
      if (code === 'clipping')
        return 'Громкие места звучат с искажением: микрофон не выдерживает громкость.'
      if (code === 'narrow_band')
        return 'Узкий диапазон звука: запись звучит как телефонный звонок.'
      return 'Микрофон и системный звук сильно различаются по громкости.'
    }),
)

const gradeText = $derived(
  good
    ? 'высоте голоса и громкости можно доверять'
    : clippingOnly
      ? 'есть перегрузка микрофона, значения могут быть неточными'
      : `высота голоса и громкость недоступны (${qualityReason})`,
)

let glossaryOpen = $state(false)
let qualityOpen = $state(false)
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

const glossary: Array<[string, string]> = [
  [
    'Доля речи',
    'Сколько времени из всей речи занял участник. Отметка на полосе показывает равную долю. Это не оценка вклада.',
  ],
  ['Типичная реплика', 'Половина реплик короче, половина длиннее.'],
  ['Монолог', 'Непрерывная речь одного участника. Пауза до 1,5 секунды её не прерывает.'],
  [
    'Перехват',
    'Один начал говорить, пока другой не закончил. Быстрый ответ: предыдущий замолчал в течение 1 секунды. Считается только при раздельной записи микрофона и системного звука.',
  ],
  [
    'Одновременная речь',
    'Время, когда разные участники говорили внахлёст. Доли участия считаются по речи каждого и могут пересекаться.',
  ],
  [
    'Пауза перед репликой',
    'Сколько времени проходит от конца одной реплики до начала реплики другого участника.',
  ],
  [
    'Долгие паузы',
    'Паузы от 5 секунд: сколько их, сколько времени они заняли и какая самая длинная.',
  ],
  ['Темп речи', 'Сколько слов участники говорят за минуту речи. Паузы между репликами не входят.'],
  ['Паузы внутри реплик', 'Какая часть времени внутри реплик приходится на молчание.'],
  ['Высота голоса', 'Сравнение с обычной высотой участника. В полутонах.'],
  ['Громкость', 'Сравнение с обычной громкостью участника. В децибелах.'],
  ['Чистота записи', 'На сколько децибел голос громче фонового шума.'],
  ['Диапазон частот', 'До какой частоты запись сохраняет звук. В килогерцах.'],
  ['Перегрузка микрофона', 'Доля звука с искажением. Так бывает при слишком громком звуке.'],
  [
    'Разница громкости микрофона и системного звука',
    'На сколько децибел они различаются. Большая разница мешает слушать запись.',
  ],
  [
    'Базовая линия',
    'Обычный уровень участника по прошлым встречам с тем же каналом. Нужно минимум 5 встреч; смена устройства может сбивать значения.',
  ],
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
      {error ? 'Не удалось посчитать метрики. Попробуйте открыть запись заново.' : 'Для этой записи метрики недоступны.'}
    </p>
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
      Качество записи:
      <InfoTip text="От качества записи зависит, можно ли доверять высоте голоса и громкости." />
      {gradeText}
    </p>
    <p class="metrics-muted">Метрики считаются на этом устройстве, аудио никуда не отправляется.</p>


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
        <InfoTip text="Обычный уровень участника по прошлым встречам с тем же каналом (микрофон или системный звук). Нужно минимум 5 встреч. Смена устройства может сбивать значения. Полутон - шаг высоты голоса, децибел - единица громкости." />
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
        <button
          type="button"
          class="metrics-reset"
          disabled={resetting}
          onclick={resetBaseline}
        >
          Сбросить базовую линию
        </button>
      {/if}
      {#if hasBaseline}
        <p class="metrics-muted">Профиль хранится только на этом устройстве.</p>
      {/if}
    </div>

    <div class="metrics-section">
      {#if qualityRows.length > 0}
        <div class="metrics-quality-details">
          <button
            type="button"
            class="metrics-glossary-toggle"
            aria-expanded={qualityOpen}
            aria-controls="metrics-quality-list"
            onclick={() => (qualityOpen = !qualityOpen)}
          >
            <span>Технические показатели</span>
            <span class="metrics-glossary-chevron {qualityOpen ? 'open' : ''}" aria-hidden="true">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </span>
          </button>
          {#if qualityOpen}
            <div class="metrics-quality-list" id="metrics-quality-list">
              {#each qualityRows as item (item.key)}
                {@render row(item)}
              {/each}
            </div>
          {/if}
        </div>
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
          файлом. Участие и ритм рассчитаны по расшифровке; высота голоса и громкость недоступны.
        </p>
      {/if}
    </div>

    <div class="metrics-glossary">
      <button
        type="button"
        class="metrics-glossary-toggle"
        aria-expanded={glossaryOpen}
        aria-controls="metrics-glossary"
        onclick={() => (glossaryOpen = !glossaryOpen)}
      >
        <span>Все определения</span>
        <span class="metrics-glossary-chevron {glossaryOpen ? 'open' : ''}" aria-hidden="true">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="6 9 12 15 18 9"></polyline>
          </svg>
        </span>
      </button>
      {#if glossaryOpen}
        <dl id="metrics-glossary">
          {#each glossary as [term, definition] (term)}
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

  .metrics-glossary {
    margin-top: 0;
    padding-top: 14px;
    border-top: 1px solid var(--border-soft);
    font-size: 11.5px;
    color: var(--text-muted);
  }

  .metrics-glossary-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: fit-content;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    font-size: 12px;
    font-weight: 700;
    color: var(--text-primary);
    cursor: pointer;
  }

  .metrics-glossary-toggle:hover {
    color: var(--accent-blue);
  }

  .metrics-glossary-toggle:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
  }

  .metrics-glossary-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    height: 12px;
    color: var(--text-faint);
    transition: transform 160ms var(--ease-spring);
    flex-shrink: 0;
  }

  .metrics-glossary-chevron.open {
    transform: rotate(180deg);
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

  @media (max-width: 1400px) {
    .metrics-glossary-toggle {
      min-height: 44px;
    }

    .metrics-value-button {
      min-height: 44px;
      padding: 10px 8px;
      border-radius: var(--radius-sm);
    }
  }
</style>
