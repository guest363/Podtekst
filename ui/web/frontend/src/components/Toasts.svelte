<script lang="ts">
  import { dismissToast, toasts } from '../lib/toast.svelte.ts'
</script>

<div class="toast-stack" aria-live="polite">
  {#each toasts as t (t.id)}
    <div class="toast {t.kind}">
      <span class="toast-msg">{t.msg}</span>
      {#if t.actions.length > 0}
        {#each t.actions as a}
          <button onclick={a.run}>{a.label}</button>
        {/each}
      {:else if t.sticky}
        <button onclick={() => dismissToast(t.id)}>Закрыть</button>
      {/if}
    </div>
  {/each}
</div>
