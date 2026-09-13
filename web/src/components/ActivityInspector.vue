<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const minPaneWidth = 280
const maxPaneWidth = 480
const paneWidth = ref(336)

const activityEntries = computed(() => remote.entries
  .filter((entry) => ['activity', 'toolCall', 'skill', 'mcp'].includes(entry.kind.type))
  .slice(-30)
  .reverse())

const statusOf = (entry: (typeof activityEntries.value)[number]) => {
  if (entry.kind.type === 'toolCall') return entry.kind.toolCall.status
  if (entry.kind.type === 'activity') return String(entry.kind.activity.phase ?? 'active')
  if (entry.kind.type === 'mcp') return entry.kind.isError ? 'failed' : 'complete'
  if (entry.kind.type === 'skill') return entry.kind.status || 'active'
  return 'active'
}

function applyPaneWidth(value: number, persist = false) {
  const next = Math.min(maxPaneWidth, Math.max(minPaneWidth, Math.round(value)))
  paneWidth.value = next
  document.documentElement.style.setProperty('--activity-pane-width', `${next}px`)
  if (!persist) return
  try {
    localStorage.setItem('yeet.activity-pane-width', String(next))
  } catch {
    // Storage can be unavailable in hardened/private browser contexts.
  }
}

function resizePane(event: PointerEvent) {
  applyPaneWidth(window.innerWidth - event.clientX)
}

function stopResize() {
  window.removeEventListener('pointermove', resizePane)
  window.removeEventListener('pointerup', stopResize)
  applyPaneWidth(paneWidth.value, true)
}

function startResize(event: PointerEvent) {
  if (window.innerWidth < 1200) return
  event.preventDefault()
  window.addEventListener('pointermove', resizePane)
  window.addEventListener('pointerup', stopResize)
}

function resizeWithKey(event: KeyboardEvent) {
  if (event.key === 'ArrowLeft') {
    event.preventDefault()
    applyPaneWidth(paneWidth.value + 24, true)
  } else if (event.key === 'ArrowRight') {
    event.preventDefault()
    applyPaneWidth(paneWidth.value - 24, true)
  }
}

onMounted(() => {
  try {
    const stored = Number(localStorage.getItem('yeet.activity-pane-width'))
    if (Number.isFinite(stored) && stored > 0) applyPaneWidth(stored)
    else applyPaneWidth(paneWidth.value)
  } catch {
    applyPaneWidth(paneWidth.value)
  }
})

onBeforeUnmount(() => {
  window.removeEventListener('pointermove', resizePane)
  window.removeEventListener('pointerup', stopResize)
})
</script>

<template>
  <aside class="activity-inspector" data-testid="activity-inspector">
    <div
      class="inspector-resize-handle"
      data-testid="inspector-resize-handle"
      role="separator"
      tabindex="0"
      aria-label="Resize activity panel"
      aria-orientation="vertical"
      :aria-valuenow="paneWidth"
      :aria-valuemin="minPaneWidth"
      :aria-valuemax="maxPaneWidth"
      @pointerdown="startResize"
      @keydown="resizeWithKey"
    ></div>

    <div class="inspector-header">
      <div>
        <h2>Activity</h2>
      </div>
      <button class="icon-button" aria-label="Close inspector" @click="remote.inspectorOpen = false">×</button>
    </div>

    <div class="inspector-summary">
      <div><span>Run</span><strong>{{ remote.state.active_run_id ? remote.state.active_run_id.slice(0, 8) : 'idle' }}</strong></div>
      <div><span>Context</span><strong>{{ remote.contextPercent == null ? '—' : `${remote.contextPercent}%` }}</strong></div>
      <div><span>Permissions</span><strong>{{ remote.permissionMode }}</strong></div>
    </div>

    <div class="inspector-list">
      <article v-for="entry in activityEntries" :key="entry.id" class="inspector-event">
        <span class="trace-node" :class="`trace-${statusOf(entry)}`"></span>
        <div>
          <template v-if="entry.kind.type === 'activity'">
            <strong>{{ entry.kind.activity.title }}</strong>
            <p v-if="entry.kind.activity.detail">{{ entry.kind.activity.detail }}</p>
          </template>
          <template v-else-if="entry.kind.type === 'toolCall'">
            <strong>{{ entry.kind.toolCall.name }}</strong>
            <p>{{ entry.kind.toolCall.status }}</p>
          </template>
          <template v-else-if="entry.kind.type === 'skill'">
            <strong>Skill · {{ entry.kind.name }}</strong>
            <p>{{ entry.kind.status || 'active' }}</p>
          </template>
          <template v-else-if="entry.kind.type === 'mcp'">
            <strong>MCP · {{ entry.kind.server }}</strong>
            <p>{{ entry.kind.name }}</p>
          </template>
        </div>
      </article>
      <div v-if="!activityEntries.length" class="empty-inspector">Activity from the current session will appear here.</div>
    </div>
  </aside>
</template>
