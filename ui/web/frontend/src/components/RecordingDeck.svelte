<script lang="ts">
  import { onMount } from 'svelte'
  import { baseName, isMobile, readJsonSafe } from '../lib/api'
  import { settings } from '../lib/settings.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'
  import { emit } from '../lib/ui.svelte.ts'
  import { engine, syncEngineFromStatus } from '../lib/transcript.svelte.ts'

  let isRecording = $state(false)
  let timerText = $state('00:00:00')
  let micName = $state('Определение…')
  let micMuted = $state(false)
  let micRms = $state(0)
  let sysLevel = $state(0)
  // Шкала для полоски (RMS в квадрате — ближе к восприятию) и честные −60…0 дБFS.
  let micLevel = $derived(Math.min(100, Math.round(Math.sqrt(micRms) * 100)))
  let micDb = $derived(micRms > 0.001 ? Math.max(-60, Math.round(20 * Math.log10(micRms))) : -60)

  const hotkeyLabel = /Mac/i.test(navigator.userAgent) ? 'Cmd+Shift+R' : 'Ctrl+Shift+R'

  let startTime = 0
  let timerInterval: ReturnType<typeof setInterval> | null = null

  function setRecordingState(rec: boolean, initialElapsed = 0): void {
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

  async function toggleRecord(): Promise<void> {
    const now = Date.now()
    if (now - lastToggleTime < 500) return
    lastToggleTime = now
    try {
      if (!isRecording) {
        const r = await fetch('/api/record/start', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ mode: settings.recordMode }),
        })
        const d = await readJsonSafe(r)
        if (d.ok) {
          setRecordingState(true)
          if (d.mic_muted) toast('Микрофон выключен в системе – будет тишина', 'error')
        } else if (d.error === 'Запись уже идёт') {
          setRecordingState(true)
        } else {
          toast(d.error || 'Не удалось начать запись', 'error')
        }
      } else {
        const r = await fetch('/api/record/stop', { method: 'POST' })
        const d = await readJsonSafe(r)
        setRecordingState(false)
        emit('recordings-changed')
        if (d.ok) {
          if (d.silent) toast('Записана тишина – микрофон ничего не записал. Проверьте звук и источник в системе', 'error')
          else if (d.filename) toast(`Запись сохранена: ${d.filename}`)
        } else if (d.error && d.error !== 'Запись не активна') {
          toast(d.error || 'Не удалось остановить запись', 'error')
        }
      }
    } catch (e) {
      toast('Ошибка записи: ' + (e as Error).message, 'error')
    }
  }

  async function pollLevels(): Promise<void> {
    try {
      const r = await fetch('/api/status')
      const d = await readJsonSafe(r)
      sysLevel = Math.min(100, Math.round((d.sys_level || 0) * 100))
      // Движок един на всё приложение: опрос — запасной канал правды о нём
      // (расшифровку могли запустить авто-режимом, CLI или IPC мимо UI).
      syncEngineFromStatus(!!d.transcribing, String(d.transcribing_file || ''))
      // Запись могли включить/выключить извне (глобальный хоткей, IPC,
      // второй клик по ярлыку) — кнопка обязана следовать за реальностью.
      if (d.recording && !isRecording) {
        setRecordingState(true, d.elapsed || 0)
      } else if (!d.recording && isRecording) {
        setRecordingState(false)
        emit('recordings-changed')
      }
      if (d.device) {
        micName = d.device.mic_name || 'Недоступен'
        micMuted = !!d.device.mic_muted
      } else {
        micName = 'Недоступен'
      }
      // Уровень микрофона — из нативного захвата ядра (/api/status), а не из
      // getUserMedia: браузерный запрос пугает и переводит Bluetooth-гарнитуры
      // в телефонный профиль. До старта записи метр честно показывает ноль.
      // tail_level_01 уже RMS с запасом ×3; приводим к 0…1.
      micRms = Math.min(1, (d.mic_level || 0) / 3)
    } catch {
      /* молча */
    }
  }

  // Хоткей внутри окна: глобальный граб не работает на Wayland при фокусе
  // окна, а в браузере (--web) Ctrl+Shift+R — это hard-reload. Ловим сами.
  function onWindowHotkey(e: KeyboardEvent): void {
    if (e.repeat) return
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.code === 'KeyR') {
      e.preventDefault()
      void toggleRecord()
    }
  }

  onMount(() => {
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
    <div class="deck-status-bar">
      <span
        class="status-pill {isRecording ? 'recording' : ''}"
        role="status"
        title={engine.running && engine.file ? engine.file : undefined}
      >
        {#if isRecording}
          Идёт запись
        {:else if engine.running}
          {engine.file ? `Расшифровка: ${baseName(engine.file)}` : 'Идёт расшифровка…'}
        {:else}
          Готов к записи
        {/if}
      </span>
    </div>

    <div class="timer-readout {isRecording ? 'recording' : ''}" aria-label="Время записи">
      {timerText}
    </div>

    <div class="vu-meter-box" aria-hidden="true">
      <div class="vu-row">
        <span class="vu-label">Вы</span>
        <div class="vu-track"><div class="vu-bar" style={`width: ${micLevel}%`}></div></div>
        <span class="vu-db">{micRms > 0.001 ? `${micDb}\u00A0дБ` : '–\u00A0дБ'}</span>
      </div>
      <div class="vu-row">
        <span class="vu-label">Собеседники</span>
        <div class="vu-track"><div class="vu-bar" style={`width: ${sysLevel}%`}></div></div>
        <span class="vu-db">{sysLevel > 0 ? `${sysLevel}%` : '–'}</span>
      </div>
    </div>

    <div class="mic-strip">
      <span>Микрофон: <span class="mic-name">{micName}</span></span>
      {#if micMuted}
        <span class="status-badge warn">Выключен в системе</span>
      {/if}
    </div>

    <button class="btn-record-main {isRecording ? 'active' : 'idle'}" onclick={toggleRecord}>
      <span>{isRecording ? 'Остановить запись' : 'Начать запись'}</span>
    </button>
    {#if !isMobile}
      <div class="hotkey-hint">
        Из любого окна: <kbd>{hotkeyLabel}</kbd>
      </div>
    {/if}
  </div>
</section>
