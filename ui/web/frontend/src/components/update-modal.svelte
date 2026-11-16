<script lang="ts">
import { inline, renderMarkdown } from '../lib/markdown'
import {
  collectDiagnostics,
  fmtBytes,
  fmtDateTime,
  loadSystem,
  system,
} from '../lib/system.svelte.ts'
import { copyText, toast } from '../lib/toast.svelte.ts'
import {
  applyUpdate,
  checkUpdate,
  closeUpdateModal,
  skipVersion,
  update,
} from '../lib/update.svelte.ts'

let { recording }: { recording?: boolean } = $props()

let detailsOpen = $state(false)
let diagBusy = $state(false)

const isRecording = $derived(recording ?? system.snapshot?.recording?.active === true)
const releaseUrl = $derived(update.url || 'https://github.com/guest363/podtekst/releases/latest')
const hasDetails = $derived(
  Boolean(update.errorCode || update.hint || update.attempt || update.stale),
)

// Владелец пропа может не передать состояние записи: читаем его из снимка системы.
$effect(() => {
  if (update.modalOpen && recording === undefined && !system.snapshot) void loadSystem()
})

// Очистка заметок от универсальных шаблонов установки других ОС и diff-ссылок
const cleanedNotes = $derived.by(() => {
  if (!update.notes) return ''
  return update.notes
    .replace(/##?\s*Как обновиться[\s\S]*?(?=##|Z)/gi, '')
    .replace(/Инструкция по установке без подписи ОС:.*$/gim, '')
    .replace(/Полный список изменений:.*$/gim, '')
    .trim()
})

type ParsedNoteItem = {
  badge: 'НОВОЕ' | 'ИСПРАВЛЕНО' | 'КОМПОНЕНТЫ' | 'ОБНОВЛЕНИЕ'
  badgeType: 'ok' | 'warn' | 'neutral' | 'blue'
  title: string
  desc: string
}

type ParsedNotes = {
  intro: string
  items: ParsedNoteItem[]
}

const parsedNotes = $derived.by((): ParsedNotes => {
  if (!cleanedNotes) return { intro: '', items: [] }
  const lines = cleanedNotes.split('\n')
  const intro: string[] = []
  const items: ParsedNoteItem[] = []

  let currentBadge: ParsedNoteItem['badge'] = 'ОБНОВЛЕНИЕ'
  let currentBadgeType: ParsedNoteItem['badgeType'] = 'blue'

  for (const raw of lines) {
    const line = raw.trim()
    if (!line) continue

    const hMatch = line.match(/^#{2,3}\s+(.+)$/)
    if (hMatch) {
      const low = hMatch[1].toLowerCase()
      if (low.includes('нов') || low.includes('добавл')) {
        currentBadge = 'НОВОЕ'
        currentBadgeType = 'ok'
      } else if (low.includes('исправл') || low.includes('почин') || low.includes('баг')) {
        currentBadge = 'ИСПРАВЛЕНО'
        currentBadgeType = 'warn'
      } else if (low.includes('компонент')) {
        currentBadge = 'КОМПОНЕНТЫ'
        currentBadgeType = 'neutral'
      } else {
        currentBadge = 'ОБНОВЛЕНИЕ'
        currentBadgeType = 'blue'
      }
      continue
    }

    const bMatch = line.match(/^[-*]\s+(.+)$/)
    if (bMatch) {
      const rawText = bMatch[1].trim()
      let itemTitle = ''
      let itemDesc = rawText

      const boldMatch = rawText.match(/^\*\*([^*]+?)\*\*[:.]?\s*(.*)$/)
      if (boldMatch) {
        itemTitle = boldMatch[1].trim()
        itemDesc = boldMatch[2].trim()
      } else {
        const colonMatch = rawText.match(/^([^:]{2,35}):\s*(.*)$/)
        if (colonMatch && !rawText.startsWith('http')) {
          itemTitle = colonMatch[1].trim()
          itemDesc = colonMatch[2].trim()
        }
      }

      items.push({
        badge: currentBadge,
        badgeType: currentBadgeType,
        title: itemTitle,
        desc: itemDesc,
      })
      continue
    }

    if (items.length === 0) {
      intro.push(line)
    }
  }

  return {
    intro: intro.join(' '),
    items,
  }
})

const STAGE_LABELS: Record<string, string> = {
  starting: 'Запуск обновления',
  checking: 'Проверка обновления',
  download: 'Скачивание',
  install: 'Установка',
  restart: 'Перезапуск',
  apply: 'Установка',
}

const ATTEMPT_OUTCOMES: Record<string, string> = {
  started: 'запущена',
  installed: 'установлено',
  failed: 'ошибка',
  not_confirmed: 'не подтверждено',
}

const detailRows = $derived.by(() => {
  const rows: Array<[string, string]> = []
  rows.push([
    'Режим запуска',
    update.selfUpdate ? 'самообновление (AppImage)' : 'исходники (run.sh)',
  ])
  if (update.errorCode) rows.push(['Код ошибки', update.errorCode])
  if (update.hint) rows.push(['Подсказка', update.hint])
  if (update.retryAfterSec > 0) rows.push(['Повтор через', `${update.retryAfterSec} с`])
  rows.push(['Данные проверки', update.stale ? 'могли устареть' : 'актуальны'])
  if (update.attempt) {
    const a = update.attempt
    rows.push([
      'Последняя попытка',
      `${a.from || '—'} → ${a.to || '—'}, ${ATTEMPT_OUTCOMES[a.outcome] || a.outcome || 'без итога'}${
        a.started_at ? `, ${fmtDateTime(a.started_at)}` : ''
      }`,
    ])
  }
  return rows
})

const stageText = (stage: string): string => {
  return STAGE_LABELS[stage] || 'Обновление'
}

const progressSize = (): string => {
  if (update.total > 0) {
    return ` · ${fmtBytes(update.downloaded) || '0 Б'} из ${fmtBytes(update.total)}`
  }
  if (update.downloaded > 0) return ` · скачано ${fmtBytes(update.downloaded)}`
  return ''
}

const retry = (): void => {
  if (update.stage === 'check') void checkUpdate(true, true)
  else void applyUpdate()
}

const copyPullCommand = async (): Promise<void> => {
  const ok = await copyText('git pull && ./run.sh')
  toast(ok ? 'Команда скопирована' : 'Не удалось скопировать', ok ? '' : 'error')
}

const diagnostics = async (): Promise<void> => {
  if (diagBusy) return
  diagBusy = true
  try {
    await collectDiagnostics()
  } finally {
    diagBusy = false
  }
}

const onWindowKey = (e: KeyboardEvent): void => {
  if (update.modalOpen && e.key === 'Escape') closeUpdateModal()
}
</script>

<svelte:window onkeydown={onWindowKey} />

{#if update.modalOpen}
  <div
    class="modal-overlay open"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Обновление приложения"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) closeUpdateModal()
    }}
  >
    <div class="modal-box update-split-box">
      <div class="modal-top update-split-top">
        <div class="modal-title">Обновление приложения</div>
        <button class="icon-btn" aria-label="Закрыть окно обновления" onclick={closeUpdateModal}>✕</button>
      </div>

      <div class="update-split-body">
        <aside class="update-split-side">
          <div class="side-info-group">
            <div class="side-version-block">
              <div class="side-label">Доступна версия</div>
              <div class="side-version-num">{update.latest || '…'}</div>
              {#if update.isNewer}
                <div class="side-status-pill ok">Готова к установке</div>
              {:else if update.checking}
                <div class="side-status-pill">Проверка…</div>
              {:else}
                <div class="side-status-pill">Установлена</div>
              {/if}
            </div>

            <div class="side-meta-list">
              <div class="side-meta-row">
                <span class="side-meta-key">Текущая</span>
                <span class="side-meta-val">{update.current || '—'}</span>
              </div>
              {#if update.sizeBytes}
                <div class="side-meta-row">
                  <span class="side-meta-key">Размер</span>
                  <span class="side-meta-val">{fmtBytes(update.sizeBytes)}</span>
                </div>
              {/if}
              {#if update.date}
                <div class="side-meta-row">
                  <span class="side-meta-key">Выпуск</span>
                  <span class="side-meta-val">{fmtDateTime(update.date)}</span>
                </div>
              {/if}
              <div class="side-meta-row">
                <span class="side-meta-key">Режим</span>
                <span class="side-meta-val">{update.selfUpdate ? 'AppImage' : 'run.sh'}</span>
              </div>
            </div>

            {#if update.isNewer && !update.selfUpdate && !update.applying}
              <div class="side-cmd-block">
                <div class="side-cmd-label">Обновление сборки:</div>
                <input class="input-field input-field-sm" readonly value="git pull && ./run.sh" />
                <button class="btn-secondary btn-xs side-copy-btn" onclick={copyPullCommand}>
                  Скопировать команду
                </button>
              </div>
            {/if}

            {#if update.applying}
              <div class="update-progress">
                <div
                  class="progress-track"
                  role="progressbar"
                  aria-valuenow={update.pct}
                  aria-valuemin="0"
                  aria-valuemax="100"
                  aria-label="Прогресс обновления"
                >
                  <div class="vu-bar" style={`width: ${update.pct}%`}></div>
                </div>
                <div class="help-text">
                  {stageText(update.stage)} — {update.pct}%{progressSize()}
                </div>
                <div class="help-text">
                  После установки приложение перезапустится само.
                </div>
              </div>
            {:else if update.error}
              <div class="help-text error-text" role="alert">
                {update.error}
                {#if update.stage === 'check' && update.retryable}
                  <div>Проверка не удалась. Повторите позже.</div>
                {/if}
              </div>
            {:else if !update.isNewer && !update.checking}
              <div class="help-text">
                Установлена последняя версия {update.current || update.latest || ''}.
              </div>
            {/if}

            {#if isRecording && update.isNewer && update.selfUpdate && !update.applying}
              <div class="help-text warn-text">
                Идёт запись. Остановите её перед обновлением.
              </div>
            {/if}

            {#if hasDetails}
              <div class="update-details-wrap">
                <button
                  type="button"
                  class="details-toggle-btn"
                  aria-expanded={detailsOpen}
                  onclick={() => (detailsOpen = !detailsOpen)}
                >
                  <span>Технические подробности</span>
                  <span class="details-chevron {detailsOpen ? 'open' : ''}" aria-hidden="true">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                {#if detailsOpen}
                  <dl class="update-details">
                    {#each detailRows as [label, value] (label)}
                      <dt>{label}</dt>
                      <dd>{value}</dd>
                    {/each}
                    <dt>Диагностика</dt>
                    <dd>
                      <button class="link-btn" disabled={diagBusy} onclick={() => void diagnostics()}>
                        {diagBusy ? 'Собираю…' : 'Собрать отчёт'}
                      </button>
                    </dd>
                  </dl>
                {/if}
              </div>
            {/if}
          </div>

          <div class="side-actions">
            {#if !update.applying}
              {#if update.error && update.retryable}
                <button class="btn-primary w-full" onclick={retry}>Повторить</button>
              {:else if update.isNewer && update.selfUpdate}
                <button class="btn-primary w-full" disabled={isRecording} onclick={() => void applyUpdate()}>
                  Обновить и перезапустить
                </button>
              {:else if update.isNewer}
                <a class="btn-primary w-full" href={releaseUrl} target="_blank" rel="noopener noreferrer">
                  Страница релиза
                </a>
              {/if}
              <button class="btn-secondary w-full" onclick={closeUpdateModal}>Позже</button>
              {#if update.isNewer && !update.error}
                <div class="side-skip-wrap">
                  {#if update.skippedVersion === update.latest}
                    <button class="link-btn muted" onclick={() => void skipVersion(null)}>
                      Вернуть эту версию
                    </button>
                  {:else}
                    <button class="link-btn muted" onclick={() => void skipVersion(update.latest)}>
                      Пропустить эту версию
                    </button>
                  {/if}
                </div>
              {/if}
            {/if}
          </div>
        </aside>

        <section class="update-split-main" aria-label="Список изменений">
          <div class="main-header">
            <div class="main-title-row">
              <span class="main-title">Что изменилось</span>
              {#if parsedNotes.items.length > 0}
                <span class="count-badge">{parsedNotes.items.length}</span>
              {/if}
            </div>
            {#if parsedNotes.intro}
              <p class="main-intro">{parsedNotes.intro}</p>
            {/if}
          </div>

          <div class="notes-feed">
            {#if parsedNotes.items.length > 0}
              {#each parsedNotes.items as item}
                <div class="note-card">
                  <div class="note-card-header">
                    <span class="item-badge badge-{item.badgeType}">{item.badge}</span>
                    {#if item.title}
                      <span class="note-card-title">{item.title}</span>
                    {/if}
                  </div>
                  <div class="note-card-desc">
                    {@html inline(item.desc)}
                  </div>
                </div>
              {/each}
            {:else if cleanedNotes}
              <div class="notes-raw md-body">
                {@html renderMarkdown(cleanedNotes)}
              </div>
            {:else}
              <div class="empty-note">
                {#if update.checking}
                  Проверка обновлений…
                {:else}
                  Установлена последняя версия. Заметок нет.
                {/if}
              </div>
            {/if}

            {#if update.notesTruncated}
              <div class="help-text notes-truncated">
                Заметки сокращены. Полный текст — <a class="link-btn" href={releaseUrl} target="_blank" rel="noopener noreferrer">на странице релиза</a>.
              </div>
            {/if}
          </div>
        </section>
      </div>
    </div>
  </div>
{/if}

<style>
  .update-split-box {
    max-width: 760px;
    width: 94vw;
    padding: 0;
    gap: 0;
    overflow: hidden;
  }
  .update-split-top {
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-soft);
  }
  .update-split-body {
    display: grid;
    grid-template-columns: 248px minmax(0, 1fr);
    min-height: 420px;
  }
  .update-split-side {
    background: var(--bg-inset);
    border-right: 1px solid var(--border-soft);
    padding: 16px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 16px;
  }
  .side-info-group {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .side-version-block {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .side-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .side-version-num {
    font-size: 22px;
    font-weight: 800;
    letter-spacing: -0.02em;
    color: var(--text-primary);
  }
  .side-status-pill {
    display: inline-flex;
    align-items: center;
    width: fit-content;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    background: #fff;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-pill);
    padding: 1px 8px;
    margin-top: 2px;
  }
  .side-status-pill.ok {
    color: var(--accent-ok);
    background: var(--accent-ok-bg);
    border-color: #9fd8c0;
  }
  .side-meta-list {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
    padding: 10px 0;
    border-top: 1px solid var(--border-card);
    border-bottom: 1px solid var(--border-card);
  }
  .side-meta-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .side-meta-key {
    color: var(--text-muted);
  }
  .side-meta-val {
    font-weight: 600;
    color: var(--text-primary);
  }
  .side-cmd-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .side-cmd-label {
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .side-copy-btn {
    align-self: flex-start;
  }
  .btn-xs {
    padding: 3px 8px;
    font-size: 11px;
  }
  .input-field-sm {
    padding: 5px 8px;
    font-size: 11.5px;
  }
  .side-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 8px;
  }
  .w-full {
    width: 100%;
    justify-content: center;
    text-align: center;
  }
  .side-skip-wrap {
    text-align: center;
    padding-top: 2px;
  }
  .update-split-main {
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    background: #fff;
  }
  .main-header {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-soft);
  }
  .main-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .main-title {
    font-size: 14.5px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .main-intro {
    font-size: 12.5px;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .notes-feed {
    flex: 1;
    overflow-y: auto;
    max-height: 380px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-right: 4px;
  }
  .note-card {
    background: #fff;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    padding: 9px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    transition: border-color 140ms ease;
  }
  .note-card:hover {
    border-color: #cbd5e1;
  }
  .note-card-header {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }
  .item-badge {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 6px;
    border-radius: 4px;
    line-height: 1.3;
    white-space: nowrap;
  }
  .badge-ok {
    background: var(--accent-ok-bg);
    color: var(--accent-ok);
    border: 1px solid #9fd8c0;
  }
  .badge-warn {
    background: var(--accent-amber-bg);
    color: var(--accent-amber);
    border: 1px solid #d9b45c;
  }
  .badge-neutral {
    background: var(--bg-inset);
    color: var(--text-muted);
    border: 1px solid var(--border-card);
  }
  .badge-blue {
    background: #edf3fc;
    color: var(--accent-blue);
    border: 1px solid #b8d0f8;
  }
  .note-card-title {
    font-size: 12.5px;
    font-weight: 700;
    color: var(--text-primary);
    line-height: 1.35;
  }
  .note-card-desc {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-muted);
  }
  .note-card-desc :global(code) {
    font-family: var(--font-mono);
    font-size: 11px;
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: 3px;
    padding: 1px 4px;
    color: var(--text-primary);
  }
  .note-card-desc :global(b),
  .note-card-desc :global(strong) {
    color: var(--text-primary);
    font-weight: 700;
  }
  .notes-raw {
    font-size: 12.5px;
    line-height: 1.5;
  }
  .notes-truncated {
    padding-top: 4px;
    font-size: 11.5px;
  }
  .cmd-field-group {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .input-field {
    padding: 7px 12px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    font-size: 12.5px;
    font-family: var(--font-mono);
    outline: none;
    background: var(--bg-inset);
    width: 100%;
    color: var(--text-primary);
  }
  .update-progress {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .update-details-wrap {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border-top: 1px solid var(--border-card);
    padding-top: 8px;
  }
  .details-toggle-btn {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    line-height: 1;
    width: fit-content;
  }
  .details-toggle-btn:hover {
    color: var(--text-primary);
  }
  .details-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    transition: transform 160ms var(--ease-spring);
    flex-shrink: 0;
    width: 12px;
    height: 12px;
  }
  .details-chevron.open {
    transform: rotate(180deg);
  }
  .update-details {
    display: grid;
    grid-template-columns: minmax(100px, max-content) minmax(0, 1fr);
    gap: 3px 10px;
    font-size: 11px;
  }
  .update-details dt {
    color: var(--text-muted);
  }
  .update-details dd {
    color: var(--text-primary);
    word-break: break-word;
  }
  .error-text {
    color: var(--accent-rec);
  }
  .warn-text {
    color: var(--accent-amber);
  }
  .link-btn.muted {
    color: var(--text-faint);
  }
  .link-btn.muted:hover {
    color: var(--text-primary);
  }
  @media (max-width: 640px) {
    .update-split-body {
      grid-template-columns: 1fr;
    }
    .update-split-side {
      border-right: none;
      border-bottom: 1px solid var(--border-soft);
    }
  }
</style>
