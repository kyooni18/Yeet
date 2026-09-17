<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, watch } from 'vue'
import { RouterView, useRoute } from 'vue-router'
import AuthGate from '@/components/AuthGate.vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const route = useRoute()
const authDialogOpen = computed(() => remote.connection === 'auth-required' && route.name !== 'enroll')

const normalizeTitlePart = (value?: string | null) => value?.replace(/\s+/g, ' ').trim() ?? ''
const currentWorkspace = computed(() => {
  const root = remote.state.workspace_root
  if (root) {
    const exact = remote.knownWorkspaces.find((workspace) => workspace.path === root || workspace.id === root)
    if (exact) return exact
  }
  return remote.knownWorkspaces.find((workspace) => workspace.is_current) ?? null
})
const pageTitle = computed(() => {
  if (route.name === 'enroll') return 'Authorize — Yeet'
  if (remote.connection === 'auth-required') return 'Authorization required — Yeet'
  if (route.name === 'settings') return 'Settings — Yeet'

  const session = normalizeTitlePart(remote.currentSession?.title)
  const workspace = normalizeTitlePart(currentWorkspace.value?.display_name)
  if (session) {
    if (workspace && workspace.toLowerCase() !== 'yeet') return `${session} · ${workspace} — Yeet`
    return `${session} — Yeet`
  }
  if (workspace && workspace.toLowerCase() !== 'yeet') return `${workspace} — Yeet`
  return 'Yeet Remote'
})

watch(pageTitle, (title) => {
  document.title = title
}, { immediate: true })

const updateViewport = () => {
  const viewport = window.visualViewport
  const height = viewport?.height ?? window.innerHeight
  const top = viewport?.offsetTop ?? 0
  const bottom = Math.max(0, window.innerHeight - top - height)
  document.documentElement.style.setProperty('--visual-viewport-top', `${top}px`)
  document.documentElement.style.setProperty('--visual-viewport-height', `${height}px`)
  document.documentElement.style.setProperty('--visual-viewport-bottom', `${bottom}px`)
}

onMounted(() => {
  remote.init()
  updateViewport()
  window.visualViewport?.addEventListener('resize', updateViewport)
  window.addEventListener('resize', updateViewport)
})

onBeforeUnmount(() => {
  remote.destroy()
  window.visualViewport?.removeEventListener('resize', updateViewport)
  window.removeEventListener('resize', updateViewport)
})
</script>

<template>
  <RouterView v-slot="{ Component }">
    <component
      :is="Component"
      :inert="authDialogOpen || undefined"
      :aria-hidden="authDialogOpen ? 'true' : undefined"
    />
  </RouterView>
  <AuthGate v-if="authDialogOpen" />
</template>
