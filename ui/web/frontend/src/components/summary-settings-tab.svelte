<script lang="ts">
import { onDestroy, onMount, tick } from 'svelte'
import type { SummarySettings } from '../lib/api'
import { isMobile } from '../lib/api'
import { errText, isRecord } from '../lib/guards.ts'
import {
  cancelSummaryGpu,
  installSummaryGpu,
  loadSummarySettings,
  refreshSummaryGpu,
  removeSummaryGpu,
  saveSummarySettings,
  summaryModelPct,
  summaryState,
} from '../lib/summary.svelte.ts'
import { summaryErrorText } from '../lib/summary-ui.ts'
import { fmtBytes, loadSystem, openTarget, system } from '../lib/system.svelte.ts'
import { toast, toastConfirm } from '../lib/toast.svelte.ts'
import { downloadModel } from '../lib/transcript.svelte.ts'
import SummaryPromptModal from './summary-prompt-modal.svelte'

let loading = $state(false)
let saving = $state(false)
let error = $state('')
let presetOpen = $state(false)
let presetTrigger = $state<HTMLButtonElement | null>(null)
let presetList = $state<HTMLDivElement | null>(null)
let guideOpen = $state(false)
let guidePresetId = $state('')

const settings = $derived(summaryState.settings)
const manualOnly = $derived(summaryState.capabilities?.classifier === 'manual_only')
const model = $derived(summaryState.model)
const modelPct = $derived(summaryModelPct())
const gpu = $derived(summaryState.gpu)
const gpuEnabled = $derived(settings?.gpu !== 'off')
const gpuBadge = $derived.by(() => {
  if (!gpu) return 'Проверка…'
  if (gpu.installing) return 'Установка…'
  if (gpu.bundled) return 'Встроено'
  if (gpu.has_update) return 'Обновление'
  if (gpu.installed) return 'Установлено'
  return 'Не установлено'
})
const gpuDownloadText = $derived(
  gpu?.compressed_bytes
    ? `Скачать ускорение GPU (${fmtBytes(gpu.compressed_bytes)})`
    : 'Скачать ускорение GPU',
)
const autoSuggest = $derived(settings?.auto_suggest ?? true)
const defaultPreset = $derived(settings?.default_preset ?? '')
const presetOptions = $derived([
  { id: '', label: 'Авто — по типу разговора' },
  ...summaryState.presets.map((p) => ({ id: p.id, label: p.label })),
])
const selectedPresetLabel = $derived(
  presetOptions.find((p) => p.id === defaultPreset)?.label ?? presetOptions[0].label,
)
const detail = $derived(settings?.detail ?? 'normal')
const modelSizeGb = $derived(
  ((model?.total_size_bytes || 2497281120) / 1073741824).toFixed(1).replace('.', ','),
)
const ramGib = $derived(((model?.ram_mb || 5530) / 1024).toFixed(1).replace('.', ','))
const requiredFreeGib = $derived(
  ((model?.required_free_mb || 6656) / 1024).toFixed(1).replace('.', ','),
)
const diskFree = $derived(fmtBytes(system.snapshot?.disk?.free_bytes) || '—')
const ramText = $derived.by(() => {
  const total = fmtBytes(system.snapshot?.memory?.total_bytes)
  if (total) return `${total} ОЗУ`
  const available = fmtBytes(system.snapshot?.memory?.available_bytes)
  return available ? `${available} ОЗУ доступно` : '—'
})

const detailBadge = $derived.by(() => {
  switch (detail) {
    case 'short':
      return 'Быстрый срез'
    case 'long':
      return 'Максимум деталей'
    default:
      return 'Сбалансировано'
  }
})

const detailInfo = $derived.by(() => {
  switch (detail) {
    case 'short':
      return {
        title: 'Кратко:',
        desc: 'только суть — 2–3 главных решения и ключевые задачи без второстепенного контекста (~1 мин чтения).',
      }
    case 'long':
      return {
        title: 'Полный протокол:',
        desc: 'подробный контекст каждого вопроса, позиции участников, все задачи, открытые вопросы и цитаты (~5–7 мин чтения).',
      }
    default:
      return {
        title: 'Сбалансированный вариант:',
        desc: 'краткий ход встречи, задачи с ответственными и сроками, принятые решения (~2–3 мин чтения).',
      }
  }
})

const reload = async (): Promise<void> => {
  loading = true
  error = ''
  try {
    await loadSummarySettings()
    if (summaryState.gpu?.installing) startGpuPoll()
  } catch (e) {
    error = `Не удалось загрузить настройки итогов: ${errText(e)}`
  } finally {
    loading = false
  }
}

let gpuPoll: ReturnType<typeof setInterval> | null = null

const stopGpuPoll = (): void => {
  if (gpuPoll) clearInterval(gpuPoll)
  gpuPoll = null
}

const startGpuPoll = (): void => {
  stopGpuPoll()
  gpuPoll = setInterval(async () => {
    try {
      await refreshSummaryGpu()
    } catch {
      return
    }
    if (!summaryState.gpu?.installing) stopGpuPoll()
  }, 500)
}

onMount(() => {
  void reload()
  if (!system.snapshot) void loadSystem()
  if (summaryState.gpu?.installing) startGpuPoll()
})

onDestroy(stopGpuPoll)

const installGpu = async (): Promise<void> => {
  if (!gpu || gpu.installing) return
  error = ''
  try {
    await installSummaryGpu()
    startGpuPoll()
  } catch (e) {
    const code = isRecord(e) && typeof e.code === 'string' ? e.code : 'gpu_internal'
    error = summaryErrorText(code, errText(e))
    toast(error, 'error')
    await refreshSummaryGpu().catch(() => {})
  }
}

const cancelGpu = async (): Promise<void> => {
  try {
    await cancelSummaryGpu()
  } catch (e) {
    toast(`Не удалось отменить скачивание: ${errText(e)}`, 'error')
  }
}

const removeGpu = async (): Promise<void> => {
  try {
    await removeSummaryGpu()
    toast('GPU-пакет удалён. Итоги работают на CPU.')
  } catch (e) {
    const code = isRecord(e) && typeof e.code === 'string' ? e.code : 'gpu_remove_failed'
    toast(summaryErrorText(code, errText(e)), 'error')
  }
}

const confirmRemoveGpu = (): void => {
  toastConfirm(
    'Удалить ускорение GPU? Итоги продолжат работать на CPU.',
    'Удалить',
    () => void removeGpu(),
  )
}

const toggleGpu = (): void => {
  void save({ gpu: gpuEnabled ? 'off' : 'auto' })
}

const save = async (patch: Partial<SummarySettings>): Promise<void> => {
  if (saving) return
  saving = true
  error = ''
  try {
    await saveSummarySettings(patch)
  } catch (e) {
    error = `Не удалось сохранить: ${errText(e)}`
    toast(error, 'error')
    await reload()
  } finally {
    saving = false
  }
}

const download = async (): Promise<void> => {
  if (!model || modelPct !== null) return
  const ok = await downloadModel(model)
  if (ok) await reload()
}

const openPresetList = (): void => {
  if (saving) return
  presetOpen = true
  void tick().then(() => {
    const current = presetList?.querySelector<HTMLElement>('[aria-selected="true"]')
    const first = presetList?.querySelector<HTMLElement>('.speaker-select-item')
    ;(current ?? first)?.focus()
  })
}

const closePreset = (returnFocus = false): void => {
  presetOpen = false
  if (returnFocus) presetTrigger?.focus()
}

const choosePreset = (id: string): void => {
  closePreset(true)
  if (id === defaultPreset) return
  void save({ default_preset: id || null })
}

const openGuide = (presetId?: string): void => {
  guidePresetId = presetId || defaultPreset || summaryState.presets[0]?.id || 'meeting'
  guideOpen = true
}

const onPresetKeydown = (e: KeyboardEvent): void => {
  if (e.key === 'Escape' && presetOpen) {
    e.stopPropagation()
    closePreset(true)
    return
  }
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
  e.preventDefault()
  if (!presetOpen) {
    openPresetList()
    return
  }
  const items = [...(presetList?.querySelectorAll<HTMLElement>('.speaker-select-item') ?? [])]
  if (!items.length) return
  const active = document.activeElement
  const index = active instanceof HTMLElement ? items.indexOf(active) : -1
  const next =
    e.key === 'ArrowDown'
      ? items[(index + 1) % items.length]
      : items[(index - 1 + items.length) % items.length]
  next?.focus()
}

const onPresetFocusOut = (e: FocusEvent): void => {
  const next = e.relatedTarget instanceof Node ? e.relatedTarget : null
  if (
    presetOpen &&
    (!next || !(e.currentTarget instanceof HTMLElement && e.currentTarget.contains(next)))
  ) {
    presetOpen = false
  }
}
</script>

<div class="settings-pane summary-settings" role="tabpanel" tabindex="0" aria-label="Итоги">
  {#if loading && !settings}
    <div class="help-text">Загружаю настройки итогов…</div>
  {:else if error && !settings}
    <div class="summary-settings-error" role="alert">
      <span>{error}</span>
      <button class="btn-secondary btn-sm" onclick={() => void reload()}>Повторить</button>
    </div>
  {:else}
    <!-- Секция 1: Автоматизация и шаблоны -->
    <div class="summary-section">
      <div class="card-title">Итоги встречи</div>

      <label class="setting-check">
        <input
          type="checkbox"
          checked={autoSuggest}
          disabled={manualOnly || saving}
          onchange={(e) => void save({ auto_suggest: e.currentTarget.checked })}
        />
        <span class="setting-switch" aria-hidden="true"></span>
        <span>Предлагать шаблон после расшифровки</span>
      </label>
      <div class="help-text">
        Определять тип разговора и показывать рекомендацию. Итоги формируются только по явной кнопке.
      </div>
      {#if manualOnly}
        <div class="help-text summary-settings-warn">
          Рекомендация отключена: недостаточно проверенных данных.
        </div>
      {/if}

      <div class="summary-setting-row">
        <div class="summary-setting-col">
          <span id="summary-default-preset-label">Шаблон по умолчанию</span>
          <button
            type="button"
            class="summary-guide-link"
            onclick={() => openGuide()}
          >
            Справочник шаблонов и промптов
          </button>
        </div>
        <div class="summary-preset-select" onfocusout={onPresetFocusOut}>
          <button
            bind:this={presetTrigger}
            type="button"
            class="summary-select"
            aria-haspopup="listbox"
            aria-expanded={presetOpen}
            aria-labelledby="summary-default-preset-label"
            disabled={saving}
            onclick={() => (presetOpen ? closePreset() : openPresetList())}
            onkeydown={onPresetKeydown}
          >
            <span class="summary-preset-value">{selectedPresetLabel}</span>
            <span class="summary-preset-chevron {presetOpen ? 'open' : ''}" aria-hidden="true">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </span>
          </button>
          {#if presetOpen}
            <button
              type="button"
              class="speaker-popover-backdrop"
              aria-label="Закрыть список шаблонов"
              onclick={() => closePreset(true)}
            ></button>
            <div
              bind:this={presetList}
              class="speaker-select-popover summary-preset-popover"
              role="listbox"
              tabindex="-1"
              aria-labelledby="summary-default-preset-label"
              onkeydown={onPresetKeydown}
            >
              {#each presetOptions as p (p.id)}
                <button
                  type="button"
                  class="speaker-select-item"
                  role="option"
                  aria-selected={p.id === defaultPreset}
                  onclick={() => choosePreset(p.id)}
                >
                  <span class="speaker-select-item-left">
                    <span
                      class="speaker-select-item-dot"
                      style={`background: ${p.id === defaultPreset ? 'var(--accent-blue)' : '#94a3b8'}`}
                    ></span>
                    <span class="speaker-select-item-name">{p.label}</span>
                  </span>
                </button>
              {/each}
              <div class="summary-popover-divider"></div>
              <button
                type="button"
                class="speaker-select-item summary-popover-action"
                onclick={() => { closePreset(); openGuide(); }}
              >
                <span class="speaker-select-item-name">Справочник всех шаблонов…</span>
              </button>
            </div>
          {/if}
        </div>
      </div>

      <div class="summary-setting-row">
        <span>Язык итогов</span>
        <span class="summary-row-note">как в расшифровке</span>
      </div>
    </div>

    <!-- Секция 2: Объём выжимки с живым пояснением -->
    <div class="summary-section summary-section-divider">
      <div class="summary-section-head">
        <div class="card-title">Объём выжимки</div>
        <span class="detail-badge">{detailBadge}</span>
      </div>

      <div class="seg-bar" role="group" aria-label="Объём выжимки">
        <button
          class="seg-item {detail === 'short' ? 'active' : ''}"
          disabled={saving}
          onclick={() => void save({ detail: 'short' })}
        >
          Кратко
        </button>
        <button
          class="seg-item {detail === 'normal' ? 'active' : ''}"
          disabled={saving}
          onclick={() => void save({ detail: 'normal' })}
        >
          Обычно
        </button>
        <button
          class="seg-item {detail === 'long' ? 'active' : ''}"
          disabled={saving}
          onclick={() => void save({ detail: 'long' })}
        >
          Подробно
        </button>
      </div>

      <div class="detail-hint">
        <strong>{detailInfo.title}</strong> {detailInfo.desc}
      </div>
    </div>

    <!-- Секция 3: Модель и ресурсы -->
    <div class="summary-section summary-section-divider">
      <div class="card-title">Модель и ресурсы</div>

      <div class="summary-model-card">
        <div class="summary-model-head">
          <div class="summary-model-meta-col">
            <span class="summary-model-title">{model?.title || 'Qwen3-4B Instruct 2507 (Q4_K_M)'}</span>
            <span class="summary-model-sub">{modelSizeGb} ГБ · локальный инференс без сети</span>
          </div>
          {#if modelPct !== null}
            <span class="summary-state info">Скачивание {modelPct}%</span>
          {:else if model?.is_installed}
            <span class="summary-state ok">Скачана</span>
          {:else}
            <span class="summary-state warn">Не скачана</span>
          {/if}
        </div>

        {#if modelPct !== null}
          <div
            class="vu-track progress-track"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={modelPct}
            aria-label="Скачивание модели итогов"
          >
            <div class="vu-bar" style={`width: ${modelPct}%`}></div>
          </div>
        {/if}

        <div class="system-facts">
          <span class="system-key">Требуется ОЗУ</span>
          <span class="system-val">≈{ramGib} ГиБ (нужно ≥{requiredFreeGib} ГиБ свободно)</span>
          <span class="system-key">Доступно в системе</span>
          <span class="system-val">{ramText}</span>
          <span class="system-key">Свободно на диске</span>
          <span class="system-val">{diskFree}</span>
        </div>

        {#if modelPct === null}
          <div class="summary-model-foot">
            {#if !model?.is_installed}
              <button class="btn-primary btn-sm" disabled={!model} onclick={() => void download()}>
                Скачать
              </button>
            {/if}
            {#if model?.license_url}
              <a class="summary-link" href={model.license_url} target="_blank" rel="noreferrer">
                {model.license || 'Лицензия'}
              </a>
            {/if}
            {#if model?.source_url}
              <a class="summary-link" href={model.source_url} target="_blank" rel="noreferrer">
                Источник весов
              </a>
            {/if}
          </div>
        {/if}
      </div>

      {#if !isMobile}
        <div class="row-start pt-1">
          <button class="btn-secondary btn-sm" onclick={() => void openTarget('models')}>
            Открыть папку моделей
          </button>
        </div>
      {/if}

      <div class="help-text">
        Анализ выполняется полностью на этом компьютере без передачи данных в сеть.
      </div>
    </div>

    {#if gpu?.supported}
      <div class="summary-section summary-section-divider">
        <div class="summary-section-head">
          <div class="card-title">Ускорение GPU</div>
          <span class="detail-badge">{gpuBadge}</span>
        </div>

        <div class="summary-model-card">
          <div class="summary-model-meta-col">
            <span class="summary-model-title">{gpu?.title || 'GPU'}</span>
            <span class="summary-model-sub">{gpu?.description || ''}</span>
          </div>

          {#if gpu?.device}
            <div class="system-facts">
              <span class="system-key">Устройство</span>
              <span class="system-val">{gpu.device}</span>
              <span class="system-key">На диске</span>
              <span class="system-val">{fmtBytes(gpu.installed_bytes)}</span>
            </div>
          {:else if gpu?.installed && !gpu.bundled}
            <div class="help-text summary-settings-warn">
              Vulkan-устройство не найдено. Итоги продолжат работать на CPU.
            </div>
          {/if}

          {#if gpu?.installing}
            <div
              class="vu-track progress-track"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={gpu.pct}
              aria-label="Установка ускорения GPU"
            >
              <div class="vu-bar" style={`width: ${gpu.pct}%`}></div>
            </div>
            <div class="help-text" role="status">{gpu.stage || 'Установка…'} {gpu.pct}%</div>
            <div class="summary-model-foot">
              <button class="btn-secondary btn-sm" onclick={() => void cancelGpu()}>Отменить</button>
            </div>
          {:else}
            {#if gpu?.error}
              <div class="summary-settings-error" role="alert">
                <span>{gpu.error}</span>
                {#if !gpu.installed}
                  <button class="btn-secondary btn-sm" onclick={() => void installGpu()}>Повторить</button>
                {/if}
              </div>
            {/if}

            {#if gpu?.installed}
              <label class="setting-check">
                <input
                  type="checkbox"
                  checked={gpuEnabled}
                  disabled={saving}
                  onchange={toggleGpu}
                />
                <span class="setting-switch" aria-hidden="true"></span>
                <span>Использовать GPU, если доступен</span>
              </label>
              <div class="help-text">
                Если GPU-пакет не загрузится, итоги автоматически перейдут на CPU.
              </div>
              <div class="summary-model-foot">
                {#if gpu.has_update}
                  <button
                    class="btn-primary btn-sm"
                    disabled={summaryState.generating}
                    onclick={() => void installGpu()}
                  >
                    Обновить
                  </button>
                {/if}
                {#if !gpu.bundled}
                  <button
                    class="btn-secondary btn-sm"
                    disabled={summaryState.generating}
                    onclick={confirmRemoveGpu}
                  >
                    Удалить
                  </button>
                {/if}
                {#if gpu.license_url}
                  <a class="summary-link" href={gpu.license_url} target="_blank" rel="noreferrer">
                    {gpu.license || 'Лицензия'}
                  </a>
                {/if}
              </div>
            {:else if !gpu?.bundled}
              <div class="help-text">
                Необязательный пакет: ускоряет итоги на видеокартах Intel, AMD и NVIDIA. Без него всё работает на CPU.
              </div>
              <div class="summary-model-foot">
                <button
                  class="btn-primary btn-sm"
                  disabled={gpu?.installing || !gpu?.supported || summaryState.generating}
                  onclick={() => void installGpu()}
                >
                  {gpuDownloadText}
                </button>
                {#if gpu?.license_url}
                  <a class="summary-link" href={gpu.license_url} target="_blank" rel="noreferrer">
                    {gpu.license || 'Лицензия'}
                  </a>
                {/if}
              </div>
            {/if}
          {/if}
        </div>
      </div>
    {/if}

    {#if error}
      <div class="summary-settings-error" role="alert">
        <span>{error}</span>
        <button class="btn-secondary btn-sm" onclick={() => void reload()}>Повторить</button>
      </div>
    {/if}
  {/if}

  <SummaryPromptModal
    open={guideOpen}
    presets={summaryState.presets}
    initialId={guidePresetId}
    defaultPresetId={defaultPreset}
    systemPromptBase={summaryState.systemPromptBase}
    onselect={(id) => choosePreset(id)}
    onclose={() => (guideOpen = false)}
  />
</div>

<style>
  .summary-settings {
    gap: 16px;
  }
  .summary-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .summary-section-divider {
    border-top: 1px solid var(--border-soft);
    padding-top: 14px;
  }
  .summary-section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
  }
  .detail-badge {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent-blue);
    background: rgba(37, 99, 235, 0.08);
    border: 1px solid rgba(37, 99, 235, 0.2);
    border-radius: var(--radius-sm);
    padding: 2px 7px;
    white-space: nowrap;
  }
  .detail-hint {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-muted);
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner);
    padding: 9px 12px;
  }
  .detail-hint strong {
    color: var(--text-primary);
    font-weight: 600;
  }
  .summary-setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-primary);
  }
  .summary-setting-col {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
  }
  .summary-guide-link {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: 11px;
    font-weight: 500;
    color: var(--accent-blue);
    cursor: pointer;
    text-underline-offset: 2px;
  }
  .summary-guide-link:hover {
    text-decoration: underline;
  }
  .summary-popover-divider {
    height: 1px;
    background: var(--border-soft);
    margin: 4px 0;
  }
  .summary-popover-action {
    color: var(--accent-blue);
    font-weight: 600;
    font-size: 12px;
  }
  .summary-row-note {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-faint);
  }
  .summary-preset-select {
    position: relative;
    min-width: 240px;
    max-width: 100%;
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
    padding: 7px 9px;
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
  }
  .summary-select:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-select:disabled {
    cursor: default;
    opacity: 0.65;
  }
  .summary-preset-value {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .summary-preset-popover {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    width: 100%;
    min-width: 0;
    max-width: 100%;
    max-height: 260px;
    z-index: 70;
    box-sizing: border-box;
  }
  .summary-model-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: var(--bg-inset);
    padding: 12px 14px;
  }
  .summary-model-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }
  .summary-model-meta-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .summary-model-title {
    font-size: 13px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .summary-model-sub {
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .summary-state {
    font-size: 11.5px;
    font-weight: 700;
    border: 1px solid currentColor;
    border-radius: var(--radius-pill);
    padding: 1px 7px;
    white-space: nowrap;
  }
  .summary-state.ok {
    color: var(--accent-ok);
  }
  .summary-state.warn {
    color: var(--accent-amber);
  }
  .summary-state.info {
    color: var(--accent-blue);
  }
  .summary-model-card .system-facts {
    display: grid;
    grid-template-columns: minmax(130px, max-content) minmax(0, 1fr);
    gap: 4px 12px;
    font-size: 12px;
    padding-top: 8px;
    border-top: 1px solid var(--border-soft);
  }
  .system-key {
    color: var(--text-faint);
  }
  .system-val {
    color: var(--text-primary);
    font-weight: 500;
    word-break: break-word;
  }
  .summary-model-foot {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    padding-top: 2px;
  }
  .summary-link {
    color: var(--accent-blue);
    font-size: 11.5px;
    text-underline-offset: 2px;
  }
  .summary-settings-error {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    background: #fdf2f2;
    border: 1px solid #f0caca;
    border-radius: var(--radius-inner);
    padding: 8px 10px;
    color: var(--accent-rec);
    font-size: 12px;
  }
  .summary-settings-warn {
    color: var(--accent-amber);
  }
  .pt-1 {
    padding-top: 4px;
  }
  @media (max-width: 720px) {
    .summary-setting-row {
      flex-direction: column;
      align-items: stretch;
      gap: 6px;
    }
    .summary-preset-select {
      width: 100%;
      min-width: 0;
    }
    .summary-select {
      min-width: 0;
      width: 100%;
      min-height: 40px;
    }
    .summary-model-card .system-facts {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    .summary-model-card .btn-primary {
      width: 100%;
      min-height: 40px;
    }
  }
</style>
