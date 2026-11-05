<script lang="ts">
  import { onMount } from 'svelte'
  import { loadHotwords, saveHotwords, settings } from '../lib/settings.svelte.ts'
  import { toast } from '../lib/toast.svelte.ts'

  let termInput = $state('')

  onMount(() => {
    if (!settings.hotwordsLoaded) {
      void loadHotwords()
    }
  })

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

  async function addTerm(): Promise<void> {
    let t = termInput.trim()
    if (!t) return

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
</script>

<div class="dict-page">
  <div class="dict-head">
    <div>
      <h2 class="card-title">Словарь терминов</h2>
      <p class="help-text">
        Подсказки на слух отдают предпочтение терминам, именам и сленгу.
        Автозамена («было = стало» или «было -> стало») исправляет частые ошибки распознавания.
      </p>
    </div>
    <span class="dict-count-badge">{settings.hotwords.length} из 500</span>
  </div>

  <div class="dict-input-wrap">
    <div class="dict-input-box">
      <input
        type="text"
        class="input-field dict-input"
        placeholder="Поиск или термин («Docker», «мерши = merge»)"
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
    </div>
    <button
      class="btn-primary"
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
    <div class="dict-list custom-scroll" role="list" aria-label="Термины в словаре">
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
  {:else if settings.hotwords.length === 0}
    <div class="dict-empty">
      Словарь пока пуст. Добавьте профессиональные термины, аббревиатуры или частые фамилии для повышения точности распознавания.
    </div>
  {:else}
    <div class="dict-empty">
      По запросу «{termInput}» ничего не найдено. Нажмите «Добавить», чтобы включить термин в словарь.
    </div>
  {/if}
</div>

<style>
  .dict-page {
    display: flex;
    flex-direction: column;
    gap: 14px;
    height: 100%;
    overflow: hidden;
  }
  .dict-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }
  .card-title {
    font-size: 16px;
    font-weight: 700;
    color: var(--text-primary);
    margin-bottom: 2px;
  }
  .help-text {
    font-size: 12.5px;
    color: var(--text-muted);
    line-height: 1.45;
  }
  .dict-count-badge {
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--text-muted);
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    padding: 3px 8px;
    border-radius: var(--radius-pill);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .dict-input-wrap {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .dict-input-box {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
  }
  .dict-input {
    width: 100%;
    padding-right: 32px;
  }
  .dict-input-clear {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    border: none;
    background: transparent;
    cursor: pointer;
    color: var(--text-faint);
    font-size: 13px;
    line-height: 1;
    padding: 6px;
    border-radius: 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .dict-input-clear:hover {
    color: var(--text-primary);
  }
  .dict-add-bar {
    font-size: 12px;
    color: var(--accent-blue);
    background: var(--bg-inset);
    border: 1px solid var(--border-soft);
    border-radius: var(--radius-sm);
    padding: 7px 12px;
    margin-top: -6px;
  }
  .dict-list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    background: #fff;
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
  }
  .dict-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 14px;
    gap: 12px;
    font-size: 13px;
    border-bottom: 1px solid var(--border-soft);
    transition: background 120ms ease;
  }
  .dict-row:last-child {
    border-bottom: none;
  }
  .dict-row:hover {
    background: var(--bg-inset);
  }
  .dict-row-text {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
  }
  .dict-word {
    font-weight: 600;
    color: var(--text-primary);
    word-break: break-word;
  }
  .dict-heard {
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: var(--text-faint);
  }
  .dict-arrow {
    color: var(--text-faint);
    font-weight: 700;
    user-select: none;
  }
  .dict-correct {
    font-weight: 700;
    color: var(--text-primary);
    word-break: break-word;
  }
  .dict-row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }
  .dict-kind {
    font-size: 10.5px;
    font-weight: 700;
    padding: 2px 7px;
    border-radius: var(--radius-pill);
    border: 1px solid transparent;
    letter-spacing: 0.02em;
  }
  .dict-kind-word {
    color: var(--accent-blue);
    background: #f0f4ff;
    border-color: #dbe4fe;
  }
  .dict-kind-sub {
    color: var(--accent-amber);
    background: var(--accent-amber-bg);
    border-color: #f6e6be;
  }
  .dict-del {
    border: none;
    background: none;
    cursor: pointer;
    color: var(--text-faint);
    font-size: 13px;
    line-height: 1;
    min-width: 36px;
    min-height: 36px;
    padding: 6px;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: color 120ms ease, background 120ms ease;
  }
  .dict-del:hover {
    color: var(--accent-rec);
    background: #fee2e2;
  }
  .dict-empty {
    padding: 32px 16px;
    text-align: center;
    font-size: 13px;
    color: var(--text-muted);
    background: var(--bg-inset);
    border: 1px dashed var(--border-card);
    border-radius: var(--radius-inner);
  }

  @media (max-width: 640px) {
    .card-title {
      font-size: 15px;
    }
    .dict-head {
      flex-direction: column;
      gap: 6px;
    }
    .dict-count-badge {
      align-self: flex-start;
    }
    .dict-row {
      padding: 10px 12px;
    }
  }
</style>
