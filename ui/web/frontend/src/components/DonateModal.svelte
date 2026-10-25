<script lang="ts">
  import { copyMethod, support } from '../lib/support.svelte.ts'
  import { ui } from '../lib/ui.svelte.ts'

  function onWindowKey(e: KeyboardEvent): void {
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
          «Запись встреч» развивается как бесплатный, локальный и открытый инструмент – без платных
          подписок и облаков.
        </p>
        <p style="margin-top: 6px;">
          Если приложение экономит вам время, вы можете поддержать автора. Донаты помогают развивать
          проект, исправлять ошибки и выпускать новые версии.
        </p>
      </div>
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
