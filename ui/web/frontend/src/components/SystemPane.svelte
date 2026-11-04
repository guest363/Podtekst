<script lang="ts">
  import { onMount } from 'svelte'
  import { fmtClock } from '../lib/api'
  import type { ModelInfo } from '../lib/api'
  import { toast } from '../lib/toast.svelte.ts'
  import { downloadFfmpeg, statusBadge } from '../lib/settings.svelte.ts'
  import { downloadModel } from '../lib/transcript.svelte.ts'
  import { checkUpdate, openUpdateModal, update } from '../lib/update.svelte.ts'
  import {
    ackCrash,
    collectDiagnostics,
    crashHasBacktrace,
    crashMessage,
    crashUnacknowledged,
    crashVersion,
    fmtBytes,
    fmtDateTime,
    loadCrashes,
    loadJournal,
    loadSystem,
    openTarget,
    runChecks,
    system,
  } from '../lib/system.svelte.ts'
  import type {
    CheckFix,
    DevicesInfo,
    DiskInfo,
    IntegrationInfo,
    JournalSession,
    MemoryInfo,
    PermissionInfo,
    PermissionsInfo,
    SystemSnapshot,
  } from '../lib/system.svelte.ts'

  let probeAudio = $state(true)

  const snap = $derived(system.snapshot)
  const pathRows = $derived(Object.entries(snap?.paths ?? {}))
  const devices = $derived(deviceRows(snap?.devices))
  const permissions = $derived(permissionRows(snap?.permissions))
  const integrations = $derived(integrationRows(snap?.integration))
  const lastCrash = $derived(system.crashes.find(crashUnacknowledged) ?? null)
  const selfUpdate = $derived(snap?.update?.mode === 'self' || update.selfUpdate)
  const skippedVersion = $derived(snap?.update?.skipped_version || update.skippedVersion)
  const latestKnown = $derived(snap?.update?.latest || update.latest)
  const lastCheckAt = $derived(snap?.update?.last_check_at || update.lastCheckAt)

  const PATH_LABELS: Record<string, string> = {
    data: 'Данные',
    recordings: 'Записи',
    models: 'Модели',
    packs: 'Пакеты',
    logs: 'Логи',
    crashes: 'Сбои',
    diagnostics: 'Диагностика',
    cache: 'Кэш',
    temp: 'Временные файлы',
  }

  const PERMISSION_LABELS: Record<string, string> = {
    microphone: 'Микрофон',
    system_audio: 'Системный звук',
    screen_capture_supported: 'Запись экрана',
    screen_recording: 'Запись экрана',
    accessibility: 'Универсальный доступ',
    notifications: 'Уведомления',
    input_monitoring: 'Мониторинг ввода',
  }

  const MODE_LABELS: Record<string, string> = {
    split: '2 канала',
    mix: '1 канал',
    screen: 'экран',
  }

  const OS_NAMES: Record<string, string> = {
    linux: 'Linux',
    windows: 'Windows',
    macos: 'macOS',
    android: 'Android',
    ios: 'iOS',
  }

  interface PermissionRow {
    name: string
    value: string
    ok?: boolean
  }

  onMount(() => {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
  })

  function refresh(): void {
    void loadSystem()
    void loadJournal(20)
    void loadCrashes()
  }

  function osText(os: SystemSnapshot['os']): string {
    if (!os) return ''
    if (typeof os === 'string') return os
    const name = os.name ? OS_NAMES[os.name] || os.name : os.family
    return [name, os.version, os.arch, os.kernel].filter(Boolean).join(' · ')
  }

  function diskText(d?: DiskInfo): string {
    if (!d) return ''
    const parts: string[] = []
    if (d.free_bytes) parts.push(`свободно ${fmtBytes(d.free_bytes)}`)
    if (d.total_bytes) parts.push(`из ${fmtBytes(d.total_bytes)}`)
    return parts.join(' ') || d.path || '—'
  }

  function memoryText(m?: MemoryInfo): string {
    if (!m) return ''
    const parts: string[] = []
    if (m.available_bytes) parts.push(`доступно ${fmtBytes(m.available_bytes)}`)
    if (m.total_bytes) parts.push(`из ${fmtBytes(m.total_bytes)}`)
    return parts.join(' ') || '—'
  }

  function deviceRows(v?: DevicesInfo): Array<{ name: string; meta: string }> {
    if (!v) return []
    const rows: Array<{ name: string; meta: string }> = []
    const add = (list: DevicesInfo['inputs'], kind: string): void => {
      for (const d of list || []) {
        const parts = [kind]
        if (d.muted) parts.push('выключено')
        if (d.system) parts.push('системный')
        rows.push({ name: String(d.name || d.id || 'Устройство'), meta: parts.join(' · ') })
      }
    }
    add(v.inputs, 'вход')
    add(v.system, 'системный звук')
    return rows
  }

  function defaultName(v: DevicesInfo, id?: string | null): string {
    if (!id) return ''
    const all = [...(v.inputs || []), ...(v.system || [])]
    return all.find((d) => d.id === id)?.name || id
  }

  function permissionRows(
    v: PermissionsInfo | PermissionInfo[] | undefined,
  ): PermissionRow[] {
    if (!v) return []
    if (Array.isArray(v)) {
      return v.map((p) => ({
        name: String(p.name || PERMISSION_LABELS[String(p.id || '')] || p.id || 'Разрешение'),
        value: p.granted === true ? 'разрешено' : p.granted === false ? 'нет' : String(p.detail || ''),
        ok: typeof p.granted === 'boolean' ? p.granted : undefined,
      }))
    }
    return Object.entries(v).map(([key, value]) => ({
      name: PERMISSION_LABELS[key] || key,
      value:
        value === true ? 'разрешено'
        : value === false ? 'нет'
        : value === 'unknown' ? 'не проверено'
        : String(value),
      ok: typeof value === 'boolean' ? value : undefined,
    }))
  }

  function integrationRows(
    v: IntegrationInfo | undefined,
  ): Array<{ label: string; value: string }> {
    if (!v) return []
    const rows: Array<{ label: string; value: string }> = []
    const hotkey = v.hotkey
    if (hotkey && typeof hotkey === 'object') {
      const reg =
        hotkey.registered === true ? 'зарегистрирована'
        : hotkey.registered === false ? 'не зарегистрирована'
        : 'регистрация не проверена'
      rows.push({ label: 'Горячая клавиша', value: `${hotkey.accelerator || '—'} — ${reg}` })
    }
    if (typeof v.autostart === 'boolean') {
      rows.push({ label: 'Автозапуск', value: v.autostart ? 'включён' : 'выключен' })
    }
    if (typeof v.single_instance === 'boolean') {
      rows.push({ label: 'Один экземпляр', value: v.single_instance ? 'да' : 'нет' })
    }
    if (typeof v.ipc === 'boolean') {
      rows.push({ label: 'Управление через IPC', value: v.ipc ? 'доступно' : 'нет' })
    }
    if (typeof v.updater === 'string') {
      rows.push({ label: 'Обновление', value: installModeText(v.updater) })
    }
    return rows
  }

  function journalFile(s: JournalSession): string {
    if (!s.file) return s.id
    if (typeof s.file === 'string') return s.file.split(/[\\/]/).pop() || s.file
    return s.file.name || s.file.id || s.id
  }

  function journalMeta(s: JournalSession): string {
    const parts: string[] = []
    if (s.started_at) parts.push(fmtDateTime(s.started_at))
    if (s.duration_sec) parts.push(fmtClock(s.duration_sec))
    if (s.channels) parts.push(s.channels === 1 ? '1 канал' : `${s.channels} канала`)
    else if (s.mode) parts.push(MODE_LABELS[s.mode] || s.mode)
    if (s.finalized === false) parts.push('не финализирована')
    if (s.errors?.length) parts.push(`Ошибок: ${s.errors.length}`)
    if (s.warnings?.length) parts.push(`Предупреждений: ${s.warnings.length}`)
    if (s.silent) parts.push('тихая запись')
    if (s.sys_silent) parts.push('тихий системный канал')
    if (s.transcribe?.status) parts.push(`Расшифровка: ${s.transcribe.status}`)
    return parts.join(' · ')
  }

  function journalBadge(s: JournalSession): string {
    if (s.status === 'ok' || s.status === 'completed' || s.status === 'done' || s.status === 'finalized') {
      return statusBadge('ok', 'Готово')
    }
    if (s.status === 'failed' || s.status === 'error') return statusBadge('error', 'Ошибка')
    if (s.status === 'recording') return statusBadge('info', 'Идёт')
    if (s.status === 'interrupted' || s.status === 'aborted') return statusBadge('warn', 'Прервана')
    return statusBadge('info', s.status || 'Без статуса')
  }

  function checkBadge(status: string): string {
    if (status === 'ok') return statusBadge('ok', 'Норма')
    if (status === 'warn') return statusBadge('warn', 'Замечание')
    if (status === 'error') return statusBadge('error', 'Ошибка')
    return statusBadge('info', 'Пропущено')
  }

  function summaryText(): string {
    const s = system.summary
    if (!s) return ''
    const parts = [
      `Норма: ${s.ok}`,
      `Замечания: ${s.warn}`,
      `Ошибки: ${s.error}`,
      `Пропущено: ${s.skip}`,
    ]
    if (system.checkedAt) parts.push(fmtDateTime(system.checkedAt))
    return parts.join(' · ')
  }

  function installModeText(mode: string): string {
    if (mode === 'self') return 'самообновление'
    if (mode === 'manual') return 'вручную'
    return mode || 'неизвестен'
  }

  async function downloadModelById(id: string): Promise<void> {
    if (!id) {
      await openTarget('models')
      return
    }
    const model: ModelInfo = { id, title: id, is_installed: false }
    if (await downloadModel(model)) void loadSystem()
  }

  async function handleFix(fix: CheckFix | null): Promise<void> {
    if (!fix) return
    switch (fix.kind) {
      case 'open_folder':
      case 'open_path':
      case 'open':
        await openTarget(fix.target)
        break
      case 'open_settings':
        await openTarget(fix.target)
        break
      case 'download_model':
      case 'model':
        await downloadModelById(fix.target)
        break
      case 'download_ffmpeg':
      case 'ffmpeg':
        await downloadFfmpeg()
        break
      case 'open_update':
      case 'update':
        openUpdateModal()
        break
      case 'collect_diagnostics':
      case 'diagnostics':
        await collectDiagnostics()
        break
      default:
        toast('Это действие пока не поддерживается')
    }
  }
</script>

<div class="system-pane">
  <section class="pane-section">
    <div class="section-head">
      <div class="card-title">Обзор</div>
      <button class="btn-secondary btn-sm" disabled={system.loading} onclick={refresh}>
        {system.loading ? 'Обновляю…' : 'Обновить'}
      </button>
    </div>
    {#if system.error}
      <div class="help-text pane-error">{system.error}</div>
    {/if}
    {#if snap}
      <div class="kv-grid">
        <span class="kv-key">Версия</span><span class="kv-val">{snap.version || '—'}</span>
        <span class="kv-key">Система</span><span class="kv-val">{osText(snap.os) || '—'}</span>
        {#if snap.disk}
          <span class="kv-key">Диск</span><span class="kv-val">{diskText(snap.disk)}</span>
        {/if}
        {#if snap.memory}
          <span class="kv-key">Память</span><span class="kv-val">{memoryText(snap.memory)}</span>
        {/if}
        {#if snap.recording?.active}
          <span class="kv-key">Запись</span>
          <span class="kv-val">
            идёт {fmtClock(snap.recording.elapsed || 0)}
            {#if snap.recording.mode} · {MODE_LABELS[snap.recording.mode] || snap.recording.mode}{/if}
          </span>
        {/if}
        {#if snap.log?.path}
          <span class="kv-key">Лог</span>
          <span class="kv-val path-val">
            {snap.log.path}{#if snap.log.bytes} · {fmtBytes(snap.log.bytes)}{/if}
          </span>
        {/if}
      </div>
      {#if pathRows.length}
        <div class="kv-grid">
          {#each pathRows as [key, value] (key)}
            <span class="kv-key">{PATH_LABELS[key] || key}</span>
            <span class="kv-val path-val">{value}</span>
          {/each}
        </div>
      {/if}
    {:else if system.loading}
      <div class="help-text">Загружаю состояние…</div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">Проверка настроек</div>
    <label class="check-probe">
      <input type="checkbox" bind:checked={probeAudio} />
      <span>Проверить микрофон короткой записью</span>
    </label>
    <div class="row-start">
      <button
        class="btn-secondary"
        disabled={system.checksLoading}
        onclick={() => void runChecks(probeAudio)}
      >
        {system.checksLoading ? 'Проверяю…' : 'Проверить настройки'}
      </button>
    </div>
    {#if system.summary}
      <div class="help-text">{summaryText()}</div>
    {/if}
    {#if system.checks.length}
      <div class="recordings-list">
        {#each system.checks as c (c.id)}
          <div class="rec-item check-item">
            <div class="rec-info">
              <span class="rec-name">{c.title}</span>
              {#if c.detail}<span class="rec-meta">{c.detail}</span>{/if}
              {#if c.hint}<span class="rec-meta check-hint">{c.hint}</span>{/if}
            </div>
            <div class="rec-actions">
              {@html checkBadge(c.status)}
              {#if c.fix}
                <button class="btn-secondary btn-sm" onclick={() => void handleFix(c.fix)}>
                  {c.fix.label || 'Исправить'}
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {:else if !system.checksLoading}
      <div class="help-text">Проверки ещё не запускались.</div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">Модели</div>
    {#if snap?.models}
      <div class="kv-grid">
        <span class="kv-key">Базовые модели</span>
        <span class="kv-val">{snap.models.core_ready ? 'готовы' : 'не хватает'}</span>
        <span class="kv-key">Установлено</span>
        <span class="kv-val">{snap.models.installed ?? 0} из {snap.models.total ?? 0}</span>
        {#if snap.models.missing_core?.length}
          <span class="kv-key">Не хватает</span>
          <span class="kv-val">{snap.models.missing_core.join(', ')}</span>
        {/if}
        {#if snap.models.updates?.length}
          <span class="kv-key">Обновления</span>
          <span class="kv-val">{snap.models.updates.join(', ')}</span>
        {/if}
      </div>
    {:else}
      <div class="help-text">Сводка по моделям недоступна.</div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">ffmpeg</div>
    <div class="rec-item">
      <div class="rec-info">
        <span class="rec-name">ffmpeg</span>
        <span class="rec-meta">
          {#if snap?.ffmpeg?.available}
            Установлен
          {:else if snap?.ffmpeg?.downloading}
            Скачивается {snap.ffmpeg.pct || 0}%
          {:else}
            Не найден. Видео, запись экрана и экспорт MP3 недоступны.
          {/if}
        </span>
        {#if snap?.ffmpeg?.error}
          <span class="rec-meta">{snap.ffmpeg.error}</span>
        {/if}
      </div>
      <div class="rec-actions">
        {@html statusBadge(snap?.ffmpeg?.available ? 'ok' : 'warn', snap?.ffmpeg?.available ? 'Есть' : 'Нет')}
      </div>
    </div>
  </section>

  <section class="pane-section">
    <div class="card-title">Аудио и разрешения</div>
    {#if devices.length}
      <div class="recordings-list">
        {#each devices as d, i (i)}
          <div class="rec-item">
            <div class="rec-info">
              <span class="rec-name">{d.name}</span>
              {#if d.meta}<span class="rec-meta">{d.meta}</span>{/if}
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <div class="help-text">Устройства не перечислены.</div>
    {/if}
    {#if snap?.devices}
      <div class="kv-grid">
        <span class="kv-key">Вход по умолчанию</span>
        <span class="kv-val">{defaultName(snap.devices, snap.devices.default_input) || 'не выбран'}</span>
        <span class="kv-key">Системный звук</span>
        <span class="kv-val">{snap.devices.has_system_audio ? 'доступен' : 'не найден'}</span>
      </div>
    {/if}
    {#if permissions.length}
      <div class="recordings-list">
        {#each permissions as p, i (i)}
          <div class="rec-item">
            <div class="rec-info">
              <span class="rec-name">{p.name}</span>
              {#if p.value}<span class="rec-meta">{p.value}</span>{/if}
            </div>
            {#if p.ok !== undefined}
              <div class="rec-actions">
                {@html statusBadge(p.ok ? 'ok' : 'warn', p.ok ? 'Есть' : 'Нет')}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">Интеграция</div>
    {#if integrations.length}
      <div class="kv-grid">
        {#each integrations as row (row.label)}
          <span class="kv-key">{row.label}</span><span class="kv-val">{row.value}</span>
        {/each}
      </div>
    {:else}
      <div class="help-text">Данные интеграции недоступны.</div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">Обновления и поставка</div>
    <div class="kv-grid">
      <span class="kv-key">Версия</span>
      <span class="kv-val">{snap?.update?.current || snap?.version || update.current || '—'}</span>
      <span class="kv-key">Режим установки</span>
      <span class="kv-val">{installModeText(snap?.update?.mode || update.installMode)}</span>
      <span class="kv-key">Самообновление</span>
      <span class="kv-val">{selfUpdate ? 'доступно' : 'только вручную'}</span>
      <span class="kv-key">Подпись ОС</span>
      <span class="kv-val">{snap?.update?.signed === true ? 'есть' : 'нет — ожидаемо для этой сборки'}</span>
      {#if latestKnown}
        <span class="kv-key">Последняя версия</span><span class="kv-val">{latestKnown}</span>
      {/if}
      {#if lastCheckAt}
        <span class="kv-key">Проверено</span><span class="kv-val">{fmtDateTime(lastCheckAt)}</span>
      {/if}
      {#if skippedVersion}
        <span class="kv-key">Пропущена версия</span><span class="kv-val">{skippedVersion}</span>
      {/if}
    </div>
    <div class="row-start">
      <button
        class="btn-secondary btn-sm"
        disabled={update.checking}
        onclick={() => void checkUpdate(true, true)}
      >
        {update.checking ? 'Проверяю…' : 'Проверить обновления'}
      </button>
      <button class="btn-secondary btn-sm" onclick={openUpdateModal}>
        Подробнее об обновлении
      </button>
    </div>
  </section>

  <section class="pane-section">
    <div class="section-head">
      <div class="card-title">Последние записи</div>
      <button class="btn-secondary btn-sm" onclick={() => void loadJournal(20)}>Обновить</button>
    </div>
    {#if system.journal.length}
      <div class="recordings-list">
        {#each system.journal as s (s.id)}
          <div class="rec-item">
            <div class="rec-info">
              <span class="rec-name">{journalFile(s)}</span>
              <span class="rec-meta">{journalMeta(s)}</span>
            </div>
            <div class="rec-actions">{@html journalBadge(s)}</div>
          </div>
        {/each}
      </div>
      {#if system.journalTruncated}
        <div class="help-text">Показаны последние {system.journal.length} из {system.journalTotal}.</div>
      {/if}
    {:else if snap?.journal?.count}
      <div class="help-text">
        Файлов журнала: {snap.journal.count}
        {#if snap.journal.last_at} · последняя запись {fmtDateTime(snap.journal.last_at)}{/if}
      </div>
    {:else}
      <div class="help-text">Журнал пуст.</div>
    {/if}
  </section>

  <section class="pane-section">
    <div class="card-title">Диагностика</div>
    <div class="help-text">
      Отчёт с версией, разрешениями, устройствами, журналом и хвостом лога.
      Аудио, расшифровок и имён участников в нём нет.
    </div>
    <div class="row-start">
      <button class="btn-secondary btn-sm" onclick={() => void collectDiagnostics()}>
        Собрать отчёт
      </button>
      <button class="btn-secondary btn-sm" onclick={() => void openTarget('diagnostics')}>
        Открыть папку
      </button>
    </div>
  </section>

  <section class="pane-section">
    <div class="section-head">
      <div class="card-title">Сбои</div>
      <button class="btn-secondary btn-sm" disabled={system.crashesLoading} onclick={() => void loadCrashes()}>
        {system.crashesLoading ? 'Загружаю…' : 'Обновить'}
      </button>
    </div>
    {#if lastCrash}
      <div class="rec-item">
        <div class="rec-info">
          <span class="rec-name">
            {lastCrash.synthetic ? 'Завершение без корректного выхода' : (lastCrash.kind || 'Сбой')}
            · {fmtDateTime(lastCrash.created_at) || 'без даты'}
          </span>
          <span class="rec-meta">{crashMessage(lastCrash) || 'Сообщение не записано.'}</span>
          <span class="rec-meta">
            {#if crashVersion(lastCrash)}Версия {crashVersion(lastCrash)}{/if}
            {#if crashHasBacktrace(lastCrash)} · есть стек вызовов{/if}
          </span>
        </div>
        <div class="rec-actions">
          <button class="btn-secondary btn-sm" onclick={() => void ackCrash(lastCrash.id)}>Скрыть</button>
        </div>
      </div>
    {:else if snap?.crashes?.count}
      <div class="help-text">
        Неразобранных сбоев нет. Файлов сбоев: {snap.crashes.count}
        {#if snap.crashes.last_at} · последний {fmtDateTime(snap.crashes.last_at)}{/if}
      </div>
    {:else}
      <div class="help-text">Неразобранных сбоев нет.</div>
    {/if}
  </section>
</div>

<style>
  .system-pane {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .pane-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .section-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
  }
  .kv-grid {
    display: grid;
    grid-template-columns: minmax(120px, max-content) minmax(0, 1fr);
    gap: 3px 14px;
    font-size: 12.5px;
  }
  .kv-key {
    color: var(--text-muted);
  }
  .kv-val {
    color: var(--text-primary);
    word-break: break-word;
  }
  .path-val {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .check-probe {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--text-muted);
    cursor: pointer;
  }
  .check-item {
    align-items: start;
  }
  .check-hint {
    color: var(--text-faint);
  }
  .pane-error {
    color: var(--accent-rec);
  }
  .system-pane :global(.status-badge.error) {
    color: var(--accent-rec);
  }
</style>
