<script lang="ts">
  import { baseName, encPath, fmtClockShort, readJsonSafe, speakerColor, speakerTint } from '../lib/api'
  import type { SpeakerInfo, SpeakerSample } from '../lib/api'
  import { toast, toastConfirm } from '../lib/toast.svelte.ts'
  import { emit, ui } from '../lib/ui.svelte.ts'
  import { loadPeople, peopleState } from '../lib/people.svelte.ts'
  import { onDestroy } from 'svelte'

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
      const own = spk.samples?.length ? spk.samples : spk.sample ? [spk.sample] : []
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
    input: HTMLInputElement | null,
    embedding: number[] | null | undefined,
    saveBtn: HTMLButtonElement,
  ): Promise<void> {
    const finalName = (input?.value.trim() || '').trim()
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
          embedding: hasEmb ? embedding : null,
          audio_file: ui.currentTranscriptPath,
          old_name: oldName,
        }),
      })
      const d = await readJsonSafe(r)
      if (d.ok) {
        if (hasEmb) {
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

  async function deleteVoiceprint(name: string): Promise<void> {
    if (!name) return
    try {
      const r = await fetch('/api/voiceprints/delete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, audio_file: ui.currentTranscriptPath }),
      })
      const d = await readJsonSafe(r)
      if (d.ok) {
        toast(`Голос «${name}» удалён из базы`)
        await loadPeople()
        emit('transcript-reload')
        emit('voiceprints-changed')
      } else {
        toast('Этот голос уже удалён из базы', 'error')
      }
    } catch (e) {
      toast('Сетевая ошибка: ' + (e as Error).message, 'error')
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
  </div>
{/snippet}

<datalist id="peopleNames">
  {#each peopleState.list as p (p.name)}
    <option value={p.name}>{p.role || ''}</option>
  {/each}
</datalist>

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
        <div
          class="speaker-card {spk.matched ? 'matched' : ''}"
          style={`--spk: ${speakerColor(displayName)}; --spk-bg: ${speakerTint(displayName)}`}
        >
          <div class="speaker-top">
            <span class="speaker-name">{displayName}</span>
            <span class="speaker-status {spk.matched ? 'ok' : ''}">
              {#if spk.matched}
                Узнан: {spk.matched_name || spk.name}
              {:else}
                {(spk.channel === 0 ? 'Микрофон' : spk.channel === 1 ? 'Собеседник' : 'Спикер') + (spk.utterances_count ? ` • реплик: ${spk.utterances_count}` : '')}
              {/if}
            </span>
          </div>

          {#if spk.candidates && spk.candidates.length > 0}
            {@const names = spk.candidates
              .slice(0, 4)
              .map((c) => (Array.isArray(c) ? String(c[0]) : (c as { name: string }).name))}
            <div class="assign-label">Похожие голоса:</div>
            <div class="candidates">
              {#each names as cand (cand)}
                <button
                  class="candidate"
                  title="Подставить это имя"
                  onclick={(e) => {
                    const card = (e.currentTarget as HTMLElement).closest('.speaker-card')
                    const inp = card?.querySelector<HTMLInputElement>('.assign-name-input')
                    if (inp) inp.value = cand
                  }}
                >
                  {cand}
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
            <input
              class="assign-name-input"
              placeholder="Имя участника"
              list="peopleNames"
              value={matchedName}
              autocomplete="off"
              aria-label="Имя человека"
            />
            <button
              class="btn-primary btn-sm"
              title="Сохранить голос за этим человеком и обновить текст"
              onclick={(e) => {
                const card = (e.currentTarget as HTMLElement).closest('.speaker-card')
                void saveVoiceprint(
                  spk.name,
                  card?.querySelector<HTMLInputElement>('.assign-name-input') ?? null,
                  spk.embedding ?? null,
                  e.currentTarget as HTMLButtonElement,
                )
              }}
            >
              Запомнить
            </button>
          </div>
          {#if spk.matched}
            <button
              class="link-danger"
              title="Удалить голос из базы"
              onclick={() =>
                toastConfirm(
                  `Удалить голос «${spk.matched_name || spk.name}» из базы?`,
                  'Удалить',
                  () => void deleteVoiceprint(spk.matched_name || spk.name),
                )}
            >
              Это не тот голос
            </button>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</aside>
