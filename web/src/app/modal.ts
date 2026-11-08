import { bindElement } from './dom'

/** Focus behavior only; sheet routing stays in the shell. */
export const modalRef = bindElement(element => {
  const restore = document.activeElement instanceof HTMLElement ? document.activeElement : null
  const background = [...document.querySelectorAll<HTMLElement>('#main, #sidebar, #workspace-panel')]
  let previous: boolean[] = []
  const items = () => [...element.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex="0"]')]
    .filter(node => node.getClientRects().length && !node.closest('[inert]'))
  const frame = requestAnimationFrame(() => {
    // Navigation unmounts in the same commit. Wait for its inert cleanup before
    // taking ownership, otherwise closing a sheet leaves mobile main inert.
    previous = background.map(node => node.inert)
    background.forEach(node => { node.inert = true })
    items()[0]?.focus({ preventScroll: true })
  })
  const trap = (event: KeyboardEvent) => {
    if (event.key !== 'Tab') return
    const list = items(), first = list[0], last = list.at(-1)
    if (!first) { event.preventDefault(); element.focus(); return }
    if (event.shiftKey && (document.activeElement === first || !element.contains(document.activeElement))) {
      event.preventDefault(); last?.focus()
    } else if (!event.shiftKey && (document.activeElement === last || !element.contains(document.activeElement))) {
      event.preventDefault(); first.focus()
    }
  }
  document.addEventListener('keydown', trap)
  return () => {
    cancelAnimationFrame(frame)
    document.removeEventListener('keydown', trap)
    background.forEach((node, index) => { node.inert = previous[index] ?? false })
    requestAnimationFrame(() => {
      if (document.querySelector('[data-sheet-dialog]')) return
      const fallback = document.querySelector<HTMLElement>('[aria-controls="sidebar"]')
      const target = restore?.isConnected && !restore.closest('[inert]') ? restore : fallback
      target?.focus({ preventScroll: true })
    })
  }
})
