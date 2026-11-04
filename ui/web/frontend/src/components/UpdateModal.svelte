<script lang="ts">
  import { renderMarkdown } from '../lib/markdown'
  import { system, collectDiagnostics, fmtBytes, fmtDateTime, loadSystem } from '../lib/system.svelte.ts'
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
  const hasDetails = $derived(Boolean(update.errorCode || update.hint || update.attempt))

  // Владелец пропа может не передать состояние записи: читаем его из снимка системы.
  $effect(() => {
    if (update.modalOpen && recording === undefined && !system.snapshot) void loadSystem()
  })

  const STAGE_LABELS: Record<string, string> = {
    starting: 'Запуск обновления',
    checking: 'Проверка обновления',
    download: 'Скачивание',
    install: 'Установка',
    restart: 'Перезапуск',
    apply: 'Установка',
  }

  const INSTALL_MODES: Record<string, string> = {
    self: 'самообновление',
    manual: 'установка вручную',
  }

  const ATTEMPT_OUTCOMES: Record<string, string> = {
    started: 'запущена',
    installed: 'установлено',
    failed: 'ошибка',
    not_confirmed: 'не подтверждено',
  }

  const detailRows = $derived.by(() => {
    const rows: Array<[string, string]> = []
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

  function stageText(stage: string): string {
    return STAGE_LABELS[stage] || 'Обновление'
  }

  function installText(mode: string): string {
    return INSTALL_MODES[mode] || mode || 'неизвестен'
  }

  function progressSize(): string {
    if (update.total > 0) {
      return ` · ${fmtBytes(update.downloaded) || '0 Б'} из ${fmtBytes(update.total)}`
    }
    if (update.downloaded > 0) return ` · скачано ${fmtBytes(update.downloaded)}`
    return ''
  }

  function retry(): void {
    if (update.stage === 'check') void checkUpdate(true, true)
    else void applyUpdate()
  }

  async function diagnostics(): Promise<void> {
    if (diagBusy) return
    diagBusy = true
    try {
      await collectDiagnostics()
    } finally {
      diagBusy = false
    }
  }

  function onWindowKey(e: KeyboardEvent): void {
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
        <div class="modal-title">Обновление</div>
        <button class="icon-btn" aria-label="Закрыть окно обновления" onclick={closeUpdateModal}>✕</button>
      </div>

      <div class="update-meta">
        <div class="update-version">Установлена версия {update.current || '…'}</div>
        {#if update.isNewer}
          <div class="update-version">
            Доступна {update.latest}
            {#if update.date} · {fmtDateTime(update.date)}{/if}
            {#if update.sizeBytes} · {fmtBytes(update.sizeBytes)}{/if}
          </div>
        {/if}
        <div class="help-text">
          Режим установки: {installText(update.installMode)}
          {#if update.stale} · данные проверки могли устареть{/if}
          {#if !update.sourcesAgree} · источники расходятся{/if}
        </div>
      </div>

      {#if update.notes && update.isNewer && !update.error}
        <div class="update-notes md-body">
          {@html renderMarkdown(update.notes)}
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
        <div class="update-error" role="alert">
          <div class="update-error-text">{update.error}</div>
          {#if update.stage === 'check' && update.retryable}
            <div class="help-text">Проверка не удалась. Повторите позже.</div>
          {/if}
        </div>
      {:else if update.isNewer && !update.selfUpdate}
        <div class="help-text">
          Эта сборка не обновляет себя сама. Скачайте пакет со страницы релиза
          и установите вручную.
        </div>
      {:else if !update.isNewer && !update.checking}
        <div class="help-text">
          Установлена последняя версия {update.current || update.latest || ''}.
        </div>
      {/if}

      {#if isRecording && update.isNewer && update.selfUpdate && !update.applying}
        <div class="update-warn">
          Идёт запись. Остановите её перед обновлением: приложение перезапустится.
        </div>
      {/if}

      {#if detailsOpen}
        <dl class="update-details">
          {#each detailRows as [label, value] (label)}
            <dt>{label}</dt>
            <dd>{value}</dd>
          {/each}
        </dl>
      {/if}

      <div class="update-footer">
        <div class="row-start">
          <button class="btn-secondary btn-sm" disabled={diagBusy} onclick={() => void diagnostics()}>
            {diagBusy ? 'Собираю…' : 'Собрать диагностику'}
          </button>
          {#if hasDetails}
            <button
              class="details-toggle"
              aria-expanded={detailsOpen}
              onclick={() => (detailsOpen = !detailsOpen)}
            >
              {detailsOpen ? 'Скрыть подробности' : 'Подробности'}
            </button>
          {/if}
        </div>
        <div class="update-actions">
          {#if update.isNewer && !update.error && !update.applying}
            {#if update.skippedVersion === update.latest}
              <button class="btn-secondary btn-sm" onclick={() => void skipVersion(null)}>
                Вернуть эту версию
              </button>
            {:else}
              <button class="btn-secondary btn-sm" onclick={() => void skipVersion(update.latest)}>
                Пропустить эту версию
              </button>
            {/if}
          {/if}
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
              Скачать со страницы релиза
            </a>
          {/if}
          {#if update.error && update.isNewer}
            <a class="btn-secondary btn-sm" href={releaseUrl} target="_blank" rel="noopener noreferrer">
              Страница релиза
            </a>
          {/if}
          <button class="btn-secondary" onclick={closeUpdateModal}>Позже</button>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .update-box {
    max-width: 620px;
  }
  .update-meta {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .update-version {
    font-size: 13px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .update-notes {
    max-height: 220px;
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
  .update-error {
    border-left: 3px solid var(--accent-rec);
    padding-left: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .update-error-text {
    font-size: 12.5px;
    color: var(--text-primary);
    word-break: break-word;
  }
  .update-warn {
    font-size: 12.5px;
    color: var(--accent-amber);
  }
  .update-details {
    display: grid;
    grid-template-columns: minmax(110px, max-content) minmax(0, 1fr);
    gap: 3px 12px;
    font-size: 11.5px;
    border-top: 1px solid var(--border-soft);
    padding-top: 8px;
  }
  .update-details dt {
    color: var(--text-muted);
  }
  .update-details dd {
    color: var(--text-primary);
    word-break: break-word;
  }
  .update-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .update-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
    margin-left: auto;
  }
  .details-toggle {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    font-size: 12px;
    color: var(--accent-blue);
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .details-toggle:hover {
    color: var(--text-primary);
  }
</style>
