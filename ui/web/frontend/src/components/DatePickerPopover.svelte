<script lang="ts">
import { onMount } from 'svelte'
import { formatRangeLabel } from '../lib/api'

type Props = {
  start: Date | null
  end: Date | null
  onSelect: (start: Date, end: Date, label: string) => void
  onClear: () => void
  onClose: () => void
  hasRecordings?: (d: Date) => boolean
}

const { start, end, onSelect, onClear, onClose, hasRecordings }: Props = $props()

const MONTHS = [
  'Январь',
  'Февраль',
  'Март',
  'Апрель',
  'Май',
  'Июнь',
  'Июль',
  'Август',
  'Сентябрь',
  'Октябрь',
  'Ноябрь',
  'Декабрь',
]

const now = new Date()
const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())

let viewYear = $state(today.getFullYear())
let viewMonth = $state(today.getMonth())

let selStart = $state<Date | null>(null)
let selEnd = $state<Date | null>(null)
let hoverDate = $state<Date | null>(null)

$effect(() => {
  selStart = start
  selEnd = end
  if (start) {
    viewYear = start.getFullYear()
    viewMonth = start.getMonth()
  }
})

let rootEl: HTMLDivElement | null = $state(null)

function prevMonth(): void {
  if (viewMonth === 0) {
    viewMonth = 11
    viewYear -= 1
  } else {
    viewMonth -= 1
  }
}

function nextMonth(): void {
  if (viewMonth === 11) {
    viewMonth = 0
    viewYear += 1
  } else {
    viewMonth += 1
  }
}

function onDayClick(d: Date): void {
  if (!selStart || (selStart && selEnd)) {
    selStart = d
    selEnd = null
  } else {
    let s = selStart
    let e = d
    if (e.getTime() < s.getTime()) {
      const tmp = s
      s = e
      e = tmp
    }
    selStart = s
    selEnd = e
    onSelect(s, e, formatRangeLabel(s, e))
    onClose()
  }
}

function confirmSingle(): void {
  if (selStart) {
    const e = selEnd ?? selStart
    onSelect(selStart, e, formatRangeLabel(selStart, e))
    onClose()
  }
}

const daysGrid = $derived.by(() => {
  // getDay(): 0 (Вс) -> смещение: Пн=0, Вт=1... Вс=6
  const firstDayIdx = (new Date(viewYear, viewMonth, 1).getDay() + 6) % 7
  const totalDays = new Date(viewYear, viewMonth + 1, 0).getDate()
  const cells: (Date | null)[] = []
  for (let i = 0; i < firstDayIdx; i++) cells.push(null)
  for (let day = 1; day <= totalDays; day++) {
    cells.push(new Date(viewYear, viewMonth, day))
  }
  return cells
})

function isSameDay(a: Date | null, b: Date | null): boolean {
  if (!a || !b) return false
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  )
}

function isToday(d: Date): boolean {
  return isSameDay(d, today)
}

function isInRange(d: Date): boolean {
  const s = selStart
  const e = selEnd ?? (selStart ? hoverDate : null)
  if (!s || !e) return false
  const t = d.getTime()
  const t1 = Math.min(s.getTime(), e.getTime())
  const t2 = Math.max(s.getTime(), e.getTime())
  return t > t1 && t < t2
}

onMount(() => {
  function onPointerDown(e: PointerEvent): void {
    if (rootEl && !rootEl.contains(e.target as Node)) {
      // Если был выбран только один день, фиксируем его
      if (selStart && !selEnd) {
        onSelect(selStart, selStart, formatRangeLabel(selStart, selStart))
      }
      onClose()
    }
  }
  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.stopPropagation()
      onClose()
    }
  }
  window.addEventListener('pointerdown', onPointerDown, true)
  window.addEventListener('keydown', onKey)
  return () => {
    window.removeEventListener('pointerdown', onPointerDown, true)
    window.removeEventListener('keydown', onKey)
  }
})
</script>

<div class="date-picker-popover" bind:this={rootEl} role="dialog" aria-label="Календарь периода">
  <!-- Навигация по месяцу -->
  <div class="calendar-nav">
    <button type="button" class="nav-arrow-btn" aria-label="Предыдущий месяц" onclick={prevMonth}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M15 18l-6-6 6-6" />
      </svg>
    </button>
    <span class="nav-month-title">{MONTHS[viewMonth]} {viewYear}</span>
    <button type="button" class="nav-arrow-btn" aria-label="Следующий месяц" onclick={nextMonth}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M9 18l6-6-6-6" />
      </svg>
    </button>
  </div>

  <!-- Дни недели -->
  <div class="calendar-weekdays" aria-hidden="true">
    <span>Пн</span><span>Вт</span><span>Ср</span><span>Чт</span><span>Пт</span><span>Сб</span><span>Вс</span>
  </div>

  <!-- Сетка дней -->
  <div class="calendar-grid">
    {#each daysGrid as cell}
      {#if cell}
        {@const isStart = isSameDay(cell, selStart)}
        {@const isEnd = isSameDay(cell, selEnd)}
        {@const inMid = isInRange(cell)}
        {@const isToday = isSameDay(cell, today)}
        {@const hasRec = hasRecordings ? hasRecordings(cell) : false}
        <button
          type="button"
          class="calendar-day-btn {isStart ? 'range-start' : ''} {isEnd ? 'range-end' : ''} {inMid ? 'range-mid' : ''} {isToday ? 'is-today' : ''}"
          onclick={() => onDayClick(cell)}
          onpointerenter={() => (hoverDate = cell)}
        >
          <span class="day-num">{cell.getDate()}</span>
          {#if hasRec}
            <span class="day-dot" aria-hidden="true"></span>
          {/if}
        </button>
      {:else}
        <span class="calendar-empty-cell"></span>
      {/if}
    {/each}
  </div>

  <!-- Подвал поповера -->
  <div class="calendar-footer">
    <button type="button" class="cal-footer-btn clear" onclick={() => { onClear(); onClose(); }}>
      Сбросить
    </button>
    {#if selStart}
      <div class="cal-footer-right">
        <span class="cal-footer-selected">
          {formatRangeLabel(selStart, selEnd ?? selStart)}
        </span>
        {#if !selEnd}
          <button type="button" class="cal-footer-btn apply" onclick={confirmSingle}>
            Выбрать
          </button>
        {/if}
      </div>
    {:else}
      <span class="cal-footer-hint">Выберите дату</span>
    {/if}
  </div>
</div>

<style>
  .date-picker-popover {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 50;
    width: 260px;
    background: #ffffff;
    border: 1px solid var(--border-card);
    border-radius: var(--radius-inner);
    box-shadow: 0 10px 30px -6px rgba(15, 30, 51, 0.16);
    padding: 10px;
    font-size: 12px;
    user-select: none;
    animation: popoverFade 140ms ease;
  }
  @keyframes popoverFade {
    from { opacity: 0; transform: translateY(-4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .calendar-nav {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
    padding: 0 2px;
  }
  .nav-month-title {
    font-weight: 700;
    font-size: 12px;
    color: var(--text-primary);
  }
  .nav-arrow-btn {
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 4px;
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .nav-arrow-btn:hover {
    background: var(--bg-inset);
    color: var(--text-primary);
  }

  .calendar-weekdays {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    text-align: center;
    font-size: 10px;
    font-weight: 600;
    color: var(--text-faint);
    margin-bottom: 4px;
  }

  .calendar-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px 3px;
  }
  .calendar-empty-cell {
    height: 30px;
  }
  .calendar-day-btn {
    border: none;
    background: transparent;
    color: var(--text-primary);
    height: 30px;
    font-size: 12px;
    cursor: pointer;
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    padding: 0;
    transition: background 100ms ease, color 100ms ease;
  }
  .calendar-day-btn:hover {
    background: var(--bg-inset);
  }

  .day-num {
    line-height: 1;
    margin-top: -2px;
  }

  /* Сегодня без грубых рамок: деликатная мягкая подсветка без рамок и обводок */
  .calendar-day-btn.is-today:not(.range-start):not(.range-end):not(.range-mid) {
    background: #edf3fd;
  }
  .calendar-day-btn.is-today:not(.range-start):not(.range-end):not(.range-mid) .day-num {
    font-weight: 800;
    color: var(--accent-blue);
  }

  /* Выбранные края диапазона (или одиночный день): синий фон, чисто белый текст */
  .calendar-day-btn.range-start,
  .calendar-day-btn.range-end {
    background: var(--accent-blue) !important;
    color: #ffffff !important;
    border-radius: 6px;
  }
  .calendar-day-btn.range-start .day-num,
  .calendar-day-btn.range-end .day-num {
    color: #ffffff !important;
    font-weight: 800;
  }

  /* Середина диапазона */
  .calendar-day-btn.range-mid {
    background: #edf3fd !important;
    border-radius: 4px;
  }
  .calendar-day-btn.range-mid .day-num {
    color: var(--accent-blue) !important;
    font-weight: 700;
  }

  /* Точка наличия записей: мягкая точка под цифрой */
  .day-dot {
    position: absolute;
    bottom: 3px;
    width: 3px;
    height: 3px;
    border-radius: 999px;
    background: var(--text-faint);
    pointer-events: none;
    opacity: 0.9;
  }
  .calendar-day-btn.range-start .day-dot,
  .calendar-day-btn.range-end .day-dot {
    background: #ffffff !important;
    opacity: 1;
  }
  .calendar-day-btn.range-mid .day-dot {
    background: var(--accent-blue) !important;
  }
  .calendar-day-btn.is-today:not(.range-start):not(.range-end) .day-dot {
    background: var(--accent-blue);
  }

  .calendar-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 8px;
    padding-top: 6px;
    border-top: 1px solid var(--border-soft);
  }
  .cal-footer-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .cal-footer-selected {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-primary);
  }
  .cal-footer-hint {
    font-size: 11px;
    color: var(--text-faint);
  }
  .cal-footer-btn {
    border: none;
    background: transparent;
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
    padding: 2px 4px;
    border-radius: var(--radius-sm);
  }
  .cal-footer-btn.clear {
    color: var(--text-muted);
  }
  .cal-footer-btn.clear:hover {
    color: var(--text-primary);
  }
  .cal-footer-btn.apply {
    color: var(--accent-blue);
  }
  .cal-footer-btn.apply:hover {
    text-decoration: underline;
  }
</style>
