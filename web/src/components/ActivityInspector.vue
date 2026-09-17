<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { useModalFocus } from '@/composables/useModalFocus'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const props = withDefaults(defineProps<{ modal?: boolean }>(), { modal: false })

function closeInspector() {
  const invoker = props.modal ? null : document.getElementById('activity-inspector-toggle')
  remote.inspectorOpen = false
  if (!(invoker instanceof HTMLElement)) return
  void nextTick(() => {
    if (invoker.isConnected && !invoker.hasAttribute('disabled')) invoker.focus({ preventScroll: true })
  })
}

const { modalRoot: inspectorRoot, handleModalKeydown } = useModalFocus(
  () => props.modal && remote.inspectorOpen,
  closeInspector,
)

function onInspectorKeydown(event: KeyboardEvent) {
  if (props.modal) {
    handleModalKeydown(event)
    return
  }
  if (event.key !== 'Escape') return
  event.preventDefault()
  event.stopPropagation()
  closeInspector()
}
const minPaneWidth = 280
const maxPaneWidth = 480
const paneWidth = ref(336)
const expandedEntryIds = ref(new Set<string>())

const activityEntries = computed(() => remote.entries
  .filter((entry) => ['activity', 'toolCall', 'skill', 'mcp'].includes(entry.kind.type))
  .slice(-30)
  .reverse())

type ActivityEntry = (typeof activityEntries.value)[number]

const statusOf = (entry: ActivityEntry) => {
  if (entry.kind.type === 'toolCall') return entry.kind.toolCall.status
  if (entry.kind.type === 'activity') return String(entry.kind.activity.phase ?? 'active')
  if (entry.kind.type === 'mcp') return entry.kind.isError ? 'failed' : 'complete'
  if (entry.kind.type === 'skill') return entry.kind.status || 'active'
  return 'active'
}

function durationLabel(ms?: number | null) {
  if (ms == null) return ''
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`
}

function toolDetail(argumentsText: string, explicit?: string | null) {
  if (explicit) return explicit
  const keys = ['path', 'query', 'command', 'url', 'capability', 'server']
  try {
    const args = JSON.parse(argumentsText || '{}') as Record<string, unknown>
    for (const key of keys) {
      if (typeof args[key] === 'string' && args[key]) return String(args[key])
    }
  } catch {
    // Malformed/streaming arguments are still available in the expanded view.
  }
  return ''
}

function summaryOf(entry: ActivityEntry) {
  if (entry.kind.type === 'activity') return entry.kind.activity.detail || String(entry.kind.activity.phase ?? 'active')
  if (entry.kind.type === 'toolCall') {
    const tool = entry.kind.toolCall
    const detail = toolDetail(tool.arguments, tool.detail)
    const duration = durationLabel(tool.durationMs)
    return [detail || tool.status, duration].filter(Boolean).join(' · ')
  }
  if (entry.kind.type === 'skill') return entry.kind.status || 'active'
  if (entry.kind.type === 'mcp') return entry.kind.name
  return ''
}

function titleOf(entry: ActivityEntry) {
  if (entry.kind.type === 'activity') return entry.kind.activity.title
  if (entry.kind.type === 'toolCall') return entry.kind.toolCall.name
  if (entry.kind.type === 'skill') return `Skill · ${entry.kind.name}`
  if (entry.kind.type === 'mcp') return `MCP · ${entry.kind.server}`
  return 'Activity'
}

function isExpanded(id: string) {
  return expandedEntryIds.value.has(id)
}

function toggleEntry(id: string) {
  const next = new Set(expandedEntryIds.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expandedEntryIds.value = next
}

function prettyJson(value: unknown) {
  if (typeof value === 'string') return value
  if (value == null) return ''
  try { return JSON.stringify(value, null, 2) }
  catch { return String(value) }
}

function prettyArguments(value: string) {
  try { return JSON.stringify(JSON.parse(value || '{}'), null, 2) }
  catch { return value || '{}' }
}

function hasDetails(entry: ActivityEntry) {
  if (entry.kind.type === 'activity') return Boolean(entry.kind.activity.detail)
  if (entry.kind.type === 'toolCall') {
    const tool = entry.kind.toolCall
    return Boolean(tool.arguments || tool.result != null || tool.error)
  }
  if (entry.kind.type === 'skill') return Boolean(entry.kind.content)
  if (entry.kind.type === 'mcp') return Boolean(entry.kind.content)
  return false
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
  <aside
    ref="inspectorRoot"
    id="activity-inspector"
    class="activity-inspector"
    data-testid="activity-inspector"
    :role="props.modal ? 'dialog' : undefined"
    :aria-modal="props.modal ? 'true' : undefined"
    :aria-labelledby="props.modal ? 'activity-inspector-title' : undefined"
    :tabindex="props.modal ? -1 : undefined"
    @keydown="onInspectorKeydown"
  >
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
      <div><h2 id="activity-inspector-title">Activity</h2></div>
      <button class="icon-button" aria-label="Close inspector" @click="closeInspector">×</button>
    </div>

    <div class="inspector-summary">
      <div><span>Run</span><strong>{{ remote.state.active_run_id ? remote.state.active_run_id.slice(0, 8) : 'idle' }}</strong></div>
      <div><span>Context</span><strong>{{ remote.contextPercent == null ? '—' : `${remote.contextPercent}%` }}</strong></div>
      <div><span>Permissions</span><strong>{{ remote.permissionMode }}</strong></div>
    </div>

    <div class="inspector-list">
      <article v-for="entry in activityEntries" :key="entry.id" class="inspector-event">
        <button
          class="inspector-event-summary"
          type="button"
          :aria-expanded="hasDetails(entry) ? isExpanded(entry.id) : undefined"
          :disabled="!hasDetails(entry)"
          @click="toggleEntry(entry.id)"
        >
          <span class="trace-node" :class="`trace-${statusOf(entry)}`"></span>
          <span class="inspector-event-copy">
            <strong>{{ titleOf(entry) }}</strong>
            <span class="inspector-event-meta" :title="summaryOf(entry)">{{ summaryOf(entry) }}</span>
          </span>
          <span v-if="hasDetails(entry)" class="inspector-disclosure" aria-hidden="true">⌄</span>
        </button>

        <div v-if="hasDetails(entry) && isExpanded(entry.id)" class="inspector-event-details" data-testid="inspector-event-details">
          <template v-if="entry.kind.type === 'activity'">
            <p>{{ entry.kind.activity.detail }}</p>
            <span v-if="durationLabel(entry.kind.activity.durationMs)" class="inspector-detail-caption">{{ durationLabel(entry.kind.activity.durationMs) }}</span>
          </template>

          <template v-else-if="entry.kind.type === 'toolCall'">
            <section>
              <span class="inspector-detail-caption">Arguments</span>
              <pre>{{ prettyArguments(entry.kind.toolCall.arguments) }}</pre>
            </section>
            <section v-if="entry.kind.toolCall.error || entry.kind.toolCall.result != null">
              <span class="inspector-detail-caption">{{ entry.kind.toolCall.error ? 'Error' : 'Result' }}</span>
              <pre :class="{ 'is-error': Boolean(entry.kind.toolCall.error) }">{{ entry.kind.toolCall.error || prettyJson(entry.kind.toolCall.result) }}</pre>
            </section>
          </template>

          <template v-else-if="entry.kind.type === 'skill'">
            <pre>{{ entry.kind.content }}</pre>
          </template>

          <template v-else-if="entry.kind.type === 'mcp'">
            <pre :class="{ 'is-error': entry.kind.isError }">{{ entry.kind.content }}</pre>
          </template>
        </div>
      </article>
      <div v-if="!activityEntries.length" class="empty-inspector">Activity from the current session will appear here.</div>
    </div>
  </aside>
</template>

<style scoped>
.inspector-event {
  display: block;
  padding: 2px 0;
}

.inspector-event + .inspector-event {
  border-top: 1px solid rgba(255, 255, 255, .035);
}

.inspector-event-summary {
  display: grid;
  width: 100%;
  min-height: 46px;
  grid-template-columns: 11px minmax(0, 1fr) 18px;
  align-items: center;
  gap: 9px;
  padding: 7px 5px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  text-align: left;
}

.inspector-event-summary:not(:disabled) { cursor: pointer; }
.inspector-event-summary:not(:disabled):hover,
.inspector-event-summary[aria-expanded="true"] { background: rgba(255, 255, 255, .035); }
.inspector-event-summary:focus-visible { outline: 2px solid var(--brand); outline-offset: -2px; }
.inspector-event-summary:disabled { opacity: 1; }
.inspector-event-summary .trace-node { margin-top: 0; }

.inspector-event-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
}

.inspector-event-copy strong,
.inspector-event-meta {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inspector-event-copy strong { color: var(--soft); font-size: 11.5px; font-weight: 620; }
.inspector-event-meta { color: var(--muted); font-size: 10.5px; }
.inspector-disclosure { color: var(--dim); font-size: 13px; text-align: center; transition: transform 120ms ease; }
.inspector-event-summary[aria-expanded="true"] .inspector-disclosure { transform: rotate(180deg); }

.inspector-event-details {
  margin: 1px 4px 8px 20px;
  padding: 8px 9px;
  border: 1px solid rgba(255, 255, 255, .05);
  border-radius: 8px;
  background: rgba(0, 0, 0, .12);
  color: var(--muted);
  font-size: 10.5px;
  line-height: 1.5;
  user-select: text;
}

.inspector-event-details section + section { margin-top: 9px; }
.inspector-event-details p { margin: 0; white-space: pre-wrap; }
.inspector-event-details pre {
  max-height: 240px;
  margin: 5px 0 0;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--soft);
  font: 10px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
  user-select: text;
}
.inspector-event-details pre.is-error { color: var(--red); }
.inspector-detail-caption { display: block; color: var(--dim); font-size: 9px; font-weight: 650; letter-spacing: .04em; text-transform: uppercase; }

@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .inspector-header .icon-button { width: 44px; height: 44px; }
}

@media (hover: none) and (pointer: coarse) and (min-width: 1200px) {
  .activity-inspector { overflow: visible; }
  .inspector-resize-handle {
    left: -40px;
    width: 44px;
  }
  .inspector-resize-handle::after { left: 39px; }
}

@media (max-width: 899px) {
  .inspector-event-summary { min-height: 48px; padding: 8px 7px; }
  .inspector-event-details { margin-left: 22px; }
}
</style>
