<script lang="ts">
  import { onMount } from 'svelte'
  import { formatRangeLabel } from '../lib/api'

  interface Props {
    start: Date | null
    end: Date | null
    onSelect: (start: Date, end: Date, label: string) => void
    onClear: () => void
    onClose: () => void
    hasRecordings?: (d: Date) => boolean
  }

  const { start, end, onSelect, onClear, onClose, hasRecordings }: Props = $props()

  const MONTHS = [
    'Январь', 'Февраль', 'Март', 'Апрель', 'Май', 'Июнь',
    'Июль', 'Август', 'Сентябрь', 'Октябрь', 'Ноябрь', 'Декабрь',
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

  // 3 пресета строго по требованию: Сегодня, 7 дней, Месяц
  function applyPreset(preset: 'today' | 'week' | 'month'): void {
    if (preset === 'today') {
      onSelect(today, today, 'Сегодня')
    } else if (preset === 'week') {
      const s = new Date(today.getTime() - 6 * 86400000)
      onSelect(s, today, '7 дней')
    } else {
      const s = new Date(today.getFullYear(), today.getMonth(), 1)
      const e = new Date(today.getFullYear(), today.getMonth() + 1, 0)
      onSelect(s, e, 'Месяц')
    }
    onClose()
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
    return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate()
  }

  function isToday(d: Date): boolean {
    return isSameDay(d, today)
  }

  const activePreset = $derived.by(() => {
    if (!selStart || !selEnd) return null
    if (isSameDay(selStart, today) && isSameDay(selEnd, today)) return 'today'
    const weekStart = new Date(today.getTime() - 6 * 86400000)
    if (isSameDay(selStart, weekStart) && isSameDay(selEnd, today)) return 'week'
    const monthStart = new Date(today.getFullYear(), today.getMonth(), 1)
    const monthEnd = new Date(today.getFullYear(), today.getMonth() + 1, 0)
    if (isSameDay(selStart, monthStart) && isSameDay(selEnd, monthEnd)) return 'month'
    return null
  })

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
  <!-- 3 обязательных пресета -->
  <div class="date-presets-row">
    <button type="button" class="preset-btn {activePreset === 'today' ? 'active' : ''}" onclick={() => applyPreset('today')}>Сегодня</button>
    <button type="button" class="preset-btn {activePreset === 'week' ? 'active' : ''}" onclick={() => applyPreset('week')}>7 дней</button>
    <button type="button" class="preset-btn {activePreset === 'month' ? 'active' : ''}" onclick={() => applyPreset('month')}>Месяц</button>
  </div>

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
      <span class="cal-footer-selected">
        {formatRangeLabel(selStart, selEnd ?? selStart)}
      </span>
      {#if !selEnd}
        <button type="button" class="cal-footer-btn apply" onclick={confirmSingle}>
          Выбрать день
        </button>
      {/if}
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

  .date-presets-row {
    display: flex;
    background: var(--bg-inset);
    border-radius: var(--radius-inner);
    padding: 2px;
    gap: 2px;
    margin-bottom: 10px;
    border: 1px solid var(--border-soft);
  }
  .preset-btn {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--text-muted);
    border-radius: var(--radius-sm);
    padding: 5px 6px;
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
    text-align: center;
    transition: background 120ms ease, color 120ms ease, box-shadow 120ms ease;
  }
  .preset-btn:hover {
    color: var(--text-primary);
  }
  .preset-btn.active {
    background: #ffffff;
    color: var(--accent-blue);
    box-shadow: 0 1px 3px rgba(15, 30, 51, 0.08);
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
    gap: 2px 0;
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
    border-radius: 0;
    padding: 0;
    transition: background 100ms ease, color 100ms ease;
  }
  .calendar-day-btn:hover {
    background: var(--bg-inset);
    border-radius: var(--radius-sm);
  }

  /* Сегодня без выбора в диапазон */
  .calendar-day-btn.is-today:not(.range-start):not(.range-end):not(.range-mid) {
    box-shadow: inset 0 0 0 1.5px var(--accent-blue);
    border-radius: var(--radius-sm);
  }
  .calendar-day-btn.is-today:not(.range-start):not(.range-end) .day-num {
    font-weight: 800;
    color: var(--accent-blue);
  }

  /* Выбранные края диапазона (или одиночный день) */
  .calendar-day-btn.range-start,
  .calendar-day-btn.range-end {
    background: var(--accent-blue) !important;
    color: #ffffff !important;
  }
  .calendar-day-btn.range-start {
    border-top-left-radius: var(--radius-sm);
    border-bottom-left-radius: var(--radius-sm);
  }
  .calendar-day-btn.range-end {
    border-top-right-radius: var(--radius-sm);
    border-bottom-right-radius: var(--radius-sm);
  }
  .calendar-day-btn.range-start.range-end {
    border-radius: var(--radius-sm);
  }

  /* КРИТИЧЕСКИЙ ФИКС: текст цифры дня в выбранном диапазоне ОБЯЗАН быть белым */
  .calendar-day-btn.range-start .day-num,
  .calendar-day-btn.range-end .day-num {
    color: #ffffff !important;
    font-weight: 800;
  }

  /* Середина диапазона */
  .calendar-day-btn.range-mid {
    background: #edf3fd !important;
    color: var(--accent-blue) !important;
  }
  .calendar-day-btn.range-mid .day-num {
    color: var(--accent-blue) !important;
    font-weight: 700;
  }

  /* Точка наличия записей */
  .day-dot {
    position: absolute;
    bottom: 2px;
    width: 3.5px;
    height: 3.5px;
    border-radius: 999px;
    background: var(--text-faint);
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
  .calendar-day-btn.is-today .day-dot {
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
  .cal-footer-selected {
    font-size: 11px;
    font-weight: 700;
    color: var(--text-primary);
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
