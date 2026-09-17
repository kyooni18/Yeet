import { onBeforeUnmount, onMounted, ref } from 'vue'

export function useMediaQuery(query: string) {
  const matches = ref(false)
  let media: MediaQueryList | null = null

  function update(event?: MediaQueryListEvent) {
    matches.value = event?.matches ?? media?.matches ?? false
  }

  onMounted(() => {
    media = window.matchMedia(query)
    update()
    media.addEventListener('change', update)
  })

  onBeforeUnmount(() => {
    media?.removeEventListener('change', update)
  })

  return matches
}
