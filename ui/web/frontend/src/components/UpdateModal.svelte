<script lang="ts">
import { renderMarkdown } from '../lib/markdown'
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
  Boolean(
    update.errorCode || update.hint || update.attempt || !update.sourcesAgree || update.stale,
  ),
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
  rows.push(['Источники', update.sourcesAgree ? 'совпадают' : 'расходятся'])
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
    <div class="modal-box update-box">
      <div class="modal-top">
        <div class="modal-title">Обновление приложения</div>
        <button class="icon-btn" aria-label="Закрыть окно обновления" onclick={closeUpdateModal}>✕</button>
      </div>

      <div class="update-version-row">
        <span>Установлена версия {update.current || '…'}</span>
        {#if update.isNewer}
          <span class="update-version-avail">
            Доступна {update.latest}
            {#if update.date}<span class="update-date"> · {fmtDateTime(update.date)}</span>{/if}
            {#if update.sizeBytes}<span class="update-date"> · {fmtBytes(update.sizeBytes)}</span>{/if}
          </span>
        {/if}
      </div>

      {#if update.isNewer && !update.selfUpdate && !update.applying}
        <div class="help-text">
          Приложение запущено из репозитория через <code>./run.sh</code>.
          Встроенный перезапуск доступен только в готовом пакете AppImage.
          Чтобы обновить текущую сборку, выполните в терминале:
        </div>
        <div class="cmd-field-group">
          <input class="input-field" readonly value="git pull && ./run.sh" />
          <button class="btn-secondary btn-sm" onclick={copyPullCommand}>Скопировать</button>
        </div>
      {/if}

      {#if cleanedNotes && update.isNewer && !update.error}
        <div class="update-notes md-body">
          {@html renderMarkdown(cleanedNotes)}
        </div>
        {#if update.notesTruncated}
          <div class="help-text">
            Заметки сокращены до 16 КБ. Полный текст — на странице релиза.
          </div>
        {/if}
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
            После установки приложение перезапустится само. Отменить скачивание нельзя.
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
          Идёт запись. Остановите её перед обновлением: приложение перезапустится.
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
                  {diagBusy ? 'Собираю…' : 'Собрать отчёт системы'}
                </button>
              </dd>
            </dl>
          {/if}
        </div>
      {/if}

      <div class="row-between">
        <div>
          {#if update.isNewer && !update.error && !update.applying}
            {#if update.skippedVersion === update.latest}
              <button class="link-btn muted" onclick={() => void skipVersion(null)}>
                Вернуть эту версию
              </button>
            {:else}
              <button class="link-btn muted" onclick={() => void skipVersion(update.latest)}>
                Пропустить эту версию
              </button>
            {/if}
          {/if}
        </div>
        <div class="row-start">
          <button class="btn-secondary" onclick={closeUpdateModal}>Позже</button>
          {#if update.applying}
            <!-- Идёт скачивание: отмены нет, ждём перезапуска. -->
          {:else if update.error && update.retryable}
            <button class="btn-primary" onclick={retry}>Повторить</button>
          {:else if update.isNewer && update.selfUpdate}
            <button class="btn-primary" disabled={isRecording} onclick={() => void applyUpdate()}>
              Обновить и перезапустить
            </button>
          {:else if update.isNewer}
            <a class="btn-primary" href={releaseUrl} target="_blank" rel="noopener noreferrer">
              Страница релиза
            </a>
          {/if}
          {#if update.error && update.isNewer}
            <a class="btn-secondary btn-sm" href={releaseUrl} target="_blank" rel="noopener noreferrer">
              Страница релиза
            </a>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .update-box {
    max-width: 520px;
    gap: 14px;
  }
  .update-version-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    font-size: 13px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .update-version-avail {
    color: var(--accent-ok);
  }
  .update-date {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-muted);
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
  .update-notes {
    max-height: 200px;
    overflow-y: auto;
    padding: 8px 0;
    border-top: 1px solid var(--border-soft);
    border-bottom: 1px solid var(--border-soft);
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
    border-top: 1px solid var(--border-soft);
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
    grid-template-columns: minmax(110px, max-content) minmax(0, 1fr);
    gap: 3px 12px;
    font-size: 11.5px;
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
</style>
