<script lang="ts">
import { pluralRecords } from '../lib/api'
import { copyMethod, support } from '../lib/support.svelte.ts'
import { ui } from '../lib/ui.svelte.ts'

const savedLabel = (minutes: number): string => {
  const total = Math.max(0, Math.round(minutes))
  if (total < 60) return `${total} мин`
  const h = Math.floor(total / 60)
  const m = total % 60
  return m > 0 ? `${h} ч ${m} мин` : `${h} ч`
}

const onWindowKey = (e: KeyboardEvent): void => {
  if (ui.donateOpen && e.key === 'Escape') {
    ui.donateOpen = false
  }
}
</script>

<svelte:window onkeydown={onWindowKey} />

{#if ui.donateOpen}
  <div
    class="modal-overlay open"
    role="dialog" tabindex="-1"
    aria-modal="true"
    aria-label="Поддержка проекта"
    onmousedown={(e) => {
      if (e.target === e.currentTarget) ui.donateOpen = false
    }}
  >
    <div class="modal-box donate-box">
      <div class="modal-top">
        <div class="modal-title">Поддержать разработку</div>
        <button class="icon-btn" aria-label="Закрыть окно" onclick={() => (ui.donateOpen = false)}>✕</button>
      </div>
      <div class="help-text">
        <p>
          «Подтекст» развивается как бесплатный, локальный и открытый инструмент – без платных
          подписок и облаков.
        </p>
        <p style="margin-top: 6px;">
          Если приложение экономит вам время, вы можете поддержать автора. Донаты помогают развивать
          проект, исправлять ошибки и выпускать новые версии.
        </p>
      </div>
      {#if support.transcribedCount > 0}
        <div class="support-stats">
          <span class="support-stats-value">≈ {savedLabel(support.savedMinutes)}</span>
          <span class="support-stats-label">
            вы уже сэкономили на расшифровке · {support.transcribedCount}
            {pluralRecords(support.transcribedCount)}
          </span>
        </div>
      {/if}
      <div class="recordings-list">
        {#if support.methods.length === 0}
          <div class="empty-note">
            Способы поддержки появятся в ближайшем обновлении – проект уже можно
            посоветовать коллегам, это тоже большая помощь.
          </div>
        {:else}
          {#each support.methods as m, i (i)}
            <div class="rec-item">
              <div class="rec-info">
                <span class="rec-name">{m.title || ''}</span>
                <span class="rec-meta">{m.desc || m.url || m.address || ''}</span>
              </div>
              {#if m.url}
                <a class="btn-primary btn-sm" href={m.url} target="_blank" rel="noopener noreferrer">
                  Поддержать
                </a>
              {:else}
                <button class="btn-secondary btn-sm" onclick={() => void copyMethod(m)}>Копировать</button>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
      <div class="row-end">
        <button class="btn-secondary" onclick={() => (ui.donateOpen = false)}>Закрыть</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .support-stats {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 10px;
  }
  .support-stats-value {
    color: var(--accent-blue);
    font-size: 20px;
    font-weight: 800;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }
  .support-stats-label {
    color: var(--text-muted);
    font-size: 12.5px;
    line-height: 1.5;
  }
</style>
