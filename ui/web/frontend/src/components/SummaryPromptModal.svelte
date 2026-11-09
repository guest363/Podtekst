<script lang="ts">
import { tick } from 'svelte'
import type { SummaryPreset } from '../lib/api'

type Props = {
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
let selectOpen = $state(false)
let selectTrigger = $state<HTMLButtonElement | null>(null)
let selectList = $state<HTMLDivElement | null>(null)

$effect(() => {
  if (open) {
    activeId = initialId || defaultPresetId || presets[0]?.id || 'meeting'
    showFullPrompt = false
    copied = false
    selectOpen = false
  }
})

const activePreset = $derived(presets.find((p) => p.id === activeId) ?? presets[0] ?? null)

const activeSections = $derived.by(() => {
  if (!activePreset) return []
  if (activePreset.sections_detail && activePreset.sections_detail.length > 0) {
    return activePreset.sections_detail
  }
  return activePreset.sections.map((id) => ({ id, title: id, role: 'plain' as const }))
})

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
  if (!open) return
  if (e.key === 'Escape') {
    if (selectOpen) {
      closeSelect(true)
      e.stopPropagation()
    } else {
      onclose()
    }
  }
}

function onBackdropMouseDown(e: MouseEvent): void {
  if (e.target === e.currentTarget) {
    onclose()
  }
}

function openSelectList(): void {
  selectOpen = true
  void tick().then(() => {
    const current = selectList?.querySelector<HTMLElement>('[aria-selected="true"]')
    const first = selectList?.querySelector<HTMLElement>('.speaker-select-item')
    ;(current ?? first)?.focus()
  })
}

function closeSelect(returnFocus = false): void {
  selectOpen = false
  if (returnFocus) selectTrigger?.focus()
}

function choosePreset(id: string): void {
  activeId = id
  closeSelect(true)
}

function onSelectKeydown(e: KeyboardEvent): void {
  if (e.key === 'Escape' && selectOpen) {
    e.stopPropagation()
    closeSelect(true)
    return
  }
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
  e.preventDefault()
  if (!selectOpen) {
    openSelectList()
    return
  }
  const items = [...(selectList?.querySelectorAll<HTMLElement>('.speaker-select-item') ?? [])]
  if (!items.length) return
  const active = document.activeElement
  const index = active instanceof HTMLElement ? items.indexOf(active) : -1
  const next =
    e.key === 'ArrowDown'
      ? items[(index + 1) % items.length]
      : items[(index - 1 + items.length) % items.length]
  next?.focus()
}

function onSelectFocusOut(e: FocusEvent): void {
  const next = e.relatedTarget instanceof Node ? e.relatedTarget : null
  if (
    selectOpen &&
    (!next || !(e.currentTarget instanceof HTMLElement && e.currentTarget.contains(next)))
  ) {
    selectOpen = false
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

      <!-- Селектор шаблона: в стандартном для приложения дизайне (.summary-select) -->
      <div class="guide-select-field">
        <div class="summary-preset-select" onfocusout={onSelectFocusOut}>
          <button
            bind:this={selectTrigger}
            type="button"
            class="summary-select"
            aria-haspopup="listbox"
            aria-expanded={selectOpen}
            aria-label="Выбрать шаблон"
            onclick={() => (selectOpen ? closeSelect() : openSelectList())}
            onkeydown={onSelectKeydown}
          >
            <span class="summary-preset-value">
              <span class="guide-select-name">{activePreset?.label || 'Выберите шаблон'}</span>
              {#if activePreset?.sensitive}
                <span class="guide-select-badge warn">черновик</span>
              {/if}
              {#if activePreset?.id === defaultPresetId}
                <span class="guide-select-badge info">по умолчанию</span>
              {/if}
            </span>
            <span class="summary-preset-chevron {selectOpen ? 'open' : ''}" aria-hidden="true">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </span>
          </button>

          {#if selectOpen}
            <button
              type="button"
              class="speaker-popover-backdrop"
              aria-label="Закрыть список шаблонов"
              onclick={() => closeSelect(true)}
            ></button>
            <div
              bind:this={selectList}
              class="speaker-select-popover summary-guide-popover"
              role="listbox"
              tabindex="-1"
              onkeydown={onSelectKeydown}
            >
              {#each presets as p (p.id)}
                <button
                  type="button"
                  class="speaker-select-item"
                  role="option"
                  aria-selected={p.id === activeId}
                  onclick={() => choosePreset(p.id)}
                >
                  <span class="speaker-select-item-left">
                    <span
                      class="speaker-select-item-dot"
                      style={`background: ${p.id === activeId ? 'var(--accent-blue)' : '#94a3b8'}`}
                    ></span>
                    <span class="speaker-select-item-name">{p.label}</span>
                  </span>
                  {#if p.sensitive}
                    <span class="speaker-select-item-sub">черновик</span>
                  {:else if p.id === defaultPresetId}
                    <span class="speaker-select-item-sub">по умолчанию</span>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </div>

      {#if activePreset}
        <div class="guide-body">
          <!-- Описание и назначение -->
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

          <!-- Секции выжимки: структура ответа модели в порядке следования -->
          <div class="guide-sections-block">
            <span class="guide-label">Структура ответа ({activeSections.length} секций)</span>
            <div class="guide-sections-flow">
              {#each activeSections as sec, idx (sec.id)}
                <div class="guide-section-step">
                  <span class="guide-step-num">{idx + 1}</span>
                  <span class="guide-step-title">{sec.title}</span>
                </div>
              {/each}
            </div>
          </div>

          <!-- Блок системного промпта -->
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
    max-width: 580px;
    width: 100%;
    max-height: min(680px, calc(100dvh - 48px));
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 22px;
  }
  .guide-select-field {
    width: 100%;
  }
  .summary-preset-select {
    position: relative;
    width: 100%;
  }
  .summary-select {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    min-width: 0;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    color: var(--text-primary);
    padding: 7px 10px;
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
  }
  .summary-select:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-preset-value {
    display: flex;
    align-items: center;
    gap: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .guide-select-name {
    font-weight: 600;
  }
  .guide-select-badge {
    font-size: 10.5px;
    font-weight: 500;
    border-radius: var(--radius-pill);
    padding: 1px 6px;
    line-height: 1.3;
  }
  .guide-select-badge.warn {
    color: var(--accent-amber);
    background: var(--accent-amber-bg, #faf3e3);
  }
  .guide-select-badge.info {
    color: var(--accent-blue);
    background: rgba(36, 86, 198, 0.08);
  }
  .summary-preset-chevron {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    color: var(--text-faint);
    transition: transform 160ms var(--ease-spring);
  }
  .summary-preset-chevron.open {
    transform: rotate(180deg);
  }
  .speaker-popover-backdrop {
    position: fixed;
    inset: 0;
    z-index: 85;
    background: transparent;
  }
  .summary-guide-popover {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    width: 100%;
    z-index: 90;
    box-sizing: border-box;
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
  .guide-sections-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .guide-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .guide-sections-flow {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .guide-section-step {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: var(--bg-inset);
    border-radius: var(--radius-sm);
    padding: 3px 8px;
    font-size: 11.5px;
  }
  .guide-step-num {
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 700;
    color: var(--text-faint);
  }
  .guide-step-title {
    font-weight: 500;
    color: var(--text-primary);
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
    max-height: 200px;
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
