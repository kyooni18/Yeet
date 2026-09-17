<script setup lang="ts">
import { computed } from 'vue'
import ActivityInspector from '@/components/ActivityInspector.vue'
import Composer from '@/components/Composer.vue'
import SessionControls from '@/components/MobileStatusSheet.vue'
import SessionSidebar from '@/components/SessionSidebar.vue'
import TopBar from '@/components/TopBar.vue'
import Transcript from '@/components/conversation/Transcript.vue'
import { useModalFocus } from '@/composables/useModalFocus'
import { useMediaQuery } from '@/composables/useMediaQuery'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const inspectorOverlayViewport = useMediaQuery('(max-width: 1199px)')
const inspectorModalOpen = computed(() => remote.inspectorOpen && inspectorOverlayViewport.value)
const shellModalOpen = computed(() => remote.mobileStatusOpen || remote.mobileSessionsOpen || inspectorModalOpen.value)

function closeSessionDrawer() {
  remote.mobileSessionsOpen = false
}


function focusConversation() {
  document.getElementById('conversation-transcript')?.focus({ preventScroll: true })
}

const { modalRoot: sessionDrawer, handleModalKeydown: handleSessionDrawerKeydown } = useModalFocus(
  () => remote.mobileSessionsOpen,
  closeSessionDrawer,
)
</script>

<template>
  <div class="remote-app" :class="{ 'inspector-is-open': remote.inspectorOpen }">
    <a v-if="!shellModalOpen && remote.connection !== 'auth-required'" class="skip-link" href="#conversation-transcript" @click.prevent="focusConversation">Skip to conversation</a>
    <SessionSidebar
      class="desktop-session-sidebar"
      :inert="shellModalOpen || undefined"
      :aria-hidden="shellModalOpen ? 'true' : undefined"
    />

    <section
      class="conversation-pane"
      :inert="shellModalOpen || undefined"
      :aria-hidden="shellModalOpen ? 'true' : undefined"
    >
      <TopBar />
      <Transcript />
      <Composer />
    </section>

    <Transition name="drawer-fade">
      <div v-if="inspectorModalOpen" class="mobile-inspector-backdrop" aria-hidden="true" @click="remote.inspectorOpen = false"></div>
    </Transition>
    <ActivityInspector
      v-show="remote.inspectorOpen"
      :modal="inspectorOverlayViewport"
      :inert="remote.mobileStatusOpen || remote.mobileSessionsOpen || undefined"
      :aria-hidden="remote.mobileStatusOpen || remote.mobileSessionsOpen ? 'true' : undefined"
    />

    <Transition name="drawer-fade">
      <div
        v-if="remote.mobileSessionsOpen"
        ref="sessionDrawer"
        class="drawer-layer"
        data-testid="session-drawer"
        role="dialog"
        aria-modal="true"
        aria-label="Sessions"
        tabindex="-1"
        :inert="remote.mobileStatusOpen || undefined"
        :aria-hidden="remote.mobileStatusOpen ? 'true' : undefined"
        @keydown="handleSessionDrawerKeydown"
        @click.self="closeSessionDrawer"
      >
        <SessionSidebar drawer @close="closeSessionDrawer" />
      </div>
    </Transition>

    <SessionControls />
  </div>
</template>
