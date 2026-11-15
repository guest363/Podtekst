<script module lang="ts">
let counter = 0
export const nextInfoTipId = (): string => `info-tip-${++counter}`
</script>

<script lang="ts">
import { tick } from 'svelte'

type Props = {
  text: string
}
let { text }: Props = $props()

const id = nextInfoTipId()

let open = $state(false)
let btn = $state<HTMLButtonElement | null>(null)
let root = $state<HTMLSpanElement | null>(null)
let top = $state(0)
let left = $state(0)
let width = $state(272)
let above = $state(false)

const position = (): void => {
  if (!btn) return
  const rect = btn.getBoundingClientRect()
  width = Math.min(272, window.innerWidth - 16)
  left = Math.min(Math.max(8, rect.left - 8), window.innerWidth - width - 8)
  above = window.innerHeight - rect.bottom < 130 && rect.top > 130
  top = above ? rect.top - 6 : rect.bottom + 6
}

const toggle = async (): Promise<void> => {
  if (open) {
    open = false
    return
  }
  open = true
  await tick()
  position()
}

const close = (): void => {
  open = false
}

$effect(() => {
  if (!open) return
  const onPointerDown = (event: PointerEvent): void => {
    if (root && event.target instanceof Node && !root.contains(event.target)) open = false
  }
  const onScroll = (): void => {
    open = false
  }
  document.addEventListener('pointerdown', onPointerDown, true)
  window.addEventListener('scroll', onScroll, true)
  window.addEventListener('resize', position)
  return () => {
    document.removeEventListener('pointerdown', onPointerDown, true)
    window.removeEventListener('scroll', onScroll, true)
    window.removeEventListener('resize', position)
  }
})
</script>

<span class="info-tip" bind:this={root}>
  <button
    type="button"
    class="info-tip-btn"
    aria-label="Пояснение"
    aria-expanded={open}
    aria-controls={open ? id : undefined}
    aria-describedby={open ? id : undefined}
    bind:this={btn}
    onclick={toggle}
    onkeydown={(event) => {
      if (event.key === 'Escape' && open) {
        event.stopPropagation()
        close()
      }
    }}
  >
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9"></circle>
      <path d="M12 11v5"></path>
      <path d="M12 7.5h.01"></path>
    </svg>
  </button>

  {#if open}
    <span
      class="info-tip-popover"
      id={id}
      role="tooltip"
      style={`top:${top}px;left:${left}px;width:${width}px;transform:${above ? 'translateY(-100%)' : 'none'}`}
    >
      {text}
    </span>
  {/if}
</span>

<style>
  .info-tip {
    display: inline-flex;
    align-items: center;
    vertical-align: middle;
  }

  .info-tip-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    margin: -3px 0 -3px 4px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    flex-shrink: 0;
  }

  .info-tip-btn:hover {
    color: var(--accent-blue);
    background: rgba(36, 86, 198, 0.08);
  }

  .info-tip-btn:focus-visible {
    color: var(--accent-blue);
    outline: 2px solid var(--accent-blue);
    outline-offset: 1px;
  }

  .info-tip-popover {
    position: fixed;
    z-index: 90;
    display: block;
    padding: 9px 11px;
    border: 1px solid var(--border-card);
    border-radius: 10px;
    background: #fff;
    box-shadow: 0 10px 30px rgba(15, 30, 51, 0.16);
    font-size: 12px;
    font-weight: 400;
    line-height: 1.45;
    color: var(--text-primary);
    text-align: left;
    text-transform: none;
    letter-spacing: 0;
    white-space: normal;
  }

  @media (max-width: 720px) {
    .info-tip-btn {
      width: 24px;
      height: 24px;
      margin: -5px 0 -5px 4px;
    }
  }
</style>
