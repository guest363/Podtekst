import { readJsonSafe } from './api'
import { loadSystem } from './system.svelte.ts'
import { toast } from './toast.svelte.ts'

export interface UpdateAttempt {
  from: string
  to: string
  started_at: string
  outcome: string
}

export const update = $state({
  current: '',
  latest: '',
  tag: '',
  name: '',
  date: '',
  notes: '',
  notesTruncated: false,
  sizeBytes: 0,
  isNewer: false,
  selfUpdate: false,
  installMode: '',
  url: '',
  sourcesAgree: true,
  stale: false,
  lastCheckAt: '',
  skippedVersion: '',
  checking: false,
  applying: false,
  pct: 0,
  downloaded: 0,
  total: 0,
  stage: '',
  error: '',
  errorCode: '',
  retryable: false,
  hint: '',
  retryAfterSec: 0,
  dismissed: false,
  dismissedVersion: '',
  modalOpen: false,
  attempt: null as UpdateAttempt | null,
})

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}

function attemptOf(v: unknown): UpdateAttempt | null {
  if (!v || typeof v !== 'object') return null
  const a = v as Record<string, unknown>
  return {
    from: str(a.from),
    to: str(a.to),
    started_at: str(a.started_at),
    outcome: str(a.outcome),
  }
}

/** Предложение показывается заново только для другой версии: фоновый опрос не сбрасывает «Позже». */
function syncDismissed(): void {
  if (!update.isNewer) {
    update.dismissed = false
    return
  }
  update.dismissed =
    update.latest === update.dismissedVersion || update.latest === update.skippedVersion
}

export async function loadUpdateState(): Promise<void> {
  try {
    const d = await readJsonSafe(await fetch('/api/update/state'))
    if (d.ok === false) return
    if (str(d.current)) update.current = str(d.current)
    if (typeof d.self_update === 'boolean') update.selfUpdate = d.self_update
    if (str(d.install_mode)) update.installMode = str(d.install_mode)
    update.skippedVersion = str(d.skipped_version)
    update.attempt = attemptOf(d.last_attempt)
    const lc = d.last_check
    if (lc && typeof lc === 'object') {
      if (str(lc.latest)) update.latest = str(lc.latest)
      if (str(lc.at)) update.lastCheckAt = str(lc.at)
      if (typeof lc.is_newer === 'boolean') update.isNewer = lc.is_newer
      update.stale = lc.stale === true
    }
    syncDismissed()
  } catch {
    // Состояние необязательно: проверка ниже всё равно запросит свежие данные.
  }
}

export async function checkUpdate(force = false, notify = false): Promise<void> {
  if (update.checking) return
  update.checking = true
  update.error = ''
  try {
    await loadUpdateState()
    const d = await readJsonSafe(
      await fetch(`/api/update/check${force ? '?force=1' : ''}`),
    )
    if (str(d.current)) update.current = str(d.current)
    if (typeof d.self_update === 'boolean') update.selfUpdate = d.self_update
    if (str(d.install_mode)) update.installMode = str(d.install_mode)
    update.skippedVersion = str(d.skipped_version)

    if (d.ok === false) {
      update.error = str(d.error) || 'Не удалось проверить обновления'
      update.errorCode = str(d.error_code)
      update.stage = str(d.stage) || 'check'
      update.retryable = d.retryable === true
      update.hint = str(d.hint)
      update.retryAfterSec = num(d.retry_after_sec)
      update.stale = d.stale === true
      if (notify) toast(update.error, 'error')
      return
    }

    update.latest = str(d.latest)
    update.tag = str(d.tag)
    update.name = str(d.name)
    update.date = str(d.date)
    update.notes = str(d.notes)
    update.notesTruncated = d.notes_truncated === true
    update.sizeBytes = num(d.size_bytes)
    update.isNewer = d.is_newer === true
    update.url = str(d.url)
    update.sourcesAgree = d.sources_agree !== false
    update.stale = d.stale === true
    update.lastCheckAt = new Date().toISOString()
    update.errorCode = ''
    update.stage = ''
    update.retryable = false
    update.hint = ''
    update.retryAfterSec = 0
    syncDismissed()

    if (update.isNewer) {
      if (notify) toast(`Доступна версия ${update.latest} — предложение на главном экране`)
    } else if (notify) {
      toast(`У вас последняя версия ${update.current}`)
    }
  } catch (e) {
    update.error = 'Не удалось проверить обновления: ' + (e as Error).message
    update.errorCode = 'network'
    update.stage = 'check'
    update.retryable = true
    if (notify) toast(update.error, 'error')
  } finally {
    update.checking = false
  }
}

let progressTimer: ReturnType<typeof setInterval> | undefined

function stopProgress(): void {
  if (progressTimer) {
    clearInterval(progressTimer)
    progressTimer = undefined
  }
}

function startProgress(): void {
  stopProgress()
  progressTimer = setInterval(() => void pollProgress(), 1000)
}

function failApply(d: any): void {
  update.applying = false
  update.error = str(d.error) || 'Не удалось начать обновление'
  update.errorCode = str(d.error_code)
  update.stage = str(d.stage) || 'apply'
  update.retryable = d.retryable === true
  update.hint = str(d.hint)
  update.retryAfterSec = num(d.retry_after_sec)
  toast(update.error, 'error')
}

async function pollProgress(): Promise<void> {
  try {
    const p = await readJsonSafe(await fetch('/api/update/progress'))
    update.pct = num(p.pct)
    update.downloaded = num(p.downloaded)
    update.total = num(p.total)
    if (str(p.stage)) update.stage = str(p.stage)
    if (str(p.version)) update.latest = str(p.version)
    if (p.error) {
      stopProgress()
      update.applying = false
      update.error = str(p.error)
      update.errorCode = str(p.error_code) || 'install_failed'
      update.retryable = p.retryable !== false
      update.hint = str(p.hint)
      toast('Обновление не установилось: ' + update.error, 'error')
    } else if (!p.running) {
      // Приложение перезапускается: сервер перестанет отвечать, опрос больше не нужен.
      stopProgress()
    }
  } catch {
    stopProgress()
  }
}

export async function applyUpdate(): Promise<void> {
  if (update.applying) return
  stopProgress()
  update.applying = true
  update.pct = 0
  update.downloaded = 0
  update.total = 0
  update.stage = 'starting'
  update.error = ''
  update.errorCode = ''
  update.hint = ''
  update.retryable = false
  try {
    const d = await readJsonSafe(await fetch('/api/update/apply', { method: 'POST' }))
    if (d.ok === false) {
      failApply(d)
      return
    }
    if (str(d.stage)) update.stage = str(d.stage)
    if (str(d.version)) update.latest = str(d.version)
  } catch (e) {
    // Соединение могло оборваться уже после старта установки: проверяем прогресс.
    try {
      const p = await readJsonSafe(await fetch('/api/update/progress'))
      if (p.running) {
        if (str(p.stage)) update.stage = str(p.stage)
        startProgress()
        return
      }
    } catch {
      // Сервер не отвечает — установка действительно не началась.
    }
    update.applying = false
    update.error = 'Обновление не запустилось: ' + (e as Error).message
    update.errorCode = 'apply_failed'
    update.retryable = true
    toast(update.error, 'error')
    return
  }
  startProgress()
}

export function dismissUpdate(): void {
  update.dismissed = true
  update.dismissedVersion = update.latest
  update.modalOpen = false
}

export async function skipVersion(version: string | null): Promise<void> {
  try {
    const d = await readJsonSafe(
      await fetch('/api/update/skip', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ version }),
      }),
    )
    if (d.ok === false) throw new Error(str(d.error) || 'Не удалось сохранить пропуск')
    update.skippedVersion = version || ''
    if (version) {
      if (version === update.latest) {
        update.dismissed = true
        update.dismissedVersion = version
        update.modalOpen = false
      }
      toast(`Версия ${version} пропущена`)
      return
    }
    if (update.dismissedVersion === update.latest) {
      update.dismissed = false
      update.dismissedVersion = ''
    }
    toast('Пропуск версии снят')
  } catch (e) {
    toast('Не удалось сохранить пропуск: ' + (e as Error).message, 'error')
  }
}

export function openUpdateModal(): void {
  update.modalOpen = true
  void loadSystem()
  if (!update.latest && !update.checking) void checkUpdate()
}

export function closeUpdateModal(): void {
  update.modalOpen = false
}
