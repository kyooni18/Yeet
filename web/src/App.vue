<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'
import { RouterView, useRoute } from 'vue-router'
import AuthGate from '@/components/AuthGate.vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const route = useRoute()

const updateViewport = () => {
  const viewport = window.visualViewport
  const height = viewport?.height ?? window.innerHeight
  const offsetTop = viewport?.offsetTop ?? 0
  document.documentElement.style.setProperty('--visual-viewport-height', `${height}px`)
  document.documentElement.style.setProperty('--visual-viewport-top', `${offsetTop}px`)
}

onMounted(() => {
  remote.init()
  updateViewport()
  window.visualViewport?.addEventListener('resize', updateViewport)
  window.visualViewport?.addEventListener('scroll', updateViewport)
  window.addEventListener('resize', updateViewport)
})

onBeforeUnmount(() => {
  remote.destroy()
  window.visualViewport?.removeEventListener('resize', updateViewport)
  window.visualViewport?.removeEventListener('scroll', updateViewport)
  window.removeEventListener('resize', updateViewport)
})
</script>

<template>
  <RouterView />
  <AuthGate v-if="remote.connection === 'auth-required' && route.name !== 'enroll'" />
</template>
