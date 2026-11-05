<script lang="ts">
  import { baseName, encPath, fmtClockShort, readJsonSafe, speakerColor, speakerTint } from '../lib/api'
  import type { SpeakerCandidate, SpeakerInfo, SpeakerSample } from '../lib/api'
  import { toast, toastConfirm } from '../lib/toast.svelte.ts'
  import { emit, ui } from '../lib/ui.svelte.ts'
  import { loadPeople, peopleState } from '../lib/people.svelte.ts'
  import { onDestroy, tick } from 'svelte'
  import { editTranscript, transcript } from '../lib/transcript.svelte.ts'

  interface Props {
    speakers: SpeakerInfo[]
  }
  let { speakers }: Props = $props()

  type MergedSpeaker = Omit<SpeakerInfo, 'samples'> & { key: string; samples: SpeakerSample[] }

  // Один человек — одна карточка: кластеры с одинаковым именем (микрофон/система)
  // объединяем, семплы склеиваем без дублей.
  let mergedSpeakers = $derived.by((): MergedSpeaker[] => {
    const byKey = new Map<string, MergedSpeaker>()
    for (const spk of speakers) {
      const key = String(spk.matched_name || spk.name || 'Спикер').trim().toLowerCase()
      let own = spk.samples?.length ? spk.samples : spk.sample ? [spk.sample] : []
      if (own.length === 0 && transcript.utterances.length > 0) {
        own = transcript.utterances
          .filter((u) => (u.speaker || '') === (spk.name || ''))
          .slice(0, 3)
          .map((u) => {
            const end = u.end ?? u.start
            return {
              start: u.start,
              end,
              dur: Math.max(0, end - u.start),
              text: u.text,
              full_text: u.text,
              channel: u.channel,
            }
          })
      }
      const cur = byKey.get(key)
      if (!cur) {
        byKey.set(key, { ...spk, key, samples: [...own] })
        continue
      }
      // Одинаковые фразы приходят из микрофона и системы с разбегом в доли секунды.
      const seen = new Set(cur.samples.map((s) => (s.text || '').trim().toLowerCase()).filter(Boolean))
      for (const s of own) {
        const text = (s.text || '').trim().toLowerCase()
        if (text && seen.has(text)) continue
        cur.samples.push(s)
        if (text) seen.add(text)
      }
      cur.utterances_count = (cur.utterances_count || 0) + (spk.utterances_count || 0)
      if (spk.matched) {
        cur.matched = true
        cur.matched_name = spk.matched_name || cur.matched_name
        cur.matched_pack_id = spk.matched_pack_id || cur.matched_pack_id
        cur.matched_pack_name = spk.matched_pack_name || cur.matched_pack_name
      }
      if (!cur.embedding && spk.embedding) cur.embedding = spk.embedding
      if ((!cur.candidates || cur.candidates.length === 0) && spk.candidates?.length) {
        cur.candidates = spk.candidates
      }
    }
    return [...byKey.values()].filter(
      (spk) => (spk.utterances_count ?? 0) > 0 || (spk.samples?.length ?? 0) > 0,
    )
  })

  let playingKey = $state('')
  let sampleAudio: HTMLAudioElement | null = null

  function stopSample(): void {
    if (sampleAudio) {
      // Хендлеры глушим до abort: load() без src в части браузеров кидает error.
      sampleAudio.onended = sampleAudio.onerror = null
      sampleAudio.pause()
      // Прерываем недокачанный запрос: без этого он догружается в фоне.
      sampleAudio.removeAttribute('src')
      sampleAudio.load()
    }
    playingKey = ''
  }

  // Фрагмент играет сразу по клику: без верхнего плеера и без полосы прогресса.
  // WAV-записи (наши, самые большие) нарезает ядро: маленький файл играется с нуля,
  // перемотка по гигабайтному WAV вешает WebView. Прочие форматы — медиафрагмент #t=.
  function toggleSample(key: string, start: number, end: number, channel?: number | null): void {
    if (playingKey === key) {
      stopSample()
      return
    }
    stopSample()
    document.querySelectorAll<HTMLMediaElement>('audio, video').forEach((m) => m.pause())

    // Новый элемент на каждый клик: переиспользованный в WebKit иногда остаётся
    // на старом currentTime и играет файл с начала.
    const audio = new Audio()
    sampleAudio = audio
    audio.preload = 'auto'
    playingKey = key
    const done = (): void => {
      if (sampleAudio === audio) playingKey = ''
    }
    audio.onended = done
    audio.onerror = () => {
      done()
      toast('Не удалось воспроизвести фрагмент', 'error')
    }

    const isWav = /\.wav$/i.test(ui.currentTranscriptPath || '')
    if (isWav) {
      const ch = channel === null || channel === undefined ? '' : `&channel=${channel}`
      audio.src =
        `/api/media/snippet?file=${encPath(baseName(ui.currentTranscriptPath))}` +
        `&start=${start}&end=${Math.max(start, end)}${ch}`
      void audio.play().catch(() => {
        if (audio.paused) done()
      })
      return
    }

    let started = false
    const begin = (): void => {
      if (started || sampleAudio !== audio) return
      started = true
      void audio.play().catch(() => {
        if (audio.paused) {
          playingKey = ''
          toast('Не удалось воспроизвести фрагмент', 'error')
        }
      })
    }

    audio.onloadedmetadata = () => {
      const target = Math.max(0, start)
      if (target > 0.05 && Math.abs(audio.currentTime - target) > 0.2) {
        // Ждём seeked, чтобы play() не начал с нуля, пока перемотка ещё идёт.
        audio.currentTime = target
      } else {
        begin()
      }
    }
    audio.onseeked = () => begin()
    audio.ontimeupdate = () => {
      if (audio.currentTime >= end - 0.03) stopSample()
    }
    // Медиафрагмент — основной способ; currentTime и onseeked выше — страховка.
    audio.src = `/audio/${encPath(baseName(ui.currentTranscriptPath))}#t=${start},${Math.max(start, end)}`
    // Если WebKit не прислал loadedmetadata/seeked, не оставляем клик без звука.
    window.setTimeout(() => {
      if (sampleAudio === audio && audio.readyState >= 1) begin()
    }, 1200)
  }

  onDestroy(() => {
    sampleAudio?.pause()
    sampleAudio = null
  })

  async function saveVoiceprint(
    oldName: string,
    nameOrInput: string | HTMLInputElement | null,
    embedding: number[] | null | undefined,
    saveBtn: HTMLButtonElement,
    sample?: { start: number; end: number; channel?: number | null },
    targetPackId?: string,
  ): Promise<void> {
    const finalName = (typeof nameOrInput === 'string' ? nameOrInput : nameOrInput?.value.trim() || '').trim()
    if (!finalName) {
      toast('Выберите человека из списка или впишите имя')
      return
    }
    saveBtn.disabled = true
    try {
      const hasEmb = Boolean(embedding && embedding.length > 0)
      const r = await fetch('/api/voiceprints/save', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: finalName,
          pack_id: targetPackId || undefined,
          embedding: hasEmb ? embedding : null,
          audio_file: ui.currentTranscriptPath,
          old_name: oldName,
          sample_start: sample?.start,
          sample_end: sample ? Math.max(sample.start, sample.end) : undefined,
          sample_channel: sample?.channel,
        }),
      })
      const d = await readJsonSafe(r)
      if (d.ok) {
        if (d.voiceprint_saved || hasEmb) {
          toast(`Голос спикера «${oldName}» привязан к «${finalName}». Текст обновлён.`)
        } else {
          toast(`Спикер «${oldName}» переименован в «${finalName}». Участник сохранён.`)
        }
        await loadPeople()
        emit('transcript-reload')
        emit('voiceprints-changed')
      } else {
        toast('Не удалось сохранить: ' + (d.error || 'неизвестная ошибка'), 'error')
      }
    } catch (e) {
      toast('Сетевая ошибка: ' + (e as Error).message, 'error')
    } finally {
      saveBtn.disabled = false
    }
  }

  async function unmatchSpeaker(speakerName: string): Promise<void> {
    if (!ui.currentTranscriptPath || !speakerName) return
    try {
      const r = await fetch('/api/transcript/edit', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          audio_file: ui.currentTranscriptPath,
          old_speaker: speakerName,
          speaker: speakerName,
          unmatch: true,
        }),
      })
      const d = await readJsonSafe(r)
      if (d.ok) {
        toast(`Опознание спикера «${speakerName}» снято в этой записи`)
        emit('transcript-reload')
      } else {
        toast('Не удалось снять опознание: ' + (d.error || 'ошибка'), 'error')
      }
    } catch (e) {
      toast('Сетевая ошибка: ' + (e as Error).message, 'error')
    }
  }

  async function detachSample(s: { start: number; end: number; channel?: number | null }): Promise<void> {
    if (!ui.currentTranscriptPath) return
    const utts = transcript.utterances
    const idx = utts.findIndex(
      (u) => Math.abs(u.start - s.start) < 0.05 && (s.channel == null || u.channel == null || u.channel === s.channel),
    )
    if (idx < 0) {
      toast('Не удалось найти реплику для открепления', 'error')
      return
    }
    const allSpeakers = new Set(utts.map((u) => u.speaker || ''))
    let newName = 'Новый собеседник'
    if (allSpeakers.has(newName)) {
      let num = 2
      while (allSpeakers.has(`Новый собеседник ${num}`)) num += 1
      newName = `Новый собеседник ${num}`
    }
    try {
      await editTranscript(ui.currentTranscriptPath, { index: idx, speaker: newName })
      toast(`Реплика вынесена в карточку «${newName}»`)
      await tick()
      const cards = Array.from(document.querySelectorAll<HTMLElement>('.speaker-card'))
      const targetCard =
        cards.find((c) => c.querySelector('.speaker-name')?.textContent?.includes(newName)) ||
        document.querySelector<HTMLElement>('.speaker-card:not(.matched)')
      const input = targetCard?.querySelector<HTMLInputElement>('.assign-name-input')
      input?.focus()
      targetCard?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
    } catch (e) {
      toast('Не удалось вынести реплику: ' + (e as Error).message, 'error')
    }
  }

  let inputValues = $state<Record<string, string>>({})
  let cardPack = $state<Record<string, string>>({})

  function getCardValue(key: string, fallback: string): string {
    return inputValues[key] ?? fallback
  }

  function setCardValue(key: string, val: string): void {
    inputValues[key] = val
  }

  function getCardPack(key: string, currentVal?: string): string {
    if (cardPack[key]) return cardPack[key]
    const val = (currentVal || '').trim().toLowerCase()
    if (val) {
      const match = peopleState.list.find((p) => p.name.toLowerCase() === val)
      if (match?.pack_id) return match.pack_id
    }
    return peopleState.packs[0]?.id || 'default'
  }

  interface DropdownState {
    spkKey: string
    x: number
    y: number
    w: number
  }
  let activeDropdown = $state<DropdownState | null>(null)
  let dropdownEl = $state<HTMLElement | null>(null)

  async function openDropdownFor(key: string, anchorEl: HTMLElement): Promise<void> {
    const card = anchorEl.closest('.speaker-card') as HTMLElement | null
    const cardRect = card?.getBoundingClientRect()
    const rect = anchorEl.getBoundingClientRect()
    const estW = Math.max(270, cardRect ? cardRect.right - rect.left : rect.width)
    activeDropdown = {
      spkKey: key,
      x: Math.max(8, Math.min(rect.left, window.innerWidth - estW - 8)),
      y: rect.bottom + 4,
      w: estW,
    }
    await tick()
    if (dropdownEl && activeDropdown) {
      const r = dropdownEl.getBoundingClientRect()
      let { x, y } = activeDropdown
      if (r.right > window.innerWidth - 8) x = Math.max(8, x - (r.right - (window.innerWidth - 8)))
      if (r.bottom > window.innerHeight - 8) {
        const above = rect.top - r.height - 4
        y = above >= 8 ? above : Math.max(8, window.innerHeight - r.height - 8)
      }
      activeDropdown = { ...activeDropdown, x, y }
    }
  }

  function closeDropdown(): void {
    activeDropdown = null
  }

  let currentInputVal = $derived.by(() => {
    if (!activeDropdown) return ''
    const k = activeDropdown.spkKey
    return (inputValues[k] ?? mergedSpeakers.find((s) => s.key === k)?.matched_name ?? '').trim()
  })

  let filteredPeople = $derived.by(() => {
    if (!activeDropdown) return []
    const q = currentInputVal.toLowerCase()
    if (!q) return peopleState.list
    return peopleState.list.filter(
      (p) => p.name.toLowerCase().includes(q) || p.role?.toLowerCase().includes(q),
    )
  })

  let otherSpeakersInMeeting = $derived.by(() => {
    if (!activeDropdown) return []
    const activeKey = activeDropdown.spkKey
    const q = currentInputVal.toLowerCase()
    const peopleNames = new Set(peopleState.list.map((p) => p.name.toLowerCase()))
    return mergedSpeakers
      .filter((s) => s.key !== activeKey)
      .filter((s) => {
        const name = (s.matched_name || s.name || 'Спикер').trim()
        if (peopleNames.has(name.toLowerCase())) return false
        if (!q) return true
        return name.toLowerCase().includes(q)
      })
  })

  let showCreateOption = $derived.by(() => {
    if (!activeDropdown || !currentInputVal) return false
    const lower = currentInputVal.toLowerCase()
    const inPeople = peopleState.list.some((p) => p.name.toLowerCase() === lower)
    const inOther = otherSpeakersInMeeting.some((s) => (s.matched_name || s.name || '').toLowerCase() === lower)
    return !inPeople && !inOther
  })

  function selectPerson(name: string, packId?: string): void {
    if (!activeDropdown) return
    const k = activeDropdown.spkKey
    setCardValue(k, name)
    if (packId) {
      cardPack[k] = packId
    } else {
      const match = peopleState.list.find((p) => p.name.toLowerCase() === name.trim().toLowerCase())
      if (match?.pack_id) cardPack[k] = match.pack_id
    }
    closeDropdown()
  }

  function onInputKey(e: KeyboardEvent, btn: HTMLButtonElement | null): void {
    if (e.key === 'Escape') {
      e.preventDefault()
      closeDropdown()
    } else if (e.key === 'Enter') {
      e.preventDefault()
      closeDropdown()
      btn?.click()
    }
  }
</script>

{#snippet sampleRow(s: { start: number; end: number; text?: string; full_text?: string; channel?: number | null }, key: string)}
  <div class="sample-row">
    <button
      class="sample-play {playingKey === key ? 'playing' : ''}"
      title={playingKey === key ? 'Остановить фрагмент' : `Прослушать фрагмент: ${(s.text || '').slice(0, 80)}`}
      aria-label={playingKey === key ? 'Остановить фрагмент' : `Прослушать фрагмент: ${(s.text || '').slice(0, 60)}`}
      aria-pressed={playingKey === key}
      onclick={() => toggleSample(key, s.start, s.end ?? s.start, s.channel)}
    >
      {#if playingKey === key}
        <svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true">
          <rect x="2.4" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
          <rect x="6.8" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
        </svg>
      {:else}
        <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M3.6 2.1v7.8L10.2 6z" fill="currentColor" />
        </svg>
      {/if}
    </button>
    <span class="sample-text" title={s.full_text || s.text || ''}>
      {fmtClockShort(s.start)} · «{s.text || ''}»
    </span>
    <button
      type="button"
      class="sample-detach-btn"
      title="Вынести эту реплику в отдельную карточку"
      onclick={() => void detachSample(s)}
    >
      Открепить
    </button>
  </div>
{/snippet}

<aside class="transcript-speakers">
  <div class="pane-title">Спикеры</div>
  <div class="speakers-deck">
    {#if mergedSpeakers.length === 0}
      <div class="empty-note">Спикеры появятся после расшифровки</div>
    {:else}
      {#each mergedSpeakers as spk, si (spk.key)}
        {@const samples = [...spk.samples].sort((a, b) => a.start - b.start)}
        {@const matchedName = spk.matched_name || ''}
        {@const displayName = spk.matched_name || spk.name || 'Спикер'}
        {@const cardVal = getCardValue(spk.key, matchedName)}
        {@const meta = [
          spk.utterances_count ? `реплик: ${spk.utterances_count}` : '',
          spk.matched_pack_name ? spk.matched_pack_name : '',
          typeof spk.silhouette === 'number' ? `силуэт: ${spk.silhouette.toFixed(2)}` : '',
        ].filter(Boolean).join(' • ')}
        <div
          class="speaker-card {spk.matched ? 'matched' : ''}"
          style={`--spk: ${speakerColor(displayName)}; --spk-bg: ${speakerTint(displayName)}`}
        >
          <div class="speaker-header">
            <div class="speaker-top">
              <span class="speaker-name">{displayName}</span>
              {#if spk.matched}
                <span class="speaker-status ok">
                  узнан{typeof spk.similarity === 'number' ? ` · ${spk.similarity.toFixed(2)}` : ''}
                  <button
                    type="button"
                    class="btn-unmatch-speaker"
                    title="Снять опознание в этой записи"
                    onclick={() => void unmatchSpeaker(spk.name || displayName)}
                  >✕</button>
                </span>
              {/if}
            </div>
            {#if meta}
              <div class="speaker-meta">{meta}</div>
            {/if}
          </div>

          {#if spk.candidates && spk.candidates.length > 0}
            {@const names = spk.candidates
              .slice(0, 4)
              .map((c) => {
                if (Array.isArray(c)) return `${String(c[0])} · ${Number(c[1]).toFixed(2)}`
                const cand = c as SpeakerCandidate
                const pSuffix = cand.pack_name ? ` (${cand.pack_name})` : ''
                const sim = cand.similarity
                return typeof sim === 'number' ? `${cand.name}${pSuffix} · ${sim.toFixed(2)}` : `${cand.name}${pSuffix}`
              })}
            {@const fullNames = spk.candidates
              .slice(0, 4)
              .map((c) => (Array.isArray(c) ? String(c[0]) : (c as SpeakerCandidate).name))}
            {@const candPacks = spk.candidates
              .slice(0, 4)
              .map((c) => (Array.isArray(c) ? undefined : (c as SpeakerCandidate).pack_id))}
            <div class="assign-label">Похожие голоса:</div>
            <div class="candidates">
              {#each names as label, i (fullNames[i])}
                <button
                  class="candidate"
                  title="Подставить это имя"
                  onclick={() => {
                    setCardValue(spk.key, fullNames[i])
                    if (candPacks[i]) cardPack[spk.key] = candPacks[i]!
                    closeDropdown()
                  }}
                >
                  {label}
                </button>
              {/each}
            </div>
          {/if}

          {#if samples.length > 0}
            <div class="speaker-samples">
              {#each samples.slice(0, 3) as s, i (`${s.start}-${i}`)}
                {@render sampleRow(s, `${si}-${i}-${s.start}`)}
              {/each}
            </div>
          {/if}

          <div class="assign-row">
            <div class="assign-input-wrap">
              <input
                class="assign-name-input"
                placeholder="Имя участника"
                value={cardVal}
                autocomplete="off"
                aria-label="Имя человека"
                oninput={(e) => {
                  setCardValue(spk.key, e.currentTarget.value)
                  const wrap = e.currentTarget.closest('.assign-input-wrap') as HTMLElement | null
                  if (wrap) void openDropdownFor(spk.key, wrap)
                }}
                onfocus={(e) => {
                  const wrap = e.currentTarget.closest('.assign-input-wrap') as HTMLElement | null
                  if (wrap) void openDropdownFor(spk.key, wrap)
                }}
                onclick={(e) => {
                  const wrap = e.currentTarget.closest('.assign-input-wrap') as HTMLElement | null
                  if (wrap) void openDropdownFor(spk.key, wrap)
                }}
                onkeydown={(e) => {
                  const card = (e.currentTarget as HTMLElement).closest('.speaker-card')
                  const btn = card?.querySelector<HTMLButtonElement>('.btn-save-voiceprint') ?? null
                  onInputKey(e, btn)
                }}
              />
              <button
                type="button"
                class="assign-chevron-btn"
                title="Показать список участников"
                aria-label="Показать список участников"
                tabindex="-1"
                onclick={(e) => {
                  e.stopPropagation()
                  const wrap = (e.currentTarget as HTMLElement).closest('.assign-input-wrap') as HTMLElement | null
                  if (activeDropdown?.spkKey === spk.key) {
                    closeDropdown()
                  } else if (wrap) {
                    void openDropdownFor(spk.key, wrap)
                  }
                }}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <polyline points="6 9 12 15 18 9"></polyline>
                </svg>
              </button>
            </div>
            <button
              class="btn-primary btn-sm btn-save-voiceprint"
              title="Сохранить голос за этим человеком и обновить текст"
              onclick={(e) => {
                void saveVoiceprint(
                  spk.name,
                  cardVal,
                  spk.embedding ?? null,
                  e.currentTarget as HTMLButtonElement,
                  samples[0]
                    ? { start: samples[0].start, end: samples[0].end, channel: samples[0].channel }
                    : undefined,
                  getCardPack(spk.key, cardVal),
                )
              }}
            >
              Запомнить
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  {#if activeDropdown}
    <button
      type="button"
      class="speaker-popover-backdrop"
      aria-label="Закрыть список"
      onclick={closeDropdown}
    ></button>
    <div
      class="speaker-select-popover"
      role="listbox"
      aria-label="Выбор участника"
      bind:this={dropdownEl}
      style={`left: ${activeDropdown.x}px; top: ${activeDropdown.y}px; width: ${activeDropdown.w}px`}
    >
      {#if showCreateOption}
        <button
          type="button"
          class="speaker-select-item is-create"
          title={`Запомнить «${currentInputVal}»`}
          onclick={() => selectPerson(currentInputVal)}
        >
          <span class="speaker-select-item-left">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="12" y1="5" x2="12" y2="19"></line>
              <line x1="5" y1="12" x2="19" y2="12"></line>
            </svg>
            <span class="speaker-select-item-name">Запомнить «{currentInputVal}»</span>
          </span>
        </button>
      {/if}

      {#if filteredPeople.length > 0}
        <div class="speaker-select-section-title">Участники из базы</div>
        {#each filteredPeople as p (p.name)}
          <button
            type="button"
            class="speaker-select-item"
            title={p.role ? `${p.name} - ${p.role}` : p.name}
            onclick={() => selectPerson(p.name, p.pack_id)}
          >
            <span class="speaker-select-item-left">
              <span class="speaker-select-item-dot" style={`background: ${speakerColor(p.name)}`}></span>
              <span class="speaker-select-item-name">{p.name}</span>
            </span>
            <span class="speaker-select-item-sub">
              {p.role ? `${p.role} · ` : ''}{p.pack_name || (p.is_external ? 'Гость' : 'Основной профиль')}
            </span>
          </button>
        {/each}
      {/if}

      {#if otherSpeakersInMeeting.length > 0}
        <div class="speaker-select-section-title">Спикеры встречи</div>
        {#each otherSpeakersInMeeting as s (s.key)}
          {@const sName = s.matched_name || s.name || 'Спикер'}
          <button
            type="button"
            class="speaker-select-item"
            title={s.utterances_count ? `${sName} - ${s.utterances_count} реплик` : sName}
            onclick={() => selectPerson(sName)}
          >
            <span class="speaker-select-item-left">
              <span class="speaker-select-item-dot" style={`background: ${speakerColor(sName)}`}></span>
              <span class="speaker-select-item-name">{sName}</span>
            </span>
            {#if s.utterances_count}
              <span class="speaker-select-item-sub">{s.utterances_count} реплик</span>
            {/if}
          </button>
        {/each}
      {/if}

      {#if !showCreateOption && filteredPeople.length === 0 && otherSpeakersInMeeting.length === 0}
        <div class="speaker-select-empty">Ничего не найдено</div>
      {/if}
    </div>
  {/if}
</aside>
