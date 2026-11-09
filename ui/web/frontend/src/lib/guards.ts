export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

export const errText = (error: unknown, fallback = 'неизвестная ошибка'): string =>
  error instanceof Error ? error.message : typeof error === 'string' ? error : fallback

export const isAbortError = (error: unknown): boolean =>
  (error instanceof Error || error instanceof DOMException) && error.name === 'AbortError'
