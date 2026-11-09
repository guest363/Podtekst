<script lang="ts">
import { onMount } from 'svelte'
import type { OkResponse } from '../lib/api'
import { baseName, isMobile, readJsonSafe } from '../lib/api'
import { errText } from '../lib/guards.ts'
import {
  isScreenCaptureSupported,
  screenRecorder,
  toggleScreenCapture,
} from '../lib/screen-recorder.svelte.ts'
import { hasSystemAudio, settings } from '../lib/settings.svelte.ts'
import { toast } from '../lib/toast.svelte.ts'
import { engine, syncEngineFromStatus } from '../lib/transcript.svelte.ts'
import { emit, openTranscript, ui } from '../lib/ui.svelte.ts'

type RecordStartResponse = OkResponse & {
  mic_muted?: boolean
  sys_error?: string
  screen_error?: string
}
type RecordStopResponse = OkResponse & {
  is_video?: boolean
  filename?: string
  file?: string
  silent?: boolean
  sys_silent?: boolean
}
type StatusResponse = {
  mic_level?: number
  sys_level?: number
  transcribing?: boolean
  transcribing_file?: string
  summarizing?: boolean
  summarizing_file?: string
  summary_phase?: string
  summary_pct?: number
  screen_recording?: boolean
  screen_supported?: boolean
  system_audio_permission?: boolean
  recording?: boolean
  elapsed?: number
  device?: { mic_name?: string; mic_muted?: boolean }
}

let isRecording = $state(false)
let screenSupported = $state(false)
// /api/status отдаёт std::env::consts::OS: на macOS это "macos", но часть
// старых проверок в UI использует "darwin" — поддерживаем оба значения.
let systemAudioPermission = $state(true)
let timerText = $state('00:00:00')
let micName = $state('Определение…')
let micMuted = $state(false)
let micRms = $state(0)
let sysRms = $state(0)

let micDb = $derived(micRms > 0.001 ? Math.max(-60, Math.round(20 * Math.log10(micRms))) : -60)
let sysDb = $derived(sysRms > 0.001 ? Math.max(-60, Math.round(20 * Math.log10(sysRms))) : -60)

let busyRunning = $derived(engine.running || engine.summaryRunning)
let busyPct = $derived(engine.running ? engine.pct : engine.summaryPct)
let busyMsg = $derived(engine.running ? engine.msg : engine.summaryMsg || 'Собираю итоги…')
let busyLabel = $derived(engine.running ? 'Ход расшифровки' : 'Ход сборки итогов')
let busyAria = $derived(engine.running ? 'Прогресс расшифровки' : 'Прогресс сборки итогов')
let busyFile = $derived(
  engine.running ? engine.file : engine.summaryRunning ? engine.summaryFile : '',
)

const hotkeyLabel = /Mac/i.test(navigator.userAgent) ? 'Cmd+Shift+R' : 'Ctrl+Shift+R'

const WAVE_BARS_COUNT = 44
const waveDelays = Array.from({ length: WAVE_BARS_COUNT }, (_, i) => {
  const distFromCenter = Math.abs(i - (WAVE_BARS_COUNT - 1) / 2)
  return (distFromCenter * 0.04).toFixed(2)
})

let startTime = 0
let timerInterval: ReturnType<typeof setInterval> | null = null

const setRecordingState = (rec: boolean, initialElapsed = 0): void => {
  isRecording = rec
  if (rec) {
    startTime = Date.now() - initialElapsed * 1000
    if (timerInterval) clearInterval(timerInterval)
    timerInterval = setInterval(() => {
      const sec = Math.floor((Date.now() - startTime) / 1000)
      const h = String(Math.floor(sec / 3600)).padStart(2, '0')
      const m = String(Math.floor((sec % 3600) / 60)).padStart(2, '0')
      const s = String(sec % 60).padStart(2, '0')
      timerText = `${h}:${m}:${s}`
    }, 500)
  } else {
    if (timerInterval) clearInterval(timerInterval)
    timerText = '00:00:00'
  }
}

let lastToggleTime = 0

const toggleRecord = async (): Promise<void> => {
  const now = Date.now()
  if (now - lastToggleTime < 500) return
  lastToggleTime = now
  try {
    if (!isRecording) {
      const wantScreen = screenRecorder.enabled && screenSupported
      const r = await fetch('/api/record/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          mode: settings.recordMode,
          screen: wantScreen,
        }),
      })
      const d = await readJsonSafe<RecordStartResponse>(r)
      if (d.ok) {
        setRecordingState(true)
        if (wantScreen) {
          screenRecorder.active = true
          toast('Запись экрана и звука началась')
        }
        if (d.mic_muted) toast('Микрофон выключен в системе – будет тишина', 'error')
        if (d.sys_error) toast(`Системный звук не пишется: ${d.sys_error}`, 'error')
        if (d.screen_error) toast(`Экран не записывается: ${d.screen_error}`, 'error')
      } else if (d.error === 'Запись уже идёт') {
        setRecordingState(true)
      } else {
        toast(d.error || 'Не удалось начать запись', 'error')
      }
    } else {
      if (timerInterval) clearInterval(timerInterval)

      const r = await fetch('/api/record/stop', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ defer_autotranscribe: false }),
      })
      const d = await readJsonSafe<RecordStopResponse>(r)
      setRecordingState(false)
      screenRecorder.active = false
      emit('recordings-changed')

      if (d.ok) {
        if (d.is_video) {
          toast(`Видеозапись сохранена: ${d.filename || baseName(d.file || '')}`)
        } else if (d.silent) {
          toast(
            'Записана тишина – микрофон ничего не записал. Проверьте звук и источник в системе',
            'error',
          )
        } else if (d.sys_silent) {
          toast(
            'Системный звук не записался — проверьте разрешение «Запись экрана и системного аудио»',
            'error',
          )
        } else if (d.filename) {
          toast(`Запись сохранена: ${d.filename}`)
        }
      } else if (d.error && d.error !== 'Запись не активна') {
        toast(d.error || 'Не удалось остановить запись', 'error')
      }
    }
  } catch (e) {
    toast(`Ошибка записи: ${errText(e)}`, 'error')
  }
}

const pollLevels = async (): Promise<void> => {
  try {
    const r = await fetch('/api/status')
    const d = await readJsonSafe<StatusResponse>(r)
    // tail_level_01 уже RMS с запасом ×3; приводим к 0…1.
    micRms = Math.min(1, (d.mic_level || 0) / 3)
    sysRms = Math.min(1, (d.sys_level || 0) / 3)

    // Движок един на всё приложение: опрос — запасной канал правды о нём
    // (расшифровку могли запустить авто-режимом, CLI или IPC мимо UI).
    syncEngineFromStatus(!!d.transcribing, String(d.transcribing_file || ''), {
      active: d.summarizing === true,
      file: String(d.summarizing_file || ''),
      phase: String(d.summary_phase || ''),
      pct: Number(d.summary_pct) || 0,
    })
    if (typeof d.screen_recording === 'boolean') {
      screenRecorder.active = d.screen_recording
    }
    if (typeof d.screen_supported === 'boolean') {
      screenSupported = d.screen_supported && !isMobile
    }
    if (typeof d.system_audio_permission === 'boolean') {
      systemAudioPermission = d.system_audio_permission
      settings.systemAudioPermission = d.system_audio_permission
    }
    // Запись могли включить/выключить извне (глобальный хоткей, IPC,
    // второй клик по ярлыку) — кнопка обязана следовать за реальностью.
    if (d.recording && !isRecording) {
      setRecordingState(true, d.elapsed || 0)
    } else if (!d.recording && isRecording) {
      setRecordingState(false)
      screenRecorder.active = false
      emit('recordings-changed')
    }
    if (d.device) {
      micName = d.device.mic_name || 'Недоступен'
      micMuted = !!d.device.mic_muted
    } else {
      micName = 'Недоступен'
    }
  } catch {}
}

// Хоткей внутри окна: глобальный граб не работает на Wayland при фокусе
// окна, а в браузере (--web) Ctrl+Shift+R — это hard-reload. Ловим сами.
const onWindowHotkey = (e: KeyboardEvent): void => {
  if (e.repeat) return
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.code === 'KeyR') {
    e.preventDefault()
    void toggleRecord()
  }
}

onMount(() => {
  screenSupported = isScreenCaptureSupported()
  const statusTimer = setInterval(pollLevels, 400)
  void pollLevels()
  return () => {
    clearInterval(statusTimer)
    if (timerInterval) clearInterval(timerInterval)
  }
})
</script>

<svelte:window onkeydown={onWindowHotkey} />

<section class="studio-deck" aria-label="Запись">
  <div class="deck-shell">
    <div class="deck-top">
      <span
        class="deck-status {isRecording ? 'recording' : ''}"
        role="status"
        title={busyFile || undefined}
      >
        <span class="status-dot {isRecording ? 'active' : ''}" aria-hidden="true"></span>
        <span class="deck-status-content">
          {#if isRecording}
            {screenRecorder.active ? 'Идёт запись экрана и звука' : 'Идёт запись встречи'}
          {:else if engine.running}
            Расшифровка {engine.pct}%{#if engine.file} · <button
              type="button"
              class="deck-file-link"
              onclick={() => openTranscript(engine.file)}
              title="Перейти к расшифровке"
            >{baseName(engine.file)}</button>{/if}
          {:else if engine.summaryRunning}
            Сборка итогов {engine.summaryPct}%{#if engine.summaryFile} · <button
              type="button"
              class="deck-file-link"
              onclick={() => openTranscript(engine.summaryFile)}
              title="Перейти к расшифровке"
            >{baseName(engine.summaryFile)}</button>{/if}
          {:else if Object.keys(engine.downloading).length > 0}
            {@const activeId = Object.keys(engine.downloading)[0]}
            {@const activePct = engine.downloading[activeId]}
            Скачивание ИИ: {activePct}%
          {:else}
            Готов к записи
          {/if}
        </span>
      </span>

      <div class="deck-chips">
        {#if !isMobile && screenSupported}
          <button
            type="button"
            class="clean-chip {screenRecorder.enabled ? 'active' : ''}"
            disabled={isRecording}
            onclick={() => {
              toggleScreenCapture()
            }}
            title={isRecording
              ? (screenRecorder.active ? 'Запись экрана активна' : 'Запись экрана не активна')
              : (screenRecorder.enabled
                ? 'Запись экрана включена (нажмите, чтобы выключить)'
                : 'Включить запись экрана')}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2"></rect>
              <line x1="8" y1="21" x2="16" y2="21"></line>
              <line x1="12" y1="17" x2="12" y2="21"></line>
            </svg>
            <span>
              {#if isRecording && screenRecorder.active}
                Экран включен
              {:else if screenRecorder.enabled}
                Экран включен
              {:else}
                ＋ Экран
              {/if}
            </span>
          </button>
        {:else if settings.platform === 'windows' || settings.platform === 'darwin' || settings.platform === 'macos'}
          <button
            type="button"
            class="clean-chip"
            onclick={() => {
              ui.settingsOpen = true
            }}
            title="ffmpeg нужен для записи экрана — установите его в Настройках → Компоненты"
          >
            <span>Запись экрана: нужен ffmpeg</span>
          </button>
        {/if}

        {#if (settings.platform === 'darwin' || settings.platform === 'macos') && !systemAudioPermission}
          <div
            class="clean-chip static warn"
            title="Системные настройки → Конфиденциальность и безопасность → Запись экрана и системного аудио. Включите Подтекст и перезапустите приложение. Если доступ уже включён — удалите Подтекст из списка («−») и выдайте заново"
          >
            <span>Системный звук: нужно разрешение</span>
          </div>
        {/if}

        {#if !isMobile}
          <div
            class="clean-chip static {micMuted ? 'warn' : ''}"
            title={micMuted ? 'Микрофон выключен в системе' : `Микрофон: ${micName}`}
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/>
              <path d="M19 10v2a7 7 0 0 1-14 0v-2"/>
              <line x1="12" y1="19" x2="12" y2="22"/>
            </svg>
            <span class="mic-name-text">{micMuted ? 'Микрофон заглушен' : micName}</span>
          </div>
        {/if}
      </div>
    </div>

    {#if busyRunning}
      {#if busyFile}
        <button
          type="button"
          class="deck-transcribe clickable"
          aria-label={busyLabel}
          onclick={() => openTranscript(busyFile)}
          title={`Перейти к записи: ${baseName(busyFile)}`}
        >
          <div
            class="progress-track"
            role="progressbar"
            aria-valuenow={busyPct}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-label={busyAria}
          >
            <div class="vu-bar" style={`width: ${busyPct}%`}></div>
          </div>
          <div class="deck-transcribe-meta">
            <span class="deck-transcribe-msg">{busyMsg}</span>
            <span class="deck-transcribe-action">
              <span>Открыть</span>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <polyline points="9 18 15 12 9 6"></polyline>
              </svg>
            </span>
          </div>
        </button>
      {:else}
        <div class="deck-transcribe" aria-label={busyLabel}>
          <div
            class="progress-track"
            role="progressbar"
            aria-valuenow={busyPct}
            aria-valuemin="0"
            aria-valuemax="100"
            aria-label={busyAria}
          >
            <div class="vu-bar" style={`width: ${busyPct}%`}></div>
          </div>
          <div class="deck-transcribe-meta">
            <span class="deck-transcribe-msg">{busyMsg}</span>
          </div>
        </div>
      {/if}
    {/if}

    <div class="deck-wave" aria-hidden="true">
      {#each waveDelays as delay}
        <div
          class="w-bar {isRecording ? 'active' : ''}"
          style={`animation-delay: ${delay}s;`}
        ></div>
      {/each}
    </div>

    <div class="deck-bottom">
      <div class="deck-timer-box">
        <span class="deck-timer {isRecording ? 'recording' : ''}" aria-label="Время записи">
          {timerText}
        </span>
        {#if !isMobile}
          <span class="deck-hotkey" title="Глобальный хоткей">{hotkeyLabel}</span>
        {/if}
      </div>

      <div class="deck-bottom-right">
        <div class="deck-levels" aria-label="Уровни звука">
          {#if isRecording}
            <span>Вы: {micRms > 0.001 ? `${micDb}\u00A0дБ` : '–\u00A0дБ'}</span>
            {#if hasSystemAudio()}
              <span class="sep">·</span>
              <span>Собеседники: {sysRms > 0.001 ? `${sysDb}\u00A0дБ` : '–\u00A0дБ'}</span>
            {/if}
          {:else}
            <span class="levels-idle">
              {hasSystemAudio() ? 'Микрофон + система' : 'Микрофон'}
            </span>
          {/if}
        </div>

        <button
          class="btn-action-round {isRecording ? 'recording' : ''}"
          onclick={toggleRecord}
          type="button"
        >
          {#if isRecording}
            <span class="btn-rec-icon square" aria-hidden="true"></span>
            <span>Остановить</span>
          {:else}
            <span class="btn-rec-icon dot" aria-hidden="true"></span>
            <span>Начать запись</span>
          {/if}
        </button>
      </div>
    </div>
  </div>
</section>
