import { readJsonSafe } from './api'

export class SummaryApiError extends Error {
  code: string
  status: number
  constructor(code: string, status: number, message?: string) {
    super(message || code)
    this.code = code
    this.status = status
  }
}

type DevModule = typeof import('./summary.dev.ts')
let devModule: DevModule | null | undefined
let mockMode = false

async function devMock(): Promise<DevModule | null> {
  if (devModule !== undefined) return devModule
  if (
    !import.meta.env.DEV ||
    typeof window === 'undefined' ||
    !new URLSearchParams(window.location.search).has('summary_mock')
  ) {
    devModule = null
    return null
  }
  devModule = await import('./summary.dev.ts')
  return devModule
}

/** Dev-стенд подменяет только транспорт `/api/summary*`, не production-путь. */
export function isMockMode(): boolean {
  return mockMode
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  const dev = await devMock()
  mockMode = Boolean(dev)
  if (dev) return (await dev.request(path, init)) as T
  const r = await fetch(path, init)
  let d: unknown
  try {
    d = await readJsonSafe(r)
  } catch {
    throw new SummaryApiError('internal', r.status)
  }
  const body = d as { ok?: boolean; code?: string; error?: string }
  if (!r.ok || body?.ok === false) {
    throw new SummaryApiError(body?.code || `http_${r.status}`, r.status, body?.error)
  }
  return body as T
}
