<script lang="ts">
  import { on, selectTab, ui } from '../lib/ui.svelte.ts'
  import { downloadModel, engine, ensureStream } from '../lib/transcript.svelte.ts'
  import {
    createPack,
    deletePack,
    downloadFfmpeg,
    exportPack,
    ffmpeg,
    hasSystemAudio,
    importPack,
    loadFfmpeg,
    loadModels,
    setAutoTranscribe,
    setRecordMode,
    settings,
    statusBadge,
    togglePack,
  } from '../lib/settings.svelte.ts'
  import { isAndroid, isMobile, remainingMb } from '../lib/api'
  import type { ModelInfo } from '../lib/api'
  import { checkUpdate, update } from '../lib/update.svelte.ts'
  import { peopleState } from '../lib/people.svelte.ts'
  import SystemSettingsTab from './SystemSettingsTab.svelte'
  import SummarySettingsTab from './SummarySettingsTab.svelte'
  import { system, runChecks, openTarget } from '../lib/system.svelte.ts'
  import type { DevicesInfo } from '../lib/system.svelte.ts'

  interface Props {
    inline?: boolean
  }
  let { inline = false }: Props = $props()

  let tab = $state<'record' | 'voices' | 'models' | 'summary' | 'system'>('record')
  let packFileInput: HTMLInputElement | null = $state(null)

  function defaultName(devices: DevicesInfo | undefined, id?: string | null): string {
    if (!id) return ''
    const all = [...(devices?.inputs || []), ...(devices?.system || [])]
    return all.find((device) => device.id === id)?.name || id
  }
  const micName = $derived(defaultName(system.snapshot?.devices, system.snapshot?.devices?.default_input))
  const systemAudio = $derived(system.snapshot?.devices?.has_system_audio === true)
  const systemAudioText = $derived(
    systemAudio
      ? 'доступен'
      : settings.platform === 'macos'
        ? 'нужен BlackHole'
        : 'не найден',
  )
  const coreReady = $derived(system.snapshot?.models?.core_ready === true)
  const summaryModels = $derived(settings.models.filter((m) => m.kind === 'summary'))
  const visibleModels = $derived(settings.models.filter((m) => m.kind !== 'summary'))
  const systemModels = $derived(
    (system.snapshot?.models?.items || []).filter((m) => m.kind !== 'summary'),
  )
  const allInstalled = $derived(
    Boolean(
      systemModels.length &&
      systemModels.every((m) => m.is_installed),
    ),
  )
  const modelsBadgeKind = $derived(
    allInstalled ? 'ok' : coreReady ? 'ok' : 'warn',
  )
  const modelsBadgeLabel = $derived(
    allInstalled ? 'Все готовы' : coreReady ? 'Базовые готовы' : 'Не всё',
  )

  async function openModelsTab(): Promise<void> {
    tab = 'models'
  }

  on('models-changed', () => void loadModels())
  on('open-settings', (value) => {
    if (value === 'people') {
      selectTab('people')
      return
    }
    if (value === 'dict') {
      selectTab('dict')
      return
    }
    if (value === 'record' || value === 'voices' || value === 'models' || value === 'system') {
      tab = value
    }
    if (value === 'summary' && settings.summaryFeature === true && !isAndroid) {
      tab = 'summary'
    }
    ui.settingsOpen = true
  })
  // Компонент живёт всё время приложения: одно чтение статуса на старте,
  // дальше состояние обновляет опрос после нажатия «Скачать».
  void loadFfmpeg()

  function onWindowKey(e: KeyboardEvent): void {
    if (!inline && ui.settingsOpen && !ui.donateOpen && e.key === 'Escape') {
      ui.settingsOpen = false
    }
  }
</script>

<svelte:window onkeydown={onWindowKey} />

{#snippet settingsContent()}
  <div class="app-drawer-top">
    <div class="settings-heading">
      <div class="modal-title">Настройки</div>
      <div class="settings-version">
        Версия {update.current || '…'} ·
        <button
          class="link-btn"
          disabled={update.checking}
          onclick={() => void checkUpdate(true, true)}
        >
          {update.checking ? 'Проверяю…' : 'Проверить обновления'}
        </button>
      </div>
    </div>
    {#if !inline}
      <button class="icon-btn" aria-label="Закрыть настройки" onclick={() => (ui.settingsOpen = false)}>✕</button>
    {/if}
  </div>

  <div class="settings-tabs" role="tablist" aria-label="Разделы настроек">
    <button
      class="settings-tab {tab === 'record' ? 'active' : ''}"
      role="tab"
      aria-selected={tab === 'record'}
      onclick={() => (tab = 'record')}
    >
      Запись и звук
    </button>
    <button
      class="settings-tab {tab === 'voices' ? 'active' : ''}"
      role="tab"
      aria-selected={tab === 'voices'}
      onclick={() => (tab = 'voices')}
    >
      Голосовые пакеты
    </button>
    <button
      class="settings-tab {tab === 'models' ? 'active' : ''}"
      role="tab"
      aria-selected={tab === 'models'}
      onclick={() => (tab = 'models')}
    >
      Модели ИИ
    </button>
    {#if settings.summaryFeature === true && !isAndroid}
      <button
        class="settings-tab {tab === 'summary' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'summary'}
        onclick={() => (tab = 'summary')}
      >
        Итоги
      </button>
    {/if}
    <button
      class="settings-tab {tab === 'system' ? 'active' : ''}"
      role="tab"
      aria-selected={tab === 'system'}
      onclick={() => (tab = 'system')}
    >
      Система и логи
    </button>
  </div>

  <div class="app-drawer-body">
      {#if tab === 'record'}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Запись">
          <div class="card-title">Режим записи</div>
          <div class="seg-bar" role="group" aria-label="Режим записи">
            {#if hasSystemAudio()}
              <button
                class="seg-item {settings.recordMode === 'split' ? 'active' : ''}"
                onclick={() => void setRecordMode('split')}
              >
                Встреча · 2 канала
              </button>
            {/if}
            <button
              class="seg-item {settings.recordMode === 'mix' ? 'active' : ''}"
              onclick={() => void setRecordMode('mix')}
            >
              {hasSystemAudio() ? 'Обычная · 1 канал' : 'Микрофон · 1 канал'}
            </button>
          </div>
          <div class="help-text">
            {#if settings.platform === 'macos' && hasSystemAudio() && !settings.systemAudioPermission}
              Разрешите «Запись экрана и системного аудио» в «Системные настройки → Конфиденциальность
              и безопасность», включите Подтекст и перезапустите приложение. Без разрешения
              собеседники не попадут в запись.
            {:else if settings.platform === 'macos' && !hasSystemAudio()}
              Системный звук на macOS не найден. Установите BlackHole 2ch и многовыходное
              устройство (см. README) — или пишите только микрофон в один канал.
            {:else if isMobile}
              На телефоне доступен только микрофон. Запись идёт в один канал; экран можно
              погасить — запись продолжится.
            {:else if settings.recordMode === 'split'}
              Ваш голос и голоса собеседников пишутся в разные каналы. Это работает, только если вы
              в наушниках: при звуке из колонок микрофон слышит собеседников, и приложение само
              оставит один микрофонный канал.
            {:else}
              Все голоса пишутся в один канал. Такую запись удобно слушать, но расшифровка получится
              менее точной.
            {/if}
          </div>

          <label class="setting-check">
            <input
              type="checkbox"
              checked={settings.autoTranscribe}
              onchange={(e) => void setAutoTranscribe(e.currentTarget.checked)}
            />
            <span class="setting-switch" aria-hidden="true"></span>
            <span>Расшифровывать автоматически после записи</span>
          </label>
          <div class="help-text">
            После остановки запись сама уйдёт в расшифровку. Результат появится в «Тексте и спикерах».
          </div>

          <div class="audio-diag-block pt-2">
            <div class="card-title">Состояние звукового тракта</div>
            <div class="system-facts">
              <span class="system-key">Микрофон</span>
              <span class="system-val">{micName || 'не выбран'}</span>
              <span class="system-key">Системный звук</span>
              <span class="system-val">{systemAudioText}</span>
              <span class="system-key">Платформа</span>
              <span class="system-val">{system.snapshot?.devices?.platform || settings.platform || 'Нативная'}</span>
            </div>
            <div class="row-start pt-2">
              {#if isMobile}
                <div class="help-text">
                  На телефоне микрофон проверяется короткой записью на главном экране.
                </div>
              {:else}
                <button
                  class="btn-secondary btn-sm"
                  disabled={system.checksLoading}
                  onclick={() => void runChecks(true)}
                >
                  {system.checksLoading ? 'Проверка…' : 'Проверить микрофон (тест 5 сек)'}
                </button>
              {/if}
            </div>
          </div>
        </div>
      {:else if tab === 'voices'}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Голосовые пакеты">
          <div class="card-title">Голосовые пакеты</div>
          <div class="help-text">
            Наборы голосов и участников: личные, рабочие или проектные. Выключенный пакет не участвует
            в распознавании. Экспорт сохраняет пакет в папку записей, импорт принимает файлы .voicepack.zip и .json.
          </div>
          <div class="row-start">
            <input
              type="text"
              class="input-field"
              placeholder="Название нового пакета"
              autocomplete="off"
              aria-label="Название нового пакета"
              bind:value={settings.newPackName}
              onkeydown={(e) => {
                if (e.key === 'Enter') void createPack()
              }}
            />
            <button class="btn-secondary" onclick={() => void createPack()}>Создать пакет</button>
            <button class="btn-secondary" onclick={() => packFileInput?.click()}>Импортировать</button>
            <input
              type="file"
              accept=".zip,.json"
              hidden
              aria-label="Выбрать файл пакета для импорта"
              bind:this={packFileInput}
              onchange={(e) => {
                const f = e.currentTarget.files?.[0]
                e.currentTarget.value = ''
                if (f) void importPack(f)
              }}
            />
          </div>
          <div class="recordings-list" id="packsListSettings">
            {#if settings.packs.length === 0}
              <div class="empty-note">Пакетов пока нет. Создайте первый кнопкой выше.</div>
            {:else}
              {#each settings.packs as p (p.id)}
                <div class="rec-item">
                  <div class="rec-info">
                    <span class="rec-name">{p.name}</span>
                    <span class="rec-meta">
                      Голосов: {p.voiceprints_count || 0} •
                      <button
                        type="button"
                        class="link-meta-btn"
                        title="Показать участников этого пакета"
                        onclick={() => {
                          selectTab('people')
                          peopleState.selectedPackFilter = p.id
                        }}
                      >
                        Участников: {p.people_count || 0} →
                      </button>
                    </span>
                  </div>
                  <div class="rec-actions">
                    {@html statusBadge(p.enabled ? 'ok' : '', p.enabled ? 'Вкл' : 'Выкл')}
                    <button class="btn-secondary btn-sm" onclick={() => void togglePack(p)}>
                      {p.enabled ? 'Выключить' : 'Включить'}
                    </button>
                    <button class="btn-secondary btn-sm" onclick={() => void exportPack(p)}>
                      Экспорт
                    </button>
                    {#if p.id !== 'default'}
                      <button class="btn-secondary btn-sm" onclick={() => deletePack(p)}>
                        Удалить
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </div>
      {:else if tab === 'models'}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Компоненты">
          <div class="row-between">
            <div class="card-title">Модели и компоненты</div>
            {@html statusBadge(modelsBadgeKind, modelsBadgeLabel)}
          </div>
          <div class="help-text">
            Нейросети работают полностью офлайн на вашем процессоре. Для первой расшифровки достаточно Silero VAD и GigaAM.
          </div>
          <div class="recordings-list" id="modelsListSettings">
            {#if !isMobile && ffmpeg.known && (!ffmpeg.available || ffmpeg.downloading || ffmpeg.error)}
              <div class="rec-item">
                <div class="rec-info">
                  <span class="rec-name">ffmpeg (обработка видео)</span>
                  <span class="rec-meta">
                    Нужен для записи экрана, импорта видео (MP4, MKV, MOV, WEBM) и экспорта MP3.
                    Для записи звука и расшифровки не требуется.
                  </span>
                  {#if ffmpeg.error}
                    <span class="rec-meta">
                      Автоскачивание не удалось ({ffmpeg.error}). Вручную:
                      {#if settings.platform === 'windows'}
                        <a href={ffmpeg.manualUrl} target="_blank" rel="noreferrer">gyan.dev/ffmpeg/builds</a>
                        — положите ffmpeg.exe и ffprobe.exe в
                        %APPDATA%\MeetingRecorder\bin или в PATH.
                      {:else if settings.platform === 'darwin'}
                        brew install ffmpeg
                      {:else}
                        sudo apt install ffmpeg (или dnf/pacman/zypper)
                      {/if}
                    </span>
                  {/if}
                  {#if ffmpeg.downloading}
                    <div
                      class="vu-track progress-track"
                      role="progressbar"
                      aria-valuenow={ffmpeg.pct}
                      aria-label="Прогресс скачивания ffmpeg"
                    >
                      <div class="vu-bar" style={`width: ${ffmpeg.pct}%`}></div>
                    </div>
                  {/if}
                </div>
                <div class="rec-actions">
                  {@html ffmpeg.downloading
                    ? statusBadge('info', `Скачивание ${ffmpeg.pct}%`)
                    : statusBadge('warn', 'Не установлен')}
                  <button
                    class="btn-secondary btn-sm"
                    disabled={ffmpeg.downloading}
                    onclick={() => void downloadFfmpeg()}
                  >
                    Скачать
                  </button>
                </div>
              </div>
            {/if}
            {#if visibleModels.length === 0}
              <div class="empty-note">Не удалось загрузить модели</div>
            {:else}
              {#each visibleModels as m (m.id)}
                {@const sizeMb = remainingMb(m)}
                {@const pct = engine.downloading[m.id]}
                {@const badge = pct !== undefined
                  ? statusBadge('info', `Скачивание ${pct}%`)
                  : m.is_installed
                    ? m.has_update
                      ? statusBadge('warn', `Обновление до v${m.version}`)
                      : statusBadge('ok', `Скачана v${m.installed_version ?? m.version}`)
                    : statusBadge('info', `${sizeMb} МБ`)}
                <div class="rec-item">
                  <div class="rec-info">
                    <span class="rec-name">{m.title}</span>
                    <span class="rec-meta">{m.description || m.id}</span>
                    {#if pct !== undefined}
                      <div class="vu-track progress-track" role="progressbar" aria-valuenow={pct} aria-label={`Прогресс скачивания ${m.title}`}>
                        <div class="vu-bar" style={`width: ${pct}%`}></div>
                      </div>
                    {/if}
                  </div>
                  <div class="rec-actions">
                    {@html badge}
                    {#if !m.is_installed}
                      <button
                        class="btn-secondary btn-sm"
                        disabled={pct !== undefined}
                        onclick={() => void downloadModel(m)}
                      >
                        Скачать
                      </button>
                    {:else if m.has_update}
                      <button
                        class="btn-primary btn-sm"
                        disabled={pct !== undefined}
                        onclick={() => void downloadModel(m)}
                      >
                        Обновить
                      </button>
                    {/if}
                  </div>
                </div>
              {/each}
            {/if}
          </div>
          {#if summaryModels.length > 0 && settings.summaryFeature === true && !isAndroid}
            <div class="help-text">
              Модель итогов — в разделе
              <button class="link-btn" onclick={() => (tab = 'summary')}>«Итоги»</button>.
            </div>
          {/if}
          <div class="row-start">
            <button class="btn-secondary" onclick={() => void loadModels()}>Обновить список</button>
            {#if !isMobile}
              <button class="btn-secondary" onclick={() => void openTarget('models')}>
                Открыть папку моделей
              </button>
            {/if}
          </div>
        </div>
      {:else if tab === 'summary' && settings.summaryFeature === true && !isAndroid}
        <SummarySettingsTab />
      {:else if tab === 'system'}
        <SystemSettingsTab />
      {/if}
    </div>

    <div class="app-drawer-bottom">
      <button class="support-link" onclick={() => (ui.donateOpen = true)}>Поддержать проект</button>
      {#if !inline}
        <button class="btn-primary" onclick={() => (ui.settingsOpen = false)}>Готово</button>
      {/if}
    </div>
{/snippet}

{#if inline}
  <div class="settings-inline-wrap">
    {@render settingsContent()}
  </div>
{:else if ui.settingsOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="app-drawer-overlay"
    role="presentation"
    onclick={() => (ui.settingsOpen = false)}
  ></div>
  <div
    class="app-drawer settings-drawer"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Настройки"
  >
    {@render settingsContent()}
  </div>
{/if}

<style>
  .settings-inline-wrap {
    display: flex;
    flex-direction: column;
    gap: 14px;
    width: 100%;
    min-width: 0;
  }
  .settings-inline-wrap :global(.app-drawer-top) {
    padding: 0 0 14px;
  }
  .settings-inline-wrap :global(.app-drawer-body) {
    overflow: visible;
    padding: 6px 0;
  }
  .settings-inline-wrap :global(.app-drawer-bottom) {
    padding: 14px 0 0;
    border-top: 1px solid var(--border-soft);
    background: transparent;
  }
  .settings-heading {
    display: flex; flex-direction: column; gap: 1px; min-width: 0;
  }
  .settings-version {
    font-size: 11.5px; color: var(--text-faint);
    display: flex; align-items: center; gap: 4px; flex-wrap: wrap;
  }
  .support-link {
    margin-right: auto;
    border: none; background: none; padding: 6px 0;
    font-family: inherit; font-size: 12px; font-weight: 600;
    color: var(--text-faint); cursor: pointer;
    text-decoration: underline; text-underline-offset: 3px;
  }
  .support-link:hover { color: var(--text-muted); }
  .settings-tabs {
    display: flex;
    gap: 6px;
    border-bottom: 1px solid var(--border-soft);
    padding: 8px 16px;
    background: var(--bg-inset);
    flex-shrink: 0;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none;
    -ms-overflow-style: none;
  }
  .settings-tabs::-webkit-scrollbar {
    display: none;
  }
  @media (max-width: 640px) {
    .settings-tabs {
      padding: 6px 12px;
      gap: 4px;
    }
  }
  .settings-tab {
    border: 1px solid transparent;
    background: transparent;
    cursor: pointer;
    padding: 6px 12px;
    border-radius: var(--radius-inner);
    font-family: inherit;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-muted);
    white-space: nowrap;
    flex-shrink: 0;
    transition: all 160ms var(--ease-spring);
  }
  .settings-tab:hover {
    color: var(--text-primary);
    background: rgba(15, 30, 51, 0.04);
  }
  .settings-tab.active {
    color: var(--text-primary);
    background: #fff;
    border-color: var(--border-card);
    box-shadow: 0 1px 2px rgba(15, 30, 51, 0.06);
  }
  .settings-pane {
    display: flex; flex-direction: column; gap: 14px;
    padding-top: 2px;
  }
  .link-meta-btn {
    border: none;
    background: transparent;
    padding: 0;
    font: inherit;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--accent-blue);
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .link-meta-btn:hover {
    color: #1a4299;
  }
  .audio-diag-block {
    border-top: 1px solid var(--border-soft);
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-top: 14px;
    margin-top: 6px;
  }
  .system-facts {
    display: grid;
    grid-template-columns: minmax(100px, max-content) minmax(0, 1fr);
    gap: 4px 12px;
    font-size: 12.5px;
  }
  .system-key {
    color: var(--text-muted);
  }
  .system-val {
    color: var(--text-primary);
    word-break: break-word;
  }
  .pt-2 {
    padding-top: 8px;
  }
</style>
