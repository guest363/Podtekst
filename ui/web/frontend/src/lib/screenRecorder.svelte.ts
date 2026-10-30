// Состояние переключателя записи экрана.
// Захват экрана и склейка выполняются нативно на стороне Rust-ядра.
import { toast } from './toast.svelte.ts'

export interface ScreenRecorderState {
  enabled: boolean
  active: boolean
  error: string | null
  supported: boolean
}

export const screenRecorder = $state<ScreenRecorderState>({
  enabled: false,
  active: false,
  error: null,
  supported: true,
})

export function isScreenCaptureSupported(): boolean {
  if (typeof window === 'undefined') return false
  if (typeof navigator !== 'undefined' && /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)) {
    return false
  }
  return screenRecorder.supported
}

export function updateScreenSupport(supported: boolean): void {
  screenRecorder.supported = supported
}

export function toggleScreenCapture(): boolean {
  screenRecorder.enabled = !screenRecorder.enabled
  if (screenRecorder.enabled) {
    toast('Запись экрана включена')
  } else {
    toast('Запись экрана выключена')
  }
  return screenRecorder.enabled
}

export function cancelScreenSelection(): void {
  screenRecorder.enabled = false
  screenRecorder.active = false
  toast('Запись экрана выключена')
}
