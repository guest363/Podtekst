<script lang="ts">
type Props = {
  label: string
  open: boolean
  controls: string
  onclick: () => void
}
let { label, open, controls, onclick }: Props = $props()
</script>

<button
  type="button"
  class="metrics-glossary-toggle"
  aria-expanded={open}
  aria-controls={controls}
  onclick={onclick}
>
  <span>{label}</span>
  <span class="metrics-glossary-chevron {open ? 'open' : ''}" aria-hidden="true">
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="6 9 12 15 18 9"></polyline>
    </svg>
  </span>
</button>

<style>
  .metrics-glossary-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    width: fit-content;
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    font-size: 12px;
    font-weight: 700;
    color: var(--text-primary);
    cursor: pointer;
  }

  .metrics-glossary-toggle:hover {
    color: var(--accent-blue);
  }

  .metrics-glossary-toggle:focus-visible {
    outline: 2px solid var(--accent-blue);
    outline-offset: 2px;
    border-radius: 3px;
  }

  .metrics-glossary-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 12px;
    height: 12px;
    color: var(--text-faint);
    transition: transform 160ms var(--ease-spring);
    flex-shrink: 0;
  }

  .metrics-glossary-chevron.open {
    transform: rotate(180deg);
  }

  @media (max-width: 1400px) {
    .metrics-glossary-toggle {
      min-height: 44px;
    }
  }
</style>
