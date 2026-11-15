<script lang="ts">
import { dismissToast, toasts } from '../lib/toast.svelte.ts'
</script>

<div class="toast-stack">
  {#each toasts as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === 'error' ? 'alert' : 'status'} aria-live={t.kind === 'error' ? 'assertive' : 'polite'}>
      <span class="toast-msg">{t.msg}</span>
      {#if t.actions.length > 0}
        {#each t.actions as a}
          <button type="button" onclick={a.run}>{a.label}</button>
        {/each}
      {:else if t.sticky}
        <button type="button" onclick={() => dismissToast(t.id)}>Закрыть</button>
      {/if}
    </div>
  {/each}
</div>
