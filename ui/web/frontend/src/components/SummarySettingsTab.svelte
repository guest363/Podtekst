<script lang="ts">
  import { onMount } from 'svelte'
  import type { SummaryDetail } from '../lib/api'
  import { isMobile } from '../lib/api'
  import { downloadModel } from '../lib/transcript.svelte.ts'
  import { fmtBytes, loadSystem, openTarget, system } from '../lib/system.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'
  import {
    loadSummarySettings,
    saveSummarySettings,
    summaryModelPct,
    summaryState,
  } from '../lib/summary.svelte.ts'

  let loading = $state(false)
  let saving = $state(false)
  let error = $state('')

  const settings = $derived(summaryState.settings)
  const manualOnly = $derived(summaryState.capabilities?.classifier === 'manual_only')
  const model = $derived(summaryState.model)
  const modelPct = $derived(summaryModelPct())
  const autoSuggest = $derived(settings?.auto_suggest ?? true)
  const defaultPreset = $derived(settings?.default_preset ?? '')
  const detail = $derived(settings?.detail ?? 'normal')
  const modelSizeGb = $derived(
    ((model?.total_size_bytes || 2497281120) / 1073741824).toFixed(1).replace('.', ','),
  )
  const ramGib = $derived((((model?.ram_mb || 5530) / 1024)).toFixed(1).replace('.', ','))
  const requiredFreeGib = $derived((((model?.required_free_mb || 6656) / 1024)).toFixed(1).replace('.', ','))
  const diskFree = $derived(fmtBytes(system.snapshot?.disk?.free_bytes) || '—')
  const ramText = $derived.by(() => {
    const total = fmtBytes(system.snapshot?.memory?.total_bytes)
    if (total) return `${total} ОЗУ`
    const available = fmtBytes(system.snapshot?.memory?.available_bytes)
    return available ? `${available} ОЗУ доступно` : '—'
  })

  async function reload(): Promise<void> {
    loading = true
    error = ''
    try {
      await loadSummarySettings()
    } catch (e) {
      error = 'Не удалось загрузить настройки итогов: ' + (e as Error).message
    } finally {
      loading = false
    }
  }

  onMount(() => {
    void reload()
    if (!system.snapshot) void loadSystem()
  })

  async function save(patch: { auto_suggest?: boolean; default_preset?: string | null; detail?: SummaryDetail }): Promise<void> {
    if (saving) return
    saving = true
    error = ''
    try {
      await saveSummarySettings(patch)
    } catch (e) {
      error = 'Не удалось сохранить: ' + (e as Error).message
      toast(error, 'error')
      await reload()
    } finally {
      saving = false
    }
  }

  async function download(): Promise<void> {
    if (!model || modelPct !== null) return
    const ok = await downloadModel(model)
    if (ok) await reload()
  }
</script>

<div class="settings-pane summary-settings" role="tabpanel" tabindex="0" aria-label="Итоги">
  <div class="card-title">Итоги встречи</div>

  {#if loading && !settings}
    <div class="help-text">Загружаю настройки итогов…</div>
  {:else if error && !settings}
    <div class="summary-settings-error" role="alert">
      <span>{error}</span>
      <button class="btn-secondary btn-sm" onclick={() => void reload()}>Повторить</button>
    </div>
  {:else}
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
      Определять тип разговора и показывать рекомендацию. Итоги не собираются без нажатия.
    </div>
    {#if manualOnly}
      <div class="help-text summary-settings-warn">
        Рекомендация отключена: недостаточно проверенных данных.
      </div>
    {/if}

    <div class="summary-setting-row">
      <label for="summary-default-preset">Шаблон по умолчанию</label>
      <select
        id="summary-default-preset"
        class="summary-select"
        value={defaultPreset}
        disabled={saving}
        onchange={(e) => void save({ default_preset: e.currentTarget.value || null })}
      >
        <option value="">Авто — по типу разговора</option>
        {#each summaryState.presets as p (p.id)}
          <option value={p.id}>{p.label}</option>
        {/each}
      </select>
    </div>

    <div class="summary-setting-block">
      <div class="summary-setting-label">Подробность</div>
      <div class="seg-bar" role="group" aria-label="Подробность итогов">
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
    </div>

    <div class="summary-setting-row">
      <span>Язык итогов</span>
      <span class="help-text">как в расшифровке</span>
    </div>

    <div class="summary-setting-block">
      <div class="summary-setting-label">Модель итогов</div>
      <div class="summary-model-card">
      <div class="summary-model-head">
        <span class="summary-model-title">{model?.title || 'Qwen3-4B Instruct 2507 (итоги, Q4_K_M)'}</span>
        {#if modelPct !== null}
          <span class="summary-state info">Скачивание {modelPct}%</span>
        {:else if model?.is_installed}
          <span class="summary-state ok">Скачана</span>
        {:else}
          <span class="summary-state warn">Не скачана</span>
        {/if}
      </div>
      <div class="help-text">
        {modelSizeGb} ГБ · ≈{ramGib} ГиБ памяти · нужно не меньше {requiredFreeGib} ГиБ свободно ·
        рекомендуется 16 ГиБ ОЗУ
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
      {:else if !model?.is_installed}
        <div class="row-start">
          <button class="btn-primary btn-sm" disabled={!model} onclick={() => void download()}>Скачать</button>
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
      {:else}
        <div class="summary-model-links">
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
    </div>

    <div class="help-text">
      Свободно: {diskFree} на диске · {ramText}. Работает без сети.
    </div>

    {#if error}
      <div class="summary-settings-error" role="alert">
        <span>{error}</span>
        <button class="btn-secondary btn-sm" onclick={() => void reload()}>Повторить</button>
      </div>
    {/if}

    {#if !isMobile}
      <div class="row-start">
        <button class="btn-secondary btn-sm" onclick={() => void openTarget('models')}>
          Открыть папку моделей
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .summary-settings {
    gap: 10px;
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
  .summary-setting-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .summary-setting-label {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-primary);
  }
  .summary-select {
    min-width: 240px;
    max-width: 100%;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    color: var(--text-primary);
    padding: 7px 9px;
    font: inherit;
    font-size: 12.5px;
  }
  .summary-select:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
  }
  .summary-model-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-inner);
    background: var(--bg-inset);
    padding: 12px;
  }
  .summary-model-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
  }
  .summary-model-title {
    font-size: 13px;
    font-weight: 700;
    color: var(--text-primary);
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
  .summary-model-links {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
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
  @media (max-width: 720px) {
    .summary-setting-row {
      flex-direction: column;
      align-items: stretch;
    }
    .summary-select {
      min-width: 0;
      width: 100%;
      min-height: 40px;
    }
    .summary-model-card .btn-primary {
      width: 100%;
      min-height: 40px;
    }
  }
</style>
