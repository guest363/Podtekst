<script lang="ts">
  import { baseName, encPath, fmtClock, readJsonSafe } from '../lib/api'
  import { clearQueue, loadQueue, queueState, removeFromQueue } from '../lib/queue.svelte.ts'
  import { engine, startTranscription, syncEngineFromStatus } from '../lib/transcript.svelte.ts'
  import { loadRecordings, recordings } from '../lib/recordings.svelte.ts'
  import { openTranscript } from '../lib/ui.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'
  import { onMount } from 'svelte'

  let fileInput: HTMLInputElement | null = $state(null)
  let uploading = $state(false)
  let isDragOver = $state(false)

  onMount(() => {
    void loadQueue()
    void loadRecordings()
    // Страница очереди не монтирует RecordingDeck, поэтому о внешнем запуске
    // (автозапуск, CLI) узнаём только опросом.
    const timer = setInterval(() => {
      void loadQueue().then(() =>
        syncEngineFromStatus(queueState.running, queueState.currentFile || ''),
      )
    }, 2000)
    return () => clearInterval(timer)
  })

  async function handleFiles(files: FileList | File[]) {
    if (!files || files.length === 0) return
    uploading = true
    let count = 0
    try {
      for (const file of Array.from(files)) {
        const res = await fetch('/api/upload', {
          method: 'POST',
          headers: {
            'X-Filename': encPath(file.name),
            'Content-Type': file.type || 'application/octet-stream',
          },
          body: file,
        })
        const data = await readJsonSafe(res)
        if (data?.ok && data.path) {
          count += 1
          await startTranscription(data.path as string)
        }
      }
      if (count > 0) {
        toast(`Добавлено файлов в очередь: ${count}`)
        await loadRecordings()
        await loadQueue()
      }
    } catch (e) {
      toast('Ошибка загрузки: ' + (e as Error).message, 'error')
    } finally {
      uploading = false
    }
  }

  function onFileInputChange(e: Event) {
    const input = e.target as HTMLInputElement
    if (input.files) {
      void handleFiles(input.files)
      input.value = ''
    }
  }

  function onDrop(e: DragEvent) {
    e.preventDefault()
    isDragOver = false
    if (e.dataTransfer?.files) {
      void handleFiles(e.dataTransfer.files)
    }
  }

  const unparsedRecordings = $derived(
    recordings.items.filter(
      (r) =>
        !r.has_transcript &&
        r.path !== engine.file &&
        !queueState.items.some((q) => q.file === r.path)
    )
  )
</script>

<div
  class="queue-hub"
  class:dragover={isDragOver}
  ondragover={(e) => {
    e.preventDefault()
    isDragOver = true
  }}
  ondragleave={(e) => {
    e.preventDefault()
    isDragOver = false
  }}
  ondrop={onDrop}
  role="region"
  aria-label="Очередь расшифровки"
>
  <input
    type="file"
    multiple
    accept="audio/*,video/*,.mp3,.wav,.m4a,.ogg,.flac,.mp4,.mov,.mkv,.webm"
    bind:this={fileInput}
    onchange={onFileInputChange}
    hidden
  />

  <div class="queue-head">
    <div class="queue-head-left">
      <h2 class="card-title">Очередь расшифровки</h2>
      <span class="queue-sub">
        {#if engine.running && queueState.items.length > 0}
          1 выполняется, {queueState.items.length} в очереди
        {:else if engine.running}
          1 выполняется
        {:else if queueState.items.length > 0}
          {queueState.items.length} в очереди
        {:else}
          Нет активных задач
        {/if}
      </span>
    </div>

    <div class="queue-head-actions">
      {#if queueState.items.length > 0}
        <button
          type="button"
          class="btn-secondary btn-sm"
          onclick={() => void clearQueue()}
          title="Снять все ожидающие задачи"
        >
          Очистить очередь
        </button>
      {/if}
      <button
        type="button"
        class="btn-primary btn-sm"
        disabled={uploading}
        onclick={() => fileInput?.click()}
      >
        {uploading ? 'Загрузка…' : '+ Добавить файлы'}
      </button>
    </div>
  </div>

  <div
    class="queue-dropzone"
    onclick={() => fileInput?.click()}
    onkeydown={(e) => {
      if (e.key === 'Enter') fileInput?.click()
    }}
    role="button"
    tabindex="0"
  >
    <svg class="drop-icon" width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="17 8 12 3 7 8" />
      <line x1="12" y1="3" x2="12" y2="15" />
    </svg>
    <div class="drop-label">
      Перетащите сюда аудио или видео файлы для пакетной обработки
    </div>
    <div class="drop-hint">
      Поддерживаются WAV, MP3, M4A, OGG, FLAC, MP4
    </div>
  </div>

  {#if engine.running || engine.summaryRunning}
    {@const activeFile = engine.running ? engine.file : engine.summaryFile}
    {@const activePct = engine.running ? engine.pct : engine.summaryPct}
    {@const activeMsg = engine.running ? engine.msg : (engine.summaryMsg || 'Сборка итогов…')}
    {@const activeTitle = engine.running ? 'Выполняется сейчас' : 'Сборка итогов'}
    <div class="queue-section">
      <div class="section-title">
        <span class="status-dot active" aria-hidden="true"></span>
        <span>{activeTitle}</span>
      </div>
      <div class="active-task-card">
        <div class="active-task-row">
          <div class="task-file-info">
            <span class="task-name" title={activeFile}>{baseName(activeFile)}</span>
            <span class="task-msg">{activeMsg}</span>
          </div>
          <div class="task-right">
            <span class="task-pct">{activePct}%</span>
            {#if activeFile}
              <button
                type="button"
                class="btn-secondary btn-sm"
                onclick={() => openTranscript(activeFile)}
              >
                Открыть
              </button>
            {/if}
          </div>
        </div>
        <div class="progress-track" role="progressbar" aria-valuenow={activePct} aria-valuemin="0" aria-valuemax="100">
          <div class="vu-bar" style={`width: ${activePct}%`}></div>
        </div>
      </div>
    </div>
  {/if}

  {#if queueState.items.length > 0}
    <div class="queue-section">
      <div class="section-title">
        <span>В очереди ({queueState.items.length})</span>
      </div>
      <div class="queue-table-wrap">
        <table class="queue-table">
          <thead>
            <tr>
              <th style="width: 44px">#</th>
              <th>Файл</th>
              <th style="width: 120px">Движок</th>
              <th style="width: 90px">Статус</th>
              <th style="width: 44px; text-align: right"></th>
            </tr>
          </thead>
          <tbody>
            {#each queueState.items as task, idx (task.file)}
              <tr>
                <td class="pos-col">#{idx + 2}</td>
                <td class="name-col" title={task.file}>
                  <span class="task-item-name">{task.name || baseName(task.file)}</span>
                </td>
                <td class="meta-col">
                  <span class="tag-engine">{task.engine || 'auto'}</span>
                </td>
                <td class="status-col">
                  <span class="badge-waiting">Ожидание</span>
                </td>
                <td class="action-col">
                  <button
                    type="button"
                    class="rec-delete-icon"
                    title="Снять с очереди"
                    aria-label={`Снять ${task.name || task.file} с очереди`}
                    onclick={() => void removeFromQueue(task.file)}
                  >
                    ✕
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
  {/if}

  {#if !engine.running && queueState.items.length === 0}
    <div class="queue-empty">
      <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="3" y="4" width="18" height="18" rx="2" />
        <line x1="8" y1="10" x2="16" y2="10" />
        <line x1="8" y1="14" x2="14" y2="14" />
      </svg>
      <div class="queue-empty-title">Очередь расшифровки пуста</div>
      <div class="queue-empty-sub">
        Загрузите аудиофайлы через «+ Добавить файлы» или перетащите их сюда для последовательного распознавания.
      </div>
    </div>
  {/if}

  {#if unparsedRecordings.length > 0}
    <div class="queue-section queue-library">
      <div class="section-title">
        <span>Записи без расшифровки ({unparsedRecordings.length})</span>
      </div>
      <div class="unparsed-list">
        {#each unparsedRecordings.slice(0, 8) as rec (rec.path)}
          <div class="unparsed-row">
            <span class="unparsed-title" title={rec.name}>{rec.name}</span>
            {#if rec.duration_sec}
              <span class="unparsed-dur">{fmtClock(rec.duration_sec)}</span>
            {/if}
            <button
              type="button"
              class="btn-secondary btn-sm"
              onclick={() => void startTranscription(rec.path)}
            >
              В очередь
            </button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .queue-hub {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 4px 0;
  }
  .queue-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .queue-head-left {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .queue-sub {
    font-size: 12px;
    color: var(--text-muted);
  }
  .queue-head-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .queue-dropzone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px 16px;
    border: 2px dashed var(--border-card);
    border-radius: var(--radius-inner);
    background: var(--bg-inset);
    cursor: pointer;
    transition: border-color 160ms var(--ease-spring), background 160ms var(--ease-spring);
    text-align: center;
  }
  .queue-dropzone:hover,
  .queue-hub.dragover .queue-dropzone {
    border-color: var(--accent-blue);
    background: #f0f7ff;
  }
  .drop-icon {
    color: var(--accent-blue);
  }
  .drop-label {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-primary);
  }
  .drop-hint {
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .queue-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .section-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 700;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .active-task-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--accent-blue);
    border-radius: var(--radius-inner);
    background: #f8fbff;
  }
  .active-task-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }
  .task-file-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .task-name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .task-msg {
    font-size: 11.5px;
    color: var(--accent-blue);
  }
  .task-right {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
  }
  .task-pct {
    font-family: var(--font-mono);
    font-size: 13px;
    font-weight: 700;
    color: var(--accent-blue);
  }
  .queue-table-wrap {
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    overflow: hidden;
    background: var(--bg-card);
  }
  .queue-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  .queue-table th {
    text-align: left;
    padding: 8px 12px;
    background: var(--bg-inset);
    color: var(--text-muted);
    font-weight: 600;
    border-bottom: 1px solid var(--border-soft);
  }
  .queue-table td {
    padding: 9px 12px;
    border-bottom: 1px solid var(--border-soft);
    color: var(--text-primary);
  }
  .queue-table tr:last-child td {
    border-bottom: none;
  }
  .pos-col {
    font-family: var(--font-mono);
    color: var(--text-faint);
    font-weight: 600;
  }
  .name-col {
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .task-item-name {
    font-weight: 500;
  }
  .tag-engine {
    display: inline-block;
    padding: 2px 6px;
    border-radius: var(--radius-sm);
    background: var(--bg-inset);
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-muted);
  }
  .badge-waiting {
    display: inline-block;
    padding: 2px 7px;
    border-radius: var(--radius-pill);
    background: var(--accent-amber-bg);
    color: var(--accent-amber);
    font-size: 11px;
    font-weight: 600;
  }
  .queue-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 36px 16px;
    text-align: center;
    color: var(--text-faint);
  }
  .queue-empty-title {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--text-primary);
  }
  .queue-empty-sub {
    font-size: 12px;
    color: var(--text-muted);
    max-width: 400px;
  }
  .unparsed-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .unparsed-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-sm);
    background: var(--bg-card);
    font-size: 12px;
  }
  .unparsed-title {
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
  }
  .unparsed-dur {
    font-family: var(--font-mono);
    color: var(--text-faint);
    font-size: 11px;
  }
</style>
