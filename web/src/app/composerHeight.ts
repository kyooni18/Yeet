import { bindElement } from './dom'

/**
 * The composer floats over the transcript, so the transcript needs to know how
 * much of its bottom edge is covered. Publishes it as `--composer-h` on the root.
 */
export const composerRef = bindElement(element => {
  const root = document.documentElement
  const publish = () => root.style.setProperty('--composer-h', `${Math.ceil(element.getBoundingClientRect().height)}px`)
  publish()
  const observer = new ResizeObserver(publish)
  observer.observe(element)
  return () => { observer.disconnect(); root.style.removeProperty('--composer-h') }
})
