import { fmtClockShort } from './api'
import type { Utterance } from './api'

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

function runs(text: string, bold = false): string {
  const props = bold ? '<w:rPr><w:b/></w:rPr>' : ''
  return `<w:r>${props}${text.split('\n').map((line, i) => `${i ? '<w:br/>' : ''}<w:t xml:space="preserve">${xml(line)}</w:t>`).join('')}</w:r>`
}

export function buildDocx(fileName: string, utterances: Utterance[]): Uint8Array {
  const paragraphs = [
    `<w:p><w:pPr><w:spacing w:after="120"/></w:pPr>${runs(`Расшифровка: ${fileName}`, true)}</w:p>`,
    ...utterances.flatMap((u) => [
      `<w:p><w:pPr><w:keepNext/><w:spacing w:before="120" w:after="40"/></w:pPr>${runs(`[${fmtClockShort(u.start)}] ${u.speaker || 'Спикер'}`, true)}</w:p>`,
      `<w:p><w:pPr><w:spacing w:after="100"/></w:pPr>${runs(u.text || '')}</w:p>`,
    ]),
  ].join('')
  const documentXml = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${paragraphs}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134"/></w:sectPr></w:body></w:document>`
  const contentTypes = `<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>`
  const rels = `<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>`
  return zip([
    { name: '[Content_Types].xml', data: encoder.encode(contentTypes) },
    { name: '_rels/.rels', data: encoder.encode(rels) },
    { name: 'word/document.xml', data: encoder.encode(documentXml) },
  ])
}

export function exportDocx(fileName: string, utterances: Utterance[]): void {
  const bytes = buildDocx(fileName, utterances)
  const blob = new Blob([bytes.buffer as ArrayBuffer], { type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = `${fileName.replace(/\.[^.]+$/, '') || 'Расшифровка'}.docx`
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 0)
}

export function printTranscript(): void {
  window.print()
}
