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
      <div class="modal-top">
        <div class="modal-heading-group">
          <div class="modal-title">Справочник шаблонов и промптов</div>
          <div class="modal-subtitle">
            Инструкции и структура конспекта, передаваемые локальной модели
          </div>
        </div>
        <button class="icon-btn" aria-label="Закрыть окно" onclick={onclose}>✕</button>
      </div>

      <!-- Горизонтальный переключатель шаблонов -->
      <div class="summary-guide-scroller">
        <div class="seg-bar" role="tablist" aria-label="Шаблоны">
          {#each presets as p (p.id)}
            <button
              type="button"
              role="tab"
              aria-selected={p.id === activeId}
              class="seg-item {p.id === activeId ? 'active' : ''}"
              onclick={() => (activeId = p.id)}
            >
              <span>{p.label}</span>
              {#if p.id === defaultPresetId}
                <span class="guide-default-dot" title="Шаблон по умолчанию"></span>
              {/if}
              {#if p.sensitive}
                <span class="guide-sensitive-mark">черновик</span>
              {/if}
            </button>
          {/each}
        </div>
      </div>

      {#if activePreset}
        <div class="summary-guide-content">
          <!-- Заголовок выбранного пресета -->
          <div class="guide-preset-header">
            <div class="guide-preset-title-wrap">
              <span class="guide-preset-name">{activePreset.label}</span>
              <span class="guide-preset-ver">{activePreset.prompt_version || 'v4'}</span>
              {#if activePreset.sensitive}
                <span class="status-badge warn">Черновик: нужна проверка</span>
              {/if}
              {#if activePreset.id === defaultPresetId}
                <span class="status-badge info">По умолчанию</span>
              {/if}
            </div>
            <div class="guide-preset-short">{activePreset.short}</div>
          </div>

          <!-- Предостережение, если задано -->
          {#if activePreset.caution}
            <div class="guide-caution">
              <strong>Внимание:</strong> {activePreset.caution}
            </div>
          {/if}

          <!-- Приоритет спикеров -->
          {#if activePreset.speaker_primary && activePreset.speaker_primary.length > 0}
            <div class="guide-field">
              <span class="guide-field-label">Приоритет реплик</span>
              <span class="guide-field-text">
                Слова участников с ролями «{activePreset.speaker_primary.join(', ')}» имеют преимущество при разногласиях.
              </span>
            </div>
          {/if}

          <!-- Спецификация секций -->
          <div class="guide-field">
            <span class="guide-field-label">Секции выходного документа</span>
            <div class="guide-sections-list">
              {#if activePreset.sections_detail && activePreset.sections_detail.length > 0}
                {#each activePreset.sections_detail as sec (sec.id)}
                  <span class="guide-section-chip">
                    <span class="guide-sec-title">{sec.title}</span>
                    {#if sec.role === 'task'}
                      <span class="guide-role-tag task">задачи</span>
                    {:else if sec.role === 'decision'}
                      <span class="guide-role-tag decision">решения</span>
                    {:else if sec.role === 'status'}
                      <span class="guide-role-tag status">статус</span>
                    {/if}
                  </span>
                {/each}
              {:else}
                {#each activePreset.sections as secId}
                  <span class="guide-section-chip">
                    <span class="guide-sec-title">{secId}</span>
                  </span>
                {/each}
              {/if}
            </div>
          </div>

          <!-- Блок системного промпта -->
          <div class="guide-field">
            <div class="guide-prompt-head">
              <span class="guide-field-label">
                {showFullPrompt ? 'Полный системный промпт' : 'Ролевая инструкция шаблона'}
              </span>
              <div class="guide-prompt-actions">
                {#if systemPromptBase}
                  <button
                    type="button"
                    class="guide-text-btn"
                    onclick={() => (showFullPrompt = !showFullPrompt)}
                  >
                    {showFullPrompt ? 'Только роль' : 'С общими правилами'}
                  </button>
                {/if}
                <button type="button" class="guide-text-btn" onclick={copyPrompt}>
                  {copied ? 'Скопировано' : 'Скопировать'}
                </button>
              </div>
            </div>
            <pre class="guide-prompt-code">{showFullPrompt ? activeFullPrompt : activeRolePrompt}</pre>
          </div>
        </div>
      {/if}

      <div class="modal-footer">
        <div>
          {#if onselect && activePreset && activePreset.id !== defaultPresetId}
            <button
              type="button"
              class="btn-secondary btn-sm"
              onclick={() => handleSelect(activePreset.id)}
            >
              Сделать шаблоном по умолчанию
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
    max-width: 640px;
    width: 100%;
    max-height: calc(100dvh - 48px);
    display: flex;
    flex-direction: column;
    padding: 0;
    overflow: hidden;
    gap: 0;
  }
  .modal-top {
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--border-soft);
  }
  .modal-heading-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .modal-subtitle {
    font-size: 11.5px;
    color: var(--text-faint);
    line-height: 1.4;
  }
  .summary-guide-scroller {
    padding: 10px 20px 6px;
    overflow-x: auto;
    background: var(--bg-inset);
    border-bottom: 1px solid var(--border-soft);
    flex-shrink: 0;
  }
  .summary-guide-scroller .seg-bar {
    width: max-content;
    display: inline-flex;
  }
  .summary-guide-scroller .seg-item {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 11px;
    font-size: 12px;
  }
  .guide-default-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent-blue);
    flex-shrink: 0;
  }
  .guide-sensitive-mark {
    font-size: 9.5px;
    color: var(--accent-amber);
    border: 1px solid currentColor;
    border-radius: var(--radius-pill);
    padding: 0 4px;
    line-height: 1.2;
  }
  .summary-guide-content {
    flex: 1;
    overflow-y: auto;
    padding: 14px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 12px;
  }
  .guide-preset-header {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .guide-preset-title-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .guide-preset-name {
    font-size: 14px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .guide-preset-ver {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-faint);
  }
  .guide-preset-short {
    font-size: 12px;
    color: var(--text-muted);
  }
  .guide-caution {
    font-size: 11.5px;
    color: var(--accent-amber);
    background: var(--accent-amber-bg, #faf3e3);
    border: 1px solid rgba(138, 90, 0, 0.2);
    border-radius: var(--radius-sm);
    padding: 7px 10px;
    line-height: 1.45;
  }
  .guide-field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .guide-field-label {
    font-size: 10.5px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-faint);
  }
  .guide-field-text {
    font-size: 12px;
    color: var(--text-muted);
  }
  .guide-sections-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .guide-section-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: var(--bg-inset);
    border: 1px solid var(--border-card);
    border-radius: var(--radius-sm);
    padding: 3px 8px;
    font-size: 11.5px;
    color: var(--text-primary);
  }
  .guide-sec-title {
    font-weight: 500;
  }
  .guide-role-tag {
    font-family: var(--font-mono);
    font-size: 9.5px;
    padding: 1px 4px;
    border-radius: 3px;
    background: #fff;
    border: 1px solid var(--border-soft);
    color: var(--text-faint);
  }
  .guide-role-tag.task {
    color: var(--accent-blue);
    border-color: rgba(36, 86, 198, 0.25);
  }
  .guide-role-tag.decision {
    color: var(--accent-ok);
    border-color: rgba(14, 122, 85, 0.25);
  }
  .guide-prompt-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .guide-prompt-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .guide-text-btn {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: 11.5px;
    color: var(--accent-blue);
    cursor: pointer;
    text-underline-offset: 2px;
  }
  .guide-text-btn:hover {
    text-decoration: underline;
  }
  .guide-prompt-code {
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.45;
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-sm);
    padding: 10px 12px;
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 180px;
    overflow-y: auto;
  }
  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 20px;
    border-top: 1px solid var(--border-soft);
    background: var(--bg-inset);
  }
</style>
