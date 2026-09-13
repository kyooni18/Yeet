<script setup lang="ts">
import { computed } from 'vue'
import { useRemoteStore } from '@/stores/remote'
import ModelPicker from './ModelPicker.vue'

const remote = useRemoteStore()
const connectionLabel = computed(() => remote.connection.replace('-', ' '))

function openSessionControls() {
  remote.mobileStatusOpen = true
}
</script>

<template>
  <header class="top-bar">
    <button class="icon-button mobile-only" data-testid="open-sessions" aria-label="Open sessions" @click="remote.mobileSessionsOpen = true">
      <span class="menu-lines" aria-hidden="true"></span>
    </button>

    <div class="top-product-group">
      <ModelPicker variant="topbar" />
    </div>

    <div class="top-status-cluster">
      <button
        class="connection-button desktop-status"
        :class="{ 'is-connected': remote.connection === 'connected' }"
        data-testid="open-status"
        :aria-label="`Connection: ${connectionLabel}. Open session controls`"
        @click="openSessionControls"
      >
        <span class="status-dot" :class="`status-${remote.connection}`"></span>
        <span v-if="remote.connection !== 'connected'" class="connection-label">{{ connectionLabel }}</span>
        <span v-else class="sr-only">connected</span>
      </button>
      <button class="icon-button infinity-toggle" :class="{ active: remote.state.infinity_mode }" data-testid="toggle-infinity" :aria-label="remote.state.infinity_mode ? 'Disable Infinity mode' : 'Enable Infinity mode'" :aria-pressed="remote.state.infinity_mode" @click="remote.setInfinity(!remote.state.infinity_mode)">∞</button>
      <button class="icon-button inspector-toggle" data-testid="toggle-inspector" aria-label="Toggle activity inspector" :aria-pressed="remote.inspectorOpen" @click="remote.inspectorOpen = !remote.inspectorOpen">
        <span class="activity-icon" aria-hidden="true"></span>
      </button>
    </div>
  </header>
</template>

<style scoped>
.top-product-group {
  display: flex;
  min-width: 0;
  align-items: center;
}
.infinity-toggle { font-size: 17px; font-weight: 700; }
.infinity-toggle.active { border-color: var(--brand-hot); color: var(--brand-hot); background: rgba(255,103,70,.08); }
</style>
