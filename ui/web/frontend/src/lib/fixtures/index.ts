import type { SummaryArtifact, SummaryRecord } from '../api'
import artifact from './summary-artifact.golden.json'
import record from './summary-record.golden.json'

// JSON-импорт выводит литеральные строки, поэтому для `satisfies` расширяем
// примитивы: проверяются поля и вложенные формы, а значения — в node-скрипте.
type Widen<T> = T extends string
  ? string
  : T extends number
    ? number
    : T extends boolean
      ? boolean
      : T extends readonly (infer U)[]
        ? Widen<U>[]
        : T extends object
          ? { [K in keyof T]: Widen<T[K]> }
          : T

export const goldenArtifact = artifact satisfies Widen<SummaryArtifact>
export const goldenRecord = record satisfies Widen<SummaryRecord>
