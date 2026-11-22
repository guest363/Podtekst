import type { Utterance } from './api'

type Cue = {
  start: number
  end: number
  speaker: string
  text: string
}

const pad = (value: number, width = 2): string => String(value).padStart(width, '0')

const stamp = (sec: number, comma: boolean): string => {
  const total = Math.round(sec * 1000)
  const ms = total % 1000
  const seconds = Math.floor(total / 1000)
  return `${pad(Math.floor(seconds / 3600))}:${pad(Math.floor(seconds / 60) % 60)}:${pad(seconds % 60)}${comma ? ',' : '.'}${pad(ms, 3)}`
}

const clean = (value: string | undefined): string =>
  String(value ?? '')
    .replace(/-->/g, '--')
    .replace(/\s+/g, ' ')
    .trim()

const escapeVtt = (value: string): string =>
  value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

const toCue = (utterance: Utterance): Cue | null => {
  const text = clean(utterance.text)
  if (!text) return null
  const start = Number.isFinite(utterance.start) ? Math.max(0, utterance.start) : 0
  const end =
    typeof utterance.end === 'number' && Number.isFinite(utterance.end)
      ? Math.max(start, utterance.end)
      : start
  return { start, end, speaker: clean(utterance.speaker), text }
}

const cues = (utterances: Utterance[]): Cue[] =>
  utterances.map(toCue).filter((cue): cue is Cue => cue !== null)

export const buildSrt = (utterances: Utterance[]): string => {
  const blocks = cues(utterances).map((cue, index) => {
    const speaker = cue.speaker || 'Спикер'
    return `${index + 1}\n${stamp(cue.start, true)} --> ${stamp(cue.end, true)}\n${speaker}: ${cue.text}`
  })
  return blocks.length > 0 ? `${blocks.join('\n\n')}\n` : ''
}

export const buildVtt = (utterances: Utterance[]): string => {
  const blocks = cues(utterances).map((cue) => {
    const speaker = escapeVtt(cue.speaker || 'Спикер')
    return `${stamp(cue.start, false)} --> ${stamp(cue.end, false)}\n<v ${speaker}>${escapeVtt(cue.text)}`
  })
  return blocks.length > 0 ? `WEBVTT\n\n${blocks.join('\n\n')}\n` : 'WEBVTT\n'
}
