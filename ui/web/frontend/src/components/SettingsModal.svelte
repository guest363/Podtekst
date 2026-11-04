<script lang="ts">
  import { toast } from '../lib/toast.svelte.ts'
  import { on, ui } from '../lib/ui.svelte.ts'
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
    loadHotwords,
    loadModels,
    saveHotwords,
    setAutoTranscribe,
    setRecordMode,
    settings,
    statusBadge,
    togglePack,
  } from '../lib/settings.svelte.ts'
  import { isMobile, remainingMb } from '../lib/api'
  import type { ModelInfo } from '../lib/api'
  import { checkUpdate, update } from '../lib/update.svelte.ts'
  import { peopleState } from '../lib/people.svelte.ts'
  import People from './People.svelte'

  let tab = $state<'record' | 'people' | 'voices' | 'dict' | 'models'>('record')
  let packFileInput: HTMLInputElement | null = $state(null)
  let termInput = $state('')

  const filteredHotwords = $derived.by(() => {
    const q = termInput.trim().toLowerCase()
    if (!q) return settings.hotwords
    const norm = q.replace(/\s*(=>|->|=)\s*/, ' => ')
    return settings.hotwords.filter(
      (x) => x.toLowerCase().includes(q) || x.toLowerCase().includes(norm),
    )
  })

  const isExactMatch = $derived.by(() => {
    const q = termInput.trim().toLowerCase()
    if (!q) return false
    const norm = q.replace(/\s*(=>|->|=)\s*/, ' => ')
    return settings.hotwords.some(
      (x) => x.toLowerCase() === q || x.toLowerCase() === norm,
    )
  })

  async function openModelsTab(): Promise<void> {
    tab = 'models'
  }

  async function openDictTab(): Promise<void> {
    tab = 'dict'
    if (!settings.hotwordsLoaded) await loadHotwords()
  }

  async function addTerm(): Promise<void> {
    let t = termInput.trim()
    if (!t) return
    // Нормализация разделителей: «было = стало» или «было -> стало» преобразуем в «было => стало»
    if (t.includes('=')) {
      t = t.replace(/\s*=>\s*|\s*=\s*/, ' => ')
    } else if (t.includes('->')) {
      t = t.replace(/\s*->\s*/, ' => ')
    }
    if (settings.hotwords.some((x) => x.toLowerCase() === t.toLowerCase())) {
      toast(`«${t}» уже в словаре`)
      return
    }
    if (settings.hotwords.length >= 500) {
      toast('Словарь полон: 500 из 500. Удалите лишнее.', 'error')
      return
    }
    termInput = ''
    await saveHotwords([t, ...settings.hotwords], settings.ctcEnabled)
  }

  async function removeTerm(t: string): Promise<void> {
    await saveHotwords(
      settings.hotwords.filter((x) => x !== t),
      settings.ctcEnabled,
    )
  }

  async function setCtcEnabled(enabled: boolean): Promise<void> {
    settings.ctcEnabled = enabled
    await saveHotwords(settings.hotwords, enabled)
  }

  on('models-changed', () => void loadModels())
  on('open-settings', (value) => {
    if (value === 'record' || value === 'people' || value === 'voices' || value === 'dict' || value === 'models') {
      tab = value
    }
    ui.settingsOpen = true
  })
  // Компонент живёт всё время приложения: одно чтение статуса на старте,
  // дальше состояние обновляет опрос после нажатия «Скачать».
  void loadFfmpeg()

  function onWindowKey(e: KeyboardEvent): void {
    if (ui.settingsOpen && !ui.donateOpen && e.key === 'Escape') {
      ui.settingsOpen = false
    }
  }
</script>

<svelte:window onkeydown={onWindowKey} />

{#if ui.settingsOpen}
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
          ·
          <button class="link-btn" onclick={() => (ui.systemOpen = true)}>Состояние</button>
        </div>
      </div>
      <button class="icon-btn" aria-label="Закрыть настройки" onclick={() => (ui.settingsOpen = false)}>✕</button>
    </div>

    <div class="settings-tabs" role="tablist" aria-label="Разделы настроек">
      <button
        class="settings-tab {tab === 'record' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'record'}
        onclick={() => (tab = 'record')}
      >
        Запись
      </button>
      <button
        class="settings-tab {tab === 'people' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'people'}
        onclick={() => (tab = 'people')}
      >
        Участники
      </button>
      <button
        class="settings-tab {tab === 'voices' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'voices'}
        onclick={() => (tab = 'voices')}
      >
        Голоса
      </button>
      <button
        class="settings-tab {tab === 'dict' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'dict'}
        onclick={() => void openDictTab()}
      >
        Словарь
      </button>
      <button
        class="settings-tab {tab === 'models' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'models'}
        onclick={() => void openModelsTab()}
      >
        Компоненты
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
              Ваш голос и голоса собеседников пишутся в разные каналы: слова не накладываются друг на
              друга при расшифровке.
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
        </div>
      {:else if tab === 'people'}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Участники">
          <People />
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
                          tab = 'people'
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
          <div class="card-title">Модели и компоненты</div>
          <div class="help-text">
            Локальные модели для распознавания речи: GigaAM (русский язык), Whisper (30+ языков),
            Silero VAD (детектор речи) и ERes2NetV2 (разделение голосов). Для первой
            расшифровки скачайте Silero VAD и одну модель распознавания.
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
            {#if settings.models.length === 0}
              <div class="empty-note">Не удалось загрузить модели</div>
            {:else}
              {#each settings.models as m (m.id)}
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
                    {#if m.license}
                      <span class="rec-meta">Лицензия: {m.license}</span>
                    {/if}
                    {#if pct !== undefined}
                      <div class="vu-track progress-track" role="progressbar" aria-valuenow={pct} aria-label={`Прогресс скачивания ${m.title}`}>
                        <div class="vu-bar" style={`width: ${pct}%`}></div>
                      </div>
                    {/if}
                  </div>
                  <div class="rec-actions">
                    {@html badge}
                    <button
                      class="btn-secondary btn-sm"
                      disabled={pct !== undefined}
                      onclick={() => void downloadModel(m)}
                    >
                      {m.is_installed ? 'Обновить' : 'Скачать'}
                    </button>
                  </div>
                </div>
              {/each}
            {/if}
          </div>
          <div class="row-start">
            <button class="btn-secondary" onclick={() => void loadModels()}>Обновить список</button>
          </div>
        </div>
      {:else if tab === 'dict'}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Словарь терминов">
          <div class="dict-head">
            <div class="card-title">Словарь терминов</div>
            <span class="rec-meta">{settings.hotwords.length} из 500</span>
          </div>
          <div class="help-text">
            Подсказки на слух отдают предпочтение терминам, именам и сленгу.
            Автозамена («было = стало» или «было -> стало») исправляет частые ошибки распознавания.
          </div>
          <div class="dict-input-wrap">
            <input
              type="text"
              class="input-field dict-input"
              placeholder="Поиск или новый термин (напр. «Docker» или «мерши = merge»)"
              autocomplete="off"
              aria-label="Поиск или новый термин"
              bind:value={termInput}
              onkeydown={(e) => {
                if (e.key === 'Enter') void addTerm()
                else if (e.key === 'Escape' && termInput) {
                  e.preventDefault()
                  termInput = ''
                }
              }}
            />
            {#if termInput}
              <button
                type="button"
                class="dict-input-clear"
                title="Очистить поиск (Esc)"
                aria-label="Очистить поиск"
                onclick={() => (termInput = '')}
              >✕</button>
            {/if}
            <button
              class="btn-secondary"
              disabled={!termInput.trim() || isExactMatch}
              onclick={() => void addTerm()}
            >Добавить</button>
          </div>

          {#if termInput.trim() && !isExactMatch}
            {@const preview = termInput.trim().replace(/\s*(=>|->|=)\s*/, ' → ')}
            <div class="dict-add-bar">
              Нажмите <strong>Enter</strong>, чтобы добавить «{preview}»
            </div>
          {/if}

          {#if filteredHotwords.length > 0}
            <div class="dict-list" role="list" aria-label="Термины в словаре">
              {#each filteredHotwords as t (t)}
                {@const isPair = t.includes(' => ')}
                <div class="dict-row" role="listitem">
                  <div class="dict-row-text">
                    {#if isPair}
                      {@const [src, dst] = t.split(' => ')}
                      <span class="dict-heard">{src}</span>
                      <span class="dict-arrow">→</span>
                      <span class="dict-correct">{dst}</span>
                    {:else}
                      <span class="dict-word">{t}</span>
                    {/if}
                  </div>
                  <div class="dict-row-actions">
                    <span class="dict-kind {isPair ? 'dict-kind-sub' : 'dict-kind-word'}">
                      {isPair ? 'замена' : 'на слух'}
                    </span>
                    <button
                      type="button"
                      class="dict-del"
                      title={`Убрать «${t}» из словаря`}
                      aria-label={`Убрать «${t}» из словаря`}
                      onclick={() => void removeTerm(t)}
                    >✕</button>
                  </div>
                </div>
              {/each}
            </div>
          {:else if termInput.trim()}
            <div class="empty-note">Ничего не найдено. Нажмите Enter или «Добавить», чтобы создать.</div>
          {:else}
            <div class="empty-note">Пока пусто. Добавьте первый термин полем выше.</div>
          {/if}

          <label class="setting-check">
            <input
              type="checkbox"
              checked={settings.ctcEnabled}
              onchange={(e) => void setCtcEnabled(e.currentTarget.checked)}
            />
            <span class="setting-switch" aria-hidden="true"></span>
            <span>Второй проход (уверенность слов + словарь)</span>
          </label>
          <div class="help-text">
            Требует модель «GigaAM-v3 e2e CTC int8» из вкладки «Компоненты».
            Без неё второй проход пропускается.
          </div>
        </div>
      {/if}
    </div>

    <div class="app-drawer-bottom">
      <button class="support-link" onclick={() => (ui.donateOpen = true)}>Поддержать проект</button>
      <button class="btn-primary" onclick={() => (ui.settingsOpen = false)}>Готово</button>
    </div>
  </div>
{/if}

<style>
  .settings-heading {
    display: flex; flex-direction: column; gap: 1px; min-width: 0;
  }
  .settings-version {
    font-size: 11.5px; color: var(--text-faint);
    display: flex; align-items: center; gap: 4px; flex-wrap: wrap;
  }
  .link-btn {
    border: none; background: none; padding: 0;
    font: inherit; color: var(--accent-blue); cursor: pointer;
    text-decoration: underline; text-underline-offset: 2px;
  }
  .link-btn:hover { color: var(--text-primary); }
  .link-btn:disabled { color: var(--text-faint); cursor: wait; text-decoration: none; }
  .support-link {
    margin-right: auto;
    border: none; background: none; padding: 6px 0;
    font-family: inherit; font-size: 12px; font-weight: 600;
    color: var(--text-faint); cursor: pointer;
    text-decoration: underline; text-underline-offset: 3px;
  }
  .support-link:hover { color: var(--text-muted); }
  .dict-head {
    display: flex; justify-content: space-between; align-items: baseline; gap: 8px;
  }
  .dict-input-wrap {
    position: relative; display: flex; gap: 8px; align-items: center;
  }
  .dict-input {
    flex: 1; padding-right: 32px;
  }
  .dict-input-clear {
    position: absolute; right: 104px; top: 50%; transform: translateY(-50%);
    border: none; background: transparent; cursor: pointer;
    color: var(--text-faint); font-size: 13px; line-height: 1; padding: 4px;
    border-radius: 4px;
  }
  .dict-input-clear:hover { color: var(--text-primary); }
  .dict-add-bar {
    font-size: 12px; color: var(--accent-blue);
    background: var(--bg-inset); border: 1px solid var(--border-soft);
    border-radius: var(--radius-sm); padding: 7px 12px; margin-top: -4px;
  }
  .dict-list {
    display: flex; flex-direction: column;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    max-height: 290px;
    overflow-y: auto;
  }
  .dict-row {
    display: flex; justify-content: space-between; align-items: center;
    padding: 7px 12px; gap: 10px; font-size: 13px;
    border-bottom: 1px solid var(--border-soft);
    transition: background 120ms ease;
  }
  .dict-row:last-child { border-bottom: none; }
  .dict-row:hover { background: var(--bg-inset); }
  .dict-row-text {
    display: flex; align-items: center; gap: 6px; min-width: 0; overflow: hidden;
  }
  .dict-word {
    font-weight: 600; color: var(--text-primary); word-break: break-word;
  }
  .dict-heard {
    color: var(--text-muted); text-decoration: line-through; text-decoration-color: var(--text-faint);
  }
  .dict-arrow {
    color: var(--text-faint); font-weight: 700; user-select: none;
  }
  .dict-correct {
    font-weight: 700; color: var(--text-primary); word-break: break-word;
  }
  .dict-row-actions {
    display: flex; align-items: center; gap: 8px; flex-shrink: 0;
  }
  .dict-kind {
    font-size: 10.5px; font-weight: 700; padding: 2px 7px;
    border-radius: var(--radius-pill); border: 1px solid transparent; letter-spacing: 0.02em;
  }
  .dict-kind-word {
    color: var(--accent-blue); background: #f0f4ff; border-color: #dbe4fe;
  }
  .dict-kind-sub {
    color: var(--accent-amber); background: var(--accent-amber-bg); border-color: #f6e6be;
  }
  .dict-del {
    border: none; background: none; cursor: pointer;
    color: var(--text-faint); font-size: 12px; line-height: 1;
    padding: 3px 5px; border-radius: 4px;
    transition: color 120ms ease, background 120ms ease;
  }
  .dict-del:hover { color: var(--accent-rec); background: #fee2e2; }
  .settings-tabs {
    display: flex; gap: 4px;
    border-bottom: 1px solid var(--border-soft);
    padding: 0 20px;
    background: #fff;
    flex-shrink: 0;
    overflow-x: auto;
    overflow-y: hidden;
  }
  @media (max-width: 640px) {
    .settings-tabs {
      display: flex;
      gap: 0;
      padding: 0 10px;
      overflow-x: auto;
      overflow-y: hidden;
    }
  }
  .settings-tab {
    border: none; background: transparent; cursor: pointer;
    padding: 9px 14px 10px;
    font-family: inherit; font-size: 13px; font-weight: 700;
    color: var(--text-muted);
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    white-space: nowrap;
    flex-shrink: 0;
    transition: color 180ms var(--ease-spring), border-color 180ms var(--ease-spring);
  }
  .settings-tab:hover { color: var(--text-primary); }
  .settings-tab.active { color: var(--text-primary); border-bottom-color: var(--accent-blue); }
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
</style>
