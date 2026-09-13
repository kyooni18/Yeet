<script setup lang="ts">
import ActivityInspector from '@/components/ActivityInspector.vue'
import Composer from '@/components/Composer.vue'
import SessionControls from '@/components/MobileStatusSheet.vue'
import SessionSidebar from '@/components/SessionSidebar.vue'
import TopBar from '@/components/TopBar.vue'
import Transcript from '@/components/conversation/Transcript.vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
</script>

<template>
  <div class="remote-app" :class="{ 'inspector-is-open': remote.inspectorOpen }">
    <SessionSidebar class="desktop-session-sidebar" />

    <section class="conversation-pane">
      <TopBar />
      <Transcript />
      <Composer />
    </section>

    <Transition name="drawer-fade">
      <div v-if="remote.inspectorOpen" class="mobile-inspector-backdrop" aria-hidden="true" @click="remote.inspectorOpen = false"></div>
    </Transition>
    <ActivityInspector v-if="remote.inspectorOpen" />

    <Transition name="drawer-fade">
      <div v-if="remote.mobileSessionsOpen" class="drawer-layer" data-testid="session-drawer" @click.self="remote.mobileSessionsOpen = false">
        <SessionSidebar drawer @close="remote.mobileSessionsOpen = false" />
      </div>
    </Transition>

    <SessionControls />
  </div>
</template>
