/** Mün web refs receive null on detach (their return value is not a cleanup). */
export function bindElement(setup: (element: HTMLElement) => () => void) {
  let current: HTMLElement | null = null
  let cleanup: (() => void) | undefined
  return (element: HTMLElement | null) => {
    if (element === current) return
    cleanup?.()
    current = element
    cleanup = element ? setup(element) : undefined
  }
}
