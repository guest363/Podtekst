<script lang="ts">
  import { toast } from '../lib/toast.svelte.ts'
  import { on, ui } from '../lib/ui.svelte.ts'
  import { downloadModel, engine, ensureStream } from '../lib/transcript.svelte.ts'
  import {
    createPack,
    downloadFfmpeg,
    exportPack,
    ffmpeg,
    hasSystemAudio,
    loadFfmpeg,
    loadModels,
    setAutoTranscribe,
    setRecordMode,
    settings,
    statusBadge,
    togglePack,
  } from '../lib/settings.svelte.ts'
  import { isMobile, readJsonSafe, remainingMb } from '../lib/api'
  import type { ModelInfo } from '../lib/api'
  import { checkUpdate, update } from '../lib/update.svelte.ts'
  import People from './People.svelte'

  let tab = $state<'record' | 'people' | 'voices' | 'models'>('record')

  on('models-changed', () => void loadModels())
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
        class="settings-tab {tab === 'models' ? 'active' : ''}"
        role="tab"
        aria-selected={tab === 'models'}
        onclick={() => (tab = 'models')}
      >
        Модели
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
            {#if settings.platform === 'macos' && !hasSystemAudio()}
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
            Базы голосов команд: личные, рабочие, гостевые. Отключённый пакет исключается из
            распознавания.
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
                      Голосов: {p.voiceprints_count || 0} • Людей: {p.people_count || 0}
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
                  </div>
                </div>
              {/each}
            {/if}
          </div>
        </div>
      {:else}
        <div class="settings-pane" role="tabpanel" tabindex="0" aria-label="Модели ИИ">
          <div class="card-title">Модели ИИ</div>
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
                  {#if settings.platform === 'windows'}
                    {#if ffmpeg.error}
                      <span class="rec-meta">
                        Автоскачивание не удалось ({ffmpeg.error}). Установите вручную:
                        <a href={ffmpeg.manualUrl} target="_blank" rel="noreferrer">gyan.dev/ffmpeg/builds</a>
                        — положите ffmpeg.exe и ffprobe.exe в
                        %APPDATA%\MeetingRecorder\bin или в PATH.
                      </span>
                    {/if}
                  {:else}
                    <!-- Автоскачивание сделано только для Windows: на других ОС — пакетный менеджер -->
                    <span class="rec-meta">
                      {settings.platform === 'darwin'
                        ? 'Установите ffmpeg через Homebrew: brew install ffmpeg'
                        : 'Установите ffmpeg из репозитория дистрибутива: sudo apt install ffmpeg (или dnf/pacman/zypper)'}
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
                  {#if settings.platform === 'windows'}
                    <button
                      class="btn-secondary btn-sm"
                      disabled={ffmpeg.downloading}
                      onclick={() => void downloadFfmpeg()}
                    >
                      Скачать
                    </button>
                  {/if}
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
  .settings-tabs {
    display: flex; gap: 4px;
    border-bottom: 1px solid var(--border-soft);
    padding: 0 20px;
    background: #fff;
    flex-shrink: 0;
    overflow: hidden;
  }
  @media (max-width: 640px) {
    .settings-tabs {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 0;
      padding: 0 10px;
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
    transition: color 180ms var(--ease-spring), border-color 180ms var(--ease-spring);
  }
  .settings-tab:hover { color: var(--text-primary); }
  .settings-tab.active { color: var(--text-primary); border-bottom-color: var(--accent-blue); }
  .settings-pane {
    display: flex; flex-direction: column; gap: 14px;
    padding-top: 2px;
  }
  .setting-check {
    display: flex; align-items: center; gap: 10px;
    font-size: 12.5px; font-weight: 700; color: var(--text-primary);
    cursor: pointer; padding-top: 4px;
  }
  .setting-check input {
    position: absolute; opacity: 0; width: 1px; height: 1px;
    pointer-events: none;
  }
  .setting-switch {
    position: relative; flex-shrink: 0;
    width: 36px; height: 21px; border-radius: 999px;
    background: #c3cedb;
    transition: background 180ms var(--ease-spring);
  }
  .setting-switch::after {
    content: ""; position: absolute; top: 3px; left: 3px;
    width: 15px; height: 15px; border-radius: 50%;
    background: #fff; box-shadow: 0 1px 3px rgba(15, 30, 51, 0.25);
    transition: transform 180ms var(--ease-spring);
  }
  .setting-check input:checked + .setting-switch { background: var(--accent-ok); }
  .setting-check input:checked + .setting-switch::after { transform: translateX(15px); }
  .setting-check input:focus-visible + .setting-switch {
    outline: 2px solid var(--accent-blue); outline-offset: 2px;
  }
</style>
