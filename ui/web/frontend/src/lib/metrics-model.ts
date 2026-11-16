import type { SpeakerMetrics, SpeechMetrics } from './api'
import { fmtClock, pluralUtterances, speakerColor } from './api'
import {
  metricsGrade,
  metricsQualityReason,
  metricsQualityWarning,
  metricsRowText,
  monologueTip,
} from './metrics-copy.ts'
import { clippingText, fmtMs, fmtSpan, pct, pluralWords } from './metrics-format.ts'

export type MetricsGroup = {
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

export const groupSpeakers = (speakers: SpeakerMetrics[]): MetricsGroup[] => {
  const map = new Map<string, MetricsGroup>()
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
}

export type MetricsParticipant = {
  key: string
  name: string
  sharePct: number
  color: string
  meta: string
}

// Хвост диаризации: кластеры с долей меньше 0,5 % в панель не пускаем.
export const participantsFrom = (groups: MetricsGroup[]): MetricsParticipant[] =>
  groups
    .filter((group) => group.share >= 0.005)
    .map((group) => {
      const line: string[] = [`${group.turns} ${pluralUtterances(group.turns)}`]
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
    })

export const metricsWpm = (speakers: SpeakerMetrics[]): number | null => {
  let words = 0
  let speech = 0
  for (const speaker of speakers) {
    words += speaker.words
    speech += speaker.speech_sec
  }
  return speech > 0 && words > 0 ? Math.round((words / speech) * 60) : null
}

// Гейт решает бэкенд: при плохом звуке L2-поля приходят null.
export const prosodyUsable = (speakers: SpeakerMetrics[]): boolean =>
  speakers.some((speaker) => speaker.voiced_sec !== null)

export const pausePct = (speakers: SpeakerMetrics[]): number | null => {
  if (!prosodyUsable(speakers)) return null
  let weighted = 0
  let speech = 0
  for (const speaker of speakers) {
    if (speaker.pause_share === null) continue
    weighted += speaker.pause_share * speaker.speech_sec
    speech += speaker.speech_sec
  }
  return speech > 0 ? Math.round((weighted / speech) * 100) : null
}

export type MetricsRow = { key: string; label: string; value: string; seek?: number; tip?: string }

export const rhythmRows = (metrics: SpeechMetrics | null, monoOnly: boolean): MetricsRow[] => {
  if (!metrics) return []
  const rows: MetricsRow[] = []
  if (metrics.totals.turn_sec_median !== null) {
    rows.push({
      key: 'turn',
      label: metricsRowText.turn.label,
      value: fmtSpan(metrics.totals.turn_sec_median),
      tip: metricsRowText.turn.tip,
    })
  }
  if (metrics.navigation.longest_monologue) {
    const monologue = metrics.navigation.longest_monologue
    rows.push({
      key: 'monologue',
      label: metricsRowText.monologue.label,
      value: `${fmtSpan(monologue.end - monologue.start)} · начало ${fmtClock(monologue.start)}`,
      tip: monologueTip(monologue.speaker),
      seek: monologue.start,
    })
  }
  if (metrics.totals.monologue_sec_median !== null && metrics.totals.monologue_sec_p90 !== null) {
    rows.push({
      key: 'monologues',
      label: metricsRowText.monologues.label,
      value: `медиана ${fmtSpan(metrics.totals.monologue_sec_median)} · p90 ${fmtSpan(metrics.totals.monologue_sec_p90)}`,
      tip: metricsRowText.monologues.tip,
    })
  }
  rows.push({
    key: 'interruptions',
    label: metricsRowText.interruptions.label,
    value: monoOnly
      ? 'недоступно: запись одним каналом'
      : `${metrics.totals.interruptions}, из них ${metrics.totals.interruptions_successful} с быстрым ответом`,
    tip: metricsRowText.interruptions.tip,
  })
  if (metrics.navigation.first_interruption && !monoOnly) {
    rows.push({
      key: 'firstInterruption',
      label: metricsRowText.firstInterruption.label,
      value: `в ${fmtClock(metrics.navigation.first_interruption.start)}`,
      tip: metricsRowText.firstInterruption.tip,
      seek: metrics.navigation.first_interruption.start,
    })
  }
  if (metrics.totals.switches_per_min !== null) {
    rows.push({
      key: 'switches',
      label: metricsRowText.switches.label,
      value: `${metrics.totals.switches_per_min.toFixed(1).replace('.', ',')} в минуту речи`,
      tip: metricsRowText.switches.tip,
    })
  }
  rows.push({
    key: 'overlap',
    label: metricsRowText.overlap.label,
    value: monoOnly
      ? 'недоступно: запись одним каналом'
      : `${fmtSpan(metrics.totals.overlap_sec)} · ${pct((metrics.totals.overlap_share ?? 0) * 100)} речи`,
    tip: metricsRowText.overlap.tip,
  })
  if (metrics.totals.response_latency_ms_median !== null) {
    rows.push({
      key: 'latency',
      label: metricsRowText.latency.label,
      value: fmtMs(metrics.totals.response_latency_ms_median),
      tip: metricsRowText.latency.tip,
    })
  }
  rows.push({
    key: 'deadAir',
    label: metricsRowText.deadAir.label,
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
    tip: metricsRowText.deadAir.tip,
    seek: metrics.navigation.first_long_pause?.start,
  })
  return rows
}

export type MetricsBaselineRow = {
  key: string
  name: string
  available: boolean
  meetings: number
  f0: number | null
  energy: number | null
}

export const hasBaseline = (speakers: SpeakerMetrics[]): boolean =>
  speakers.some((speaker) => speaker.baseline !== null)

export const baselineRows = (
  speakers: SpeakerMetrics[],
  participants: MetricsParticipant[],
  usable: boolean,
): MetricsBaselineRow[] => {
  if (!usable) return []
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
}

export const qualityRows = (metrics: SpeechMetrics | null): MetricsRow[] => {
  if (!metrics) return []
  const quality = metrics.quality
  const rows: MetricsRow[] = []
  if (quality.snr_db !== null) {
    rows.push({
      key: 'snr',
      label: metricsRowText.snr.label,
      value: `${Math.round(quality.snr_db)} дБ`,
    })
  }
  if (quality.bandwidth_hz !== null) {
    rows.push({
      key: 'band',
      label: metricsRowText.band.label,
      value: `${(quality.bandwidth_hz / 1000).toFixed(1).replace('.', ',')} кГц`,
    })
  }
  if (quality.clipping_ratio !== null) {
    rows.push({
      key: 'clip',
      label: metricsRowText.clip.label,
      value: quality.clipping_ratio > 0.001 ? clippingText(quality.clipping_ratio) : 'нет',
    })
  }
  if (quality.channel_balance_db !== null) {
    rows.push({
      key: 'balance',
      label: metricsRowText.balance.label,
      value: `${Math.abs(quality.channel_balance_db).toFixed(1).replace('.', ',')} дБ`,
    })
  }
  return rows
}

export const qualityWarnings = (codes: string[]): string[] =>
  codes
    .filter((code) => ['low_snr', 'clipping', 'narrow_band', 'channel_balance'].includes(code))
    .map((code) => metricsQualityWarning[code])

export const qualityReason = (codes: string[]): string => {
  if (codes.includes('no_audio')) return metricsQualityReason.noAudio
  if (codes.includes('quality_unavailable')) return metricsQualityReason.unavailable
  if (codes.includes('low_snr')) return metricsQualityReason.noisy
  if (codes.includes('narrow_band')) return metricsQualityReason.narrow
  if (codes.includes('prosody_unavailable')) return metricsQualityReason.noVoice
  return metricsQualityReason.below
}

export const clippingOnly = (metrics: SpeechMetrics | null): boolean => {
  const codes = metrics?.warnings ?? []
  return (
    codes.includes('clipping') &&
    !codes.some((code) =>
      ['low_snr', 'narrow_band', 'no_audio', 'prosody_unavailable', 'quality_unavailable'].includes(
        code,
      ),
    ) &&
    (metrics?.quality.snr_db ?? 0) >= 20 &&
    (metrics?.quality.bandwidth_hz ?? 0) >= 6400
  )
}

export const gradeText = (metrics: SpeechMetrics | null): string => {
  if (metrics?.quality.grade === 'good') return metricsGrade.good
  if (clippingOnly(metrics)) return metricsGrade.clipping
  return metricsGrade.unavailable(qualityReason(metrics?.warnings ?? []))
}

export const prosodyReason = (reason: string): string =>
  `Высота голоса и громкость недоступны: ${reason}.`
