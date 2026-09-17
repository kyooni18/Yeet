import { nextTick, onBeforeUnmount, onMounted, ref, watch, type WatchSource } from 'vue'

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

export function useModalFocus(isOpen: WatchSource<boolean>, close: () => void) {
  const modalRoot = ref<HTMLElement | null>(null)
  let previouslyFocused: HTMLElement | null = null
  let lastPointerTarget: HTMLElement | null = null
  let restoreFrame = 0
  let modalActive = false

  function focusableElements(): HTMLElement[] {
    const root = modalRoot.value
    if (!root) return []
    return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)]
      .filter((element) => element.getClientRects().length > 0 && element.getAttribute('aria-hidden') !== 'true')
  }

  function rememberPointerTarget(event: PointerEvent) {
    const target = event.target
    if (!(target instanceof HTMLElement)) return
    const focusable = target.closest<HTMLElement>(FOCUSABLE_SELECTOR)
    if (focusable && !modalRoot.value?.contains(focusable)) lastPointerTarget = focusable
  }

  function containDocumentFocus(event: FocusEvent) {
    if (!modalActive) return
    const root = modalRoot.value
    if (!root || root.closest('[inert]')) return
    const target = event.target
    if (!(target instanceof Node) || root.contains(target)) return
    ;(focusableElements()[0] ?? root).focus({ preventScroll: true })
  }

  function restoreFocus() {
    const target = previouslyFocused ?? lastPointerTarget
    previouslyFocused = null
    lastPointerTarget = null
    if (!target?.isConnected || target.hasAttribute('disabled')) return

    void nextTick(() => {
      restoreFrame = window.requestAnimationFrame(() => {
        restoreFrame = 0
        if (!target.isConnected || target.hasAttribute('disabled') || target.closest('[inert]')) return
        target.focus({ preventScroll: true })
      })
    })
  }

  function handleModalKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
      return
    }
    if (event.key !== 'Tab') return

    const root = modalRoot.value
    if (!root) return
    const focusables = focusableElements()
    if (!focusables.length) {
      event.preventDefault()
      root.focus({ preventScroll: true })
      return
    }

    const first = focusables[0]
    const last = focusables[focusables.length - 1]
    const active = document.activeElement
    if (active === root) {
      event.preventDefault()
      ;(event.shiftKey ? last : first).focus({ preventScroll: true })
    } else if (event.shiftKey && active === first) {
      event.preventDefault()
      last.focus({ preventScroll: true })
    } else if (!event.shiftKey && active === last) {
      event.preventDefault()
      first.focus({ preventScroll: true })
    }
  }

  watch(isOpen, (open) => {
    modalActive = open
    if (open) {
      if (restoreFrame) {
        window.cancelAnimationFrame(restoreFrame)
        restoreFrame = 0
      }
      const active = document.activeElement
      const activeElement = active instanceof HTMLElement && active !== document.body && active !== document.documentElement
        ? active
        : null
      previouslyFocused = activeElement ?? (lastPointerTarget?.isConnected ? lastPointerTarget : null)
      void nextTick(() => modalRoot.value?.focus({ preventScroll: true }))
    } else {
      restoreFocus()
    }
  }, { immediate: true })

  onMounted(() => {
    document.addEventListener('pointerdown', rememberPointerTarget, true)
    document.addEventListener('focusin', containDocumentFocus)
  })

  onBeforeUnmount(() => {
    modalActive = false
    document.removeEventListener('pointerdown', rememberPointerTarget, true)
    document.removeEventListener('focusin', containDocumentFocus)
    if (restoreFrame) window.cancelAnimationFrame(restoreFrame)
  })

  return { modalRoot, handleModalKeydown }
}
