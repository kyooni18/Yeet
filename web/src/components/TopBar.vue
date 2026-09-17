<script setup lang="ts">
import { computed } from 'vue'
import { useRemoteStore } from '@/stores/remote'
import ModelPicker from './ModelPicker.vue'

const remote = useRemoteStore()
const connectionLabel = computed(() => remote.connection.replace('-', ' '))
const sessionControlsReadOnly = computed(() => remote.connection !== 'connected')

function openSessionControls() {
  remote.mobileStatusOpen = true
}
</script>

<template>
  <header class="top-bar">
    <button class="icon-button mobile-only" data-testid="open-sessions" aria-label="Open sessions" @click="remote.mobileSessionsOpen = true">
      <span class="menu-lines" aria-hidden="true"></span>
    </button>

    <fieldset class="top-product-group" :disabled="sessionControlsReadOnly">
      <ModelPicker variant="topbar" />
    </fieldset>

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
      <button class="icon-button goal-toggle" :class="{ active: remote.state.goal_mode }" data-testid="toggle-goal" :disabled="sessionControlsReadOnly" :aria-label="remote.state.goal_mode ? 'Disable Goal mode' : 'Enable Goal mode'" :aria-pressed="remote.state.goal_mode" @click="remote.setGoal(!remote.state.goal_mode)">◎</button>
      <button id="activity-inspector-toggle" class="icon-button inspector-toggle" data-testid="toggle-inspector" aria-label="Toggle activity inspector" aria-controls="activity-inspector" :aria-pressed="remote.inspectorOpen" @click="remote.inspectorOpen = !remote.inspectorOpen">
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
  margin: 0;
  padding: 0;
  border: 0;
}
.goal-toggle { font-size: 17px; font-weight: 700; }
.goal-toggle.active { border-color: var(--brand-hot); color: var(--brand-hot); background: rgba(255,103,70,.08); }
@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .top-bar .icon-button {
    width: 44px;
    height: 44px;
    min-height: 44px;
  }
  .connection-button { min-height: 44px; }
  .top-product-group :deep(.model-picker-trigger) { min-height: 44px; }
  .top-product-group :deep(.model-picker-search) {
    min-height: 44px !important;
    font-size: 16px;
  }
  .top-product-group :deep(.model-picker-providers button) {
    min-width: 44px;
    min-height: 44px;
  }
}
@media (max-width: 480px) {
  .top-product-group { flex: 1 1 auto; }
  .top-status-cluster { flex: 0 0 auto; }
  .goal-toggle,
  .desktop-status.is-connected { display: none; }
}
</style>
