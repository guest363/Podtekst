import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const read = (name) => JSON.parse(readFileSync(join(root, 'src/lib/fixtures', name), 'utf8'))

function fail(message) {
  console.error(`summary fixtures: ${message}`)
  process.exit(1)
}

function keys(value, expected, where) {
  if (!value || typeof value !== 'object') fail(`${where}: not an object`)
  for (const key of expected) {
    if (!(key in value)) fail(`${where}: missing ${key}`)
  }
}

const artifact = read('summary-artifact.golden.json')
const record = read('summary-record.golden.json')

keys(
  artifact,
  [
    'schema',
    'schema_minor',
    'id',
    'generated_at',
    'source',
    'classification',
    'preset',
    'detail',
    'output_language',
    'generator',
    'summary',
    'provenance',
  ],
  'artifact',
)
keys(
  artifact.source,
  ['schema', 'rev', 'utterances', 'chars', 'language', 'duration_sec'],
  'artifact.source',
)
keys(
  artifact.generator,
  ['runtime', 'model_id', 'model_sha256', 'prompt_version', 'duration_ms', 'timings_ms'],
  'artifact.generator',
)
keys(
  artifact.summary,
  ['sections', 'warnings', 'truncated', 'coverage', 'items_total'],
  'artifact.summary',
)
keys(
  record,
  [
    'id',
    'job_id',
    'file',
    'preset_id',
    'preset_label',
    'model_id',
    'created_at',
    'duration_ms',
    'source_rev',
    'detail',
    'language',
    'output_language',
    'sections',
    'markdown',
    'truncated',
    'warnings',
    'schema',
    'schema_minor',
  ],
  'record',
)

if (record.id !== artifact.id || record.source_rev !== artifact.source.rev)
  fail('record does not project the artifact')
if (record.sections.length !== artifact.summary.sections.length) fail('section count differs')
for (const [index, section] of record.sections.entries()) {
  keys(section, ['id', 'title', 'role', 'items'], `record.sections[${index}]`)
  for (const [itemIndex, item] of section.items.entries()) {
    keys(item, ['text', 'role'], `record.sections[${index}].items[${itemIndex}]`)
  }
}
if (typeof record.markdown !== 'string' || !record.markdown.startsWith('#'))
  fail('record.markdown is not Markdown')
console.log(`summary fixtures: ok (${record.sections.length} sections)`)
