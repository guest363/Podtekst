import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const source = readFileSync(join(root, 'src/lib/subtitles.ts'), 'utf8')
const require = createRequire(import.meta.url)
const ts = require('typescript')
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  fileName: 'subtitles.ts',
})
const moduleUrl = `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`
const { buildSrt, buildVtt } = await import(moduleUrl)

assert.equal(buildSrt([]), '')
assert.equal(buildVtt([]), 'WEBVTT\n')

const srt = buildSrt([
  { start: 0, end: 2.5, speaker: 'Спикер 1', text: 'строка 1' },
  { start: 3, end: 4.004, speaker: 'Спикер 2', text: 'строка 2' },
])
assert.equal(
  srt,
  '1\n00:00:00,000 --> 00:00:02,500\nСпикер 1: строка 1\n\n2\n00:00:03,000 --> 00:00:04,004\nСпикер 2: строка 2\n',
)
assert.ok(srt.startsWith('1\n'))
assert.ok(!srt.startsWith('\uFEFF'))

const vtt = buildVtt([
  { start: 0, end: 2.5, speaker: 'Спикер 1', text: 'строка 1' },
  { start: 3, end: 4.004, speaker: 'Спикер 2', text: 'a & b < c > d' },
])
assert.equal(
  vtt,
  'WEBVTT\n\n00:00:00.000 --> 00:00:02.500\n<v Спикер 1>строка 1\n\n00:00:03.000 --> 00:00:04.004\n<v Спикер 2>a &amp; b &lt; c &gt; d\n',
)
assert.ok(vtt.startsWith('WEBVTT'))
assert.equal(
  buildVtt([{ start: 0, end: 1, speaker: 'Спикер 1', text: 'строка 1\r\n\r\nстрока 2' }]),
  'WEBVTT\n\n00:00:00.000 --> 00:00:01.000\n<v Спикер 1>строка 1 строка 2\n',
)

assert.equal(
  buildSrt([{ start: 3723.004, end: 3724, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n01:02:03,004 --> 01:02:04,000\nСпикер 1: строка 1\n',
)
assert.equal(
  buildSrt([{ start: 59.9996, end: 60.5, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n00:01:00,000 --> 00:01:00,500\nСпикер 1: строка 1\n',
)
assert.equal(
  buildSrt([{ start: -4, end: 1, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n00:00:00,000 --> 00:00:01,000\nСпикер 1: строка 1\n',
)
assert.equal(
  buildSrt([{ start: 5, end: 4, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n00:00:05,000 --> 00:00:05,000\nСпикер 1: строка 1\n',
)
assert.equal(
  buildSrt([{ start: 5, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n00:00:05,000 --> 00:00:05,000\nСпикер 1: строка 1\n',
)
assert.equal(
  buildSrt([{ start: Number.NaN, end: Number.NaN, speaker: 'Спикер 1', text: 'строка 1' }]),
  '1\n00:00:00,000 --> 00:00:00,000\nСпикер 1: строка 1\n',
)

const filtered = buildSrt([
  { start: 0, end: 1, speaker: 'Спикер 1', text: '  ' },
  { start: 1, end: 2, speaker: 'Спикер 1', text: 'строка 1' },
  { start: 2, end: 3, text: '\n\n' },
  { start: 3, end: 4, text: 'строка 2' },
])
assert.equal(
  filtered,
  '1\n00:00:01,000 --> 00:00:02,000\nСпикер 1: строка 1\n\n2\n00:00:03,000 --> 00:00:04,000\nСпикер: строка 2\n',
)

const cleaned = buildSrt([
  { start: 0, end: 1, speaker: 'Спикер --> 1', text: 'строка 1\r\n\r\nстрока 2 --> дальше' },
])
assert.equal(
  cleaned,
  '1\n00:00:00,000 --> 00:00:01,000\nСпикер -- 1: строка 1 строка 2 -- дальше\n',
)

console.log('subtitles: OK')
