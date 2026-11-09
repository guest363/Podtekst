<script lang="ts">
  import type { SummaryPreset } from '../lib/api'

  interface Props {
    open: boolean
    presets: SummaryPreset[]
    initialId?: string
    defaultPresetId?: string
    systemPromptBase?: string
    onselect?: (id: string) => void
    onclose: () => void
  }

  let {
    open,
    presets,
    initialId = '',
    defaultPresetId = '',
    systemPromptBase = '',
    onselect,
    onclose,
  }: Props = $props()

  let activeId = $state('')
  let showFullPrompt = $state(false)
  let copied = $state(false)

  $effect(() => {
    if (open) {
      activeId = initialId || defaultPresetId || presets[0]?.id || 'meeting'
      showFullPrompt = false
      copied = false
    }
  })

  const activePreset = $derived(
    presets.find((p) => p.id === activeId) ?? presets[0] ?? null,
  )

  const activeFullPrompt = $derived.by(() => {
    if (!activePreset) return ''
    let text = `${activePreset.role || ''} ${activePreset.domain || ''}`
    if (activePreset.speaker_primary && activePreset.speaker_primary.length > 0) {
      text += ` Приоритет при противоречиях: слова ${activePreset.speaker_primary.join(', ')} важнее остальных.`
    }
    if (systemPromptBase) {
      text += `\n\n${systemPromptBase}`
    }
    return text.trim()
  })

  const activeRolePrompt = $derived.by(() => {
    if (!activePreset) return ''
    let text = `${activePreset.role || ''}\n${activePreset.domain || ''}`
    if (activePreset.speaker_primary && activePreset.speaker_primary.length > 0) {
      text += `\nПриоритет реплик: слова ${activePreset.speaker_primary.join(', ')} важнее остальных.`
    }
    return text.trim()
  })

  function onWindowKey(e: KeyboardEvent): void {
    if (open && e.key === 'Escape') {
      e.stopPropagation()
      onclose()
    }
  }

  function onBackdropMouseDown(e: MouseEvent): void {
    if (e.target === e.currentTarget) {
      onclose()
    }
  }

  async function copyPrompt(): Promise<void> {
    const text = showFullPrompt ? activeFullPrompt : activeRolePrompt
    if (!text) return
    try {
      await navigator.clipboard.writeText(text)
      copied = true
      setTimeout(() => {
        copied = false
      }, 1600)
    } catch {
      /* буфер недоступен */
    }
  }

  function handleSelect(id: string): void {
    onselect?.(id)
    onclose()
  }
</script>

<svelte:window onkeydown={onWindowKey} />

{#if open}
  <div
    class="modal-overlay open"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Справочник шаблонов и промптов"
    onmousedown={onBackdropMouseDown}
  >
    <div class="modal-box summary-guide-box">
      <!-- Шапка модального окна -->
      <div class="modal-top">
        <div class="modal-title">Справочник шаблонов и промптов</div>
        <button class="icon-btn" aria-label="Закрыть окно" onclick={onclose}>✕</button>
      </div>

      <!-- Селектор шаблона: один чистый элемент без горизонтальных табов -->
      <div class="guide-select-field">
        <label for="guide-preset-select" class="guide-label">Шаблон</label>
        <div class="guide-select-wrap">
          <select
            id="guide-preset-select"
            class="guide-select"
            value={activeId}
            onchange={(e) => (activeId = e.currentTarget.value)}
          >
            {#each presets as p (p.id)}
              <option value={p.id}>
                {p.label}{p.sensitive ? ' (черновик)' : ''}{p.id === defaultPresetId ? ' — по умолчанию' : ''}
              </option>
            {/each}
          </select>
        </div>
      </div>

      {#if activePreset}
        <div class="guide-body">
          <!-- Описание и назначение выбранного шаблона -->
          <div class="guide-meta">
            <div class="guide-desc-row">
              <span class="guide-desc">{activePreset.short}</span>
              <span class="guide-ver">{activePreset.prompt_version || 'v4'}</span>
            </div>

            {#if activePreset.caution}
              <div class="guide-caution">
                {activePreset.caution}
              </div>
            {/if}

            {#if activePreset.speaker_primary && activePreset.speaker_primary.length > 0}
              <div class="guide-speakers">
                Приоритет: реплики «{activePreset.speaker_primary.join(', ')}» имеют преимущество при разногласиях.
              </div>
            {/if}
          </div>

          <!-- Блок системного промпта без лишних обводок -->
          <div class="guide-prompt-block">
            <div class="guide-prompt-header">
              <span class="guide-label">
                {showFullPrompt ? 'Полный системный промпт' : 'Ролевая инструкция'}
              </span>
              <div class="guide-prompt-actions">
                {#if systemPromptBase}
                  <button
                    type="button"
                    class="guide-action-btn"
                    onclick={() => (showFullPrompt = !showFullPrompt)}
                  >
                    {showFullPrompt ? 'Только роль' : 'С общими правилами'}
                  </button>
                {/if}
                <button type="button" class="guide-action-btn" onclick={copyPrompt}>
                  {copied ? 'Скопировано' : 'Скопировать'}
                </button>
              </div>
            </div>
            <pre class="guide-prompt-code custom-scroll">{showFullPrompt ? activeFullPrompt : activeRolePrompt}</pre>
          </div>
        </div>
      {/if}

      <!-- Футер модалки -->
      <div class="guide-footer">
        <div>
          {#if onselect && activePreset && activePreset.id !== defaultPresetId}
            <button
              type="button"
              class="btn-secondary btn-sm"
              onclick={() => handleSelect(activePreset.id)}
            >
              Сделать основным
            </button>
          {/if}
        </div>
        <button type="button" class="btn-secondary btn-sm" onclick={onclose}>
          Закрыть
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .summary-guide-box {
    max-width: 560px;
    width: 100%;
    max-height: min(640px, calc(100dvh - 48px));
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 22px;
  }
  .guide-select-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .guide-label {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .guide-select-wrap {
    position: relative;
    width: 100%;
  }
  .guide-select {
    width: 100%;
    padding: 8px 34px 8px 12px;
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2.5' stroke-linecap='round' stroke-linejoin='round' xmlns='http://www.w3.org/2000/svg'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 12px center;
    cursor: pointer;
    transition: border-color 150ms var(--ease-spring);
  }
  .guide-select:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 1px;
    border-color: var(--accent-blue);
  }
  .guide-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-height: 0;
    flex: 1;
    overflow-y: auto;
  }
  .guide-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .guide-desc-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .guide-desc {
    font-size: 12.5px;
    color: var(--text-primary);
    line-height: 1.45;
  }
  .guide-ver {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-faint);
    flex-shrink: 0;
  }
  .guide-caution {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--accent-amber);
    background: var(--accent-amber-bg, #faf3e3);
    border-radius: var(--radius-sm);
    padding: 7px 10px;
  }
  .guide-speakers {
    font-size: 11.5px;
    color: var(--text-muted);
    line-height: 1.4;
  }
  .guide-prompt-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 0;
    flex: 1;
  }
  .guide-prompt-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .guide-prompt-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .guide-action-btn {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: 11.5px;
    color: var(--accent-blue);
    cursor: pointer;
    text-underline-offset: 2px;
  }
  .guide-action-btn:hover {
    text-decoration: underline;
  }
  .guide-prompt-code {
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner);
    padding: 10px 12px;
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 220px;
    overflow-y: auto;
  }
  .guide-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding-top: 4px;
  }
</style>
