// Тосты вместо alert/confirm/prompt. sticky=true — висит, пока пользователь не закроет.
export interface ToastAction {
  label: string
  run: () => void
}

export interface Toast {
  id: number
  msg: string
  kind: '' | 'error'
  sticky: boolean
  actions: ToastAction[]
}

let nextId = 1
export const toasts: Toast[] = $state([])

export function toast(msg: string, kind: '' | 'error' = '', sticky = false): number {
  const id = nextId++
  toasts.push({ id, msg, kind, sticky, actions: [] })
  if (!sticky) {
    setTimeout(() => dismissToast(id), 4200)
  }
  return id
}

export function toastConfirm(msg: string, okLabel: string, onOk: () => void): void {
  const id = nextId++
  toasts.push({
    id,
    msg,
    kind: '',
    sticky: true,
    actions: [
      { label: 'Отмена', run: () => dismissToast(id) },
      {
        label: okLabel,
        run: () => {
          dismissToast(id)
          onOk()
        },
      },
    ],
  })
}

export function dismissToast(id: number): void {
  const i = toasts.findIndex((t) => t.id === id)
  if (i >= 0) toasts.splice(i, 1)
}

// Копирование с надёжным запасным путём для мобильных браузеров, незащищённых HTTP-контекстов и старых WebView.
export async function copyText(s: string): Promise<boolean> {
  if (window.isSecureContext && typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(s)
      return true
    } catch {
      // Пользователь мог отклонить разрешение — пробуем запасной путь ниже
    }
  }

  try {
    const ta = document.createElement('textarea')
    ta.value = s
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.top = '0'
    ta.style.left = '-9999px'
    ta.style.fontSize = '12pt'
    document.body.appendChild(ta)

    const sel = document.getSelection()
    const prevRange = sel && sel.rangeCount > 0 ? sel.getRangeAt(0) : null

    ta.focus()
    ta.select()
    ta.setSelectionRange(0, s.length)
    const ok = document.execCommand('copy')
    document.body.removeChild(ta)

    if (prevRange && sel) {
      sel.removeAllRanges()
      sel.addRange(prevRange)
    }
    return ok
  } catch {
    return false
  }
}
