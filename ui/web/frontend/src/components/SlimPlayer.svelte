<script lang="ts">
  import { fmtClock } from '../lib/api'

  interface Props {
    src: string
    isVideo: boolean
    fileName: string
    mediaId?: string
    durationSec?: number
  }
  let { src, isVideo, fileName, mediaId = '', durationSec = 0 }: Props = $props()

  let media: HTMLMediaElement | null = $state(null)
  let playing = $state(false)
  let cur = $state(0)
  let mediaDur = $state(0)
  // Длительность из списка записей видна сразу, до загрузки метаданных плеером.
  let dur = $derived(mediaDur || durationSec || 0)

  // src может меняться (сниппеты): подтягиваем в элемент.
  $effect(() => {
    if (media && media.getAttribute('src') !== src) {
      media.src = src
      media.load()
      cur = 0
      mediaDur = 0
    }
  })

  function toggle(): void {
    if (!media) return
    const all = Array.from(document.querySelectorAll('audio, video')) as HTMLMediaElement[]
    for (const m of all) {
      if (m !== media && !m.paused) m.pause()
    }
    if (media.paused) media.play().catch(() => {})
    else media.pause()
  }

  function seek(e: MouseEvent): void {
    const track = e.currentTarget as HTMLElement
    const rect = track.getBoundingClientRect()
    const ratio = (e.clientX - rect.left) / Math.max(1, rect.width)
    if (media && isFinite(media.duration) && media.duration > 0) {
      media.currentTime = Math.max(0, Math.min(media.duration, ratio * media.duration))
    }
  }
</script>

<div class="slim-player">
  {#if isVideo}
    <!-- svelte-ignore a11y_media_has_caption: субтитров нет, это запись встречи, а не медиатека -->
    <video bind:this={media} {src} id={mediaId || undefined} preload="auto" hidden
      aria-label={`Слушать ${fileName}`}
      onplay={() => (playing = true)} onpause={() => (playing = false)}
      ontimeupdate={() => media && (cur = media.currentTime)}
      onseeked={() => media && (cur = media.currentTime)}
      ondurationchange={() => media && isFinite(media.duration) && (mediaDur = media.duration || 0)}
      onloadedmetadata={() => media && isFinite(media.duration) && (mediaDur = media.duration || 0)}></video>
  {:else}
    <audio bind:this={media} {src} id={mediaId || undefined} preload="auto" hidden
      aria-label={`Слушать ${fileName}`}
      onplay={() => (playing = true)} onpause={() => (playing = false)}
      ontimeupdate={() => media && (cur = media.currentTime)}
      onseeked={() => media && (cur = media.currentTime)}
      ondurationchange={() => media && isFinite(media.duration) && (mediaDur = media.duration || 0)}
      onloadedmetadata={() => media && isFinite(media.duration) && (mediaDur = media.duration || 0)}></audio>
  {/if}
  <button class="slim-play" onclick={toggle} aria-label={playing ? `Остановить ${fileName}` : `Слушать ${fileName}`}>
    {#if playing}
      <svg width="11" height="11" viewBox="0 0 12 12" aria-hidden="true">
        <rect x="2.4" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
        <rect x="6.8" y="2" width="2.8" height="8" rx="1" fill="currentColor" />
      </svg>
    {:else}
      <svg class="play-glyph" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <path d="M3.6 2.1v7.8L10.2 6z" fill="currentColor" />
      </svg>
    {/if}
  </button>
  <div class="slim-track" onclick={seek} role="presentation">
    <div class="slim-fill" style={`width: ${dur ? Math.min(100, (cur / dur) * 100) : 0}%`}></div>
  </div>
  <span class="slim-time">{fmtClock(cur)} / {fmtClock(dur)}</span>
</div>
