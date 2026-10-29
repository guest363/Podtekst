import { fmtClockShort } from './api'
import type { Utterance } from './api'
import { toast } from './toast.svelte.ts'

export interface TranscriptMeta {
  title: string
  source: string
  durStr?: string
  speakers?: string[]
}

const encoder = new TextEncoder()
const CRC_TABLE = Uint32Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let i = 0; i < 8; i += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})

function crc32(data: Uint8Array): number {
  let crc = 0xffffffff
  for (const byte of data) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8)
  return (crc ^ 0xffffffff) >>> 0
}

function le(value: number, bytes: number): Uint8Array {
  return Uint8Array.from({ length: bytes }, (_, i) => (value >>> (i * 8)) & 0xff)
}

function join(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0))
  let offset = 0
  for (const part of parts) {
    out.set(part, offset)
    offset += part.length
  }
  return out
}

function zip(files: Array<{ name: string; data: Uint8Array }>): Uint8Array {
  const locals: Uint8Array[] = []
  const centrals: Uint8Array[] = []
  let offset = 0
  for (const file of files) {
    const name = encoder.encode(file.name)
    const crc = crc32(file.data)
    const local = join([
      le(0x04034b50, 4), le(20, 2), le(0x800, 2), le(0, 2), le(0, 2), le(0x21, 2),
      le(crc, 4), le(file.data.length, 4), le(file.data.length, 4), le(name.length, 2), le(0, 2),
      name, file.data,
    ])
    locals.push(local)
    centrals.push(join([
      le(0x02014b50, 4), le(20, 2), le(20, 2), le(0x800, 2), le(0, 2), le(0, 2), le(0x21, 2),
      le(crc, 4), le(file.data.length, 4), le(file.data.length, 4), le(name.length, 2),
      le(0, 2), le(0, 2), le(0, 2), le(0, 2), le(0, 4), le(offset, 4), name,
    ]))
    offset += local.length
  }
  const central = join(centrals)
  return join([
    ...locals,
    central,
    le(0x06054b50, 4), le(0, 2), le(0, 2), le(files.length, 2), le(files.length, 2),
    le(central.length, 4), le(offset, 4), le(0, 2),
  ])
}

function xml(value: string): string {
  return value.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' })[c] ?? c)
}

function runs(text: string, bold = false, color?: string): string {
  const bTag = bold ? '<w:b/>' : ''
  const cTag = color ? `<w:color w:val="${color}"/>` : ''
  const props = bTag || cTag ? `<w:rPr>${bTag}${cTag}</w:rPr>` : ''
  return `<w:r>${props}${text.split('\n').map((line, i) => `${i ? '<w:br/>' : ''}<w:t xml:space="preserve">${xml(line)}</w:t>`).join('')}</w:r>`
}

export function buildDocx(fileName: string, utterances: Utterance[], meta?: TranscriptMeta): Uint8Array {
  const title = meta?.title || fileName
  const source = meta?.source || fileName
  const dur = meta?.durStr ? ` (${meta.durStr})` : ''
  const speakers = meta?.speakers?.filter(Boolean) || []

  const headerParagraphs: string[] = [
    // Заголовок документа
    `<w:p><w:pPr><w:spacing w:after="160"/></w:pPr><w:r><w:rPr><w:b/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr><w:t xml:space="preserve">Расшифровка встречи: ${xml(title)}</w:t></w:r></w:p>`,
    // Источник
    `<w:p><w:pPr><w:spacing w:after="60"/></w:pPr>${runs('Источник: ', true, '333333')}${runs(`${source}${dur}`, false, '555555')}</w:p>`,
  ]

  if (speakers.length > 0) {
    headerParagraphs.push(
      `<w:p><w:pPr><w:spacing w:after="140"/></w:pPr>${runs('Спикеры: ', true, '333333')}${runs(speakers.join(', '), false, '555555')}</w:p>`,
    )
  }

  // Горизонтальная разделительная линия
  headerParagraphs.push(
    `<w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="6" w:space="8" w:color="D0D7DE"/></w:pBdr><w:spacing w:after="240"/></w:pPr></w:p>`,
  )

  const bodyParagraphs = utterances.flatMap((u) => [
    `<w:p><w:pPr><w:keepNext/><w:spacing w:before="140" w:after="40"/></w:pPr>${runs(`[${fmtClockShort(u.start)}] ${u.speaker || 'Спикер'}:`, true, '1F2328')}</w:p>`,
    `<w:p><w:pPr><w:spacing w:after="100"/></w:pPr>${runs(u.text || '', false, '24292F')}</w:p>`,
  ])

  const documentXml = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${headerParagraphs.join('')}${bodyParagraphs.join('')}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134"/></w:sectPr></w:body></w:document>`
  const contentTypes = `<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>`
  const rels = `<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>`
  return zip([
    { name: '[Content_Types].xml', data: encoder.encode(contentTypes) },
    { name: '_rels/.rels', data: encoder.encode(rels) },
    { name: 'word/document.xml', data: encoder.encode(documentXml) },
  ])
}

export async function exportDocx(
  fileName: string,
  utterances: Utterance[],
  meta?: TranscriptMeta,
): Promise<void> {
  const bytes = buildDocx(fileName, utterances, meta)
  const defaultName = `${fileName.replace(/\.[^.]+$/, '') || 'Расшифровка'}.docx`

  // 1. Системный диалог сохранения файла через бэкенд (zenity / kdialog / osascript / powershell)
  try {
    const res = await fetch(`/api/export/save_dialog?name=${encodeURIComponent(defaultName)}`, {
      method: 'POST',
      body: bytes.buffer as ArrayBuffer,
    })
    if (res.ok) {
      const data = (await res.json()) as {
        ok: boolean
        path?: string
        cancelled?: boolean
        no_dialog?: boolean
        error?: string
      }
      if (data.ok && data.path) {
        toast(`Файл сохранён: ${data.path}`)
        return
      }
      if (data.cancelled) {
        return
      }
      if (data.error) {
        toast(`Ошибка сохранения: ${data.error}`, 'error')
        return
      }
    }
  } catch {
    // В случае сбоя сети fallback ниже
  }

  // 2. В современных браузерах: File System Access API
  const win = window as unknown as {
    showSaveFilePicker?: (options: {
      suggestedName?: string
      types?: Array<{ description: string; accept: Record<string, string[]> }>
    }) => Promise<{
      createWritable: () => Promise<{
        write: (data: unknown) => Promise<void>
        close: () => Promise<void>
      }>
    }>
  }
  if (typeof win.showSaveFilePicker === 'function') {
    try {
      const handle = await win.showSaveFilePicker({
        suggestedName: defaultName,
        types: [
          {
            description: 'Word Document (*.docx)',
            accept: {
              'application/vnd.openxmlformats-officedocument.wordprocessingml.document': ['.docx'],
            },
          },
        ],
      })
      const writable = await handle.createWritable()
      await writable.write(bytes)
      await writable.close()
      toast(`Файл сохранён: ${defaultName}`)
      return
    } catch (err: unknown) {
      if ((err as Error)?.name === 'AbortError') {
        return
      }
    }
  }

  // 3. Fallback: классическое скачивание через <a>
  const blob = new Blob([bytes.buffer as ArrayBuffer], {
    type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = defaultName
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
  setTimeout(() => URL.revokeObjectURL(url), 1000)
  toast(`Файл скачан: ${defaultName}`)
}

export function printTranscript(): void {
  window.print()
}
