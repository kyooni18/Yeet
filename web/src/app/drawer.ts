import { bindElement } from './dom'

/** DOM-only behavior for responsive navigation drawers, not application state. */
function drawerFocus(element: HTMLElement) {
  const inspector = element.id === 'workspace-panel'
  const media = matchMedia(inspector ? '(min-width: 1280px)' : '(min-width: 900px)')
  let restore: HTMLElement | null = null
  let background: HTMLElement[] = []
  let modal = false
  const focusables = () => [...element.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), [tabindex="0"]')]
    .filter(node => node.getClientRects().length > 0)
  const trap = (event: KeyboardEvent) => {
    if (media.matches || event.key !== 'Tab') return
    const items = focusables()
    const first = items[0], last = items.at(-1)
    if (!first) { event.preventDefault(); return }
    if (event.shiftKey && (document.activeElement === first || !element.contains(document.activeElement))) {
      event.preventDefault(); last?.focus()
    } else if (!event.shiftKey && (document.activeElement === last || !element.contains(document.activeElement))) {
      event.preventDefault(); first.focus()
    }
  }
  const release = () => {
    if (!modal) return
    modal = false
    background.forEach(node => { node.inert = false })
    background = []
    // A second drawer or sheet may have taken focus during dismissal.
    if (element.contains(document.activeElement) || document.activeElement === document.body) {
      // The navigation trigger is reinserted in the same commit as dismissal.
      requestAnimationFrame(() => {
        if (document.activeElement !== document.body && !element.contains(document.activeElement)) return
        const fallback = document.querySelector<HTMLElement>(inspector ? '[aria-controls="workspace-panel"]' : '[aria-controls="sidebar"]')
        // Touch browsers do not necessarily focus the tapped trigger; prefer
        // that trigger over whatever unrelated control was focused beforehand.
        const target = fallback ?? (restore?.isConnected && restore !== document.body && !restore.closest('[inert]') ? restore : null)
        target?.focus({ preventScroll: true })
      })
    }
  }
  const sync = () => {
    if (media.matches) { release(); return }
    if (modal) return
    modal = true
    restore = document.activeElement instanceof HTMLElement ? document.activeElement : null
    background = [...document.querySelectorAll<HTMLElement>(inspector ? '#main, #sidebar' : '#main, #workspace-panel')]
      .filter(node => !node.inert)
    background.forEach(node => { node.inert = true })
    focusables()[0]?.focus({ preventScroll: true })
  }
  sync()
  media.addEventListener('change', sync)
  document.addEventListener('keydown', trap)
  return () => {
    media.removeEventListener('change', sync)
    document.removeEventListener('keydown', trap)
    release()
  }
}

export const sidebarRef = bindElement(drawerFocus)
export const inspectorRef = bindElement(drawerFocus)
