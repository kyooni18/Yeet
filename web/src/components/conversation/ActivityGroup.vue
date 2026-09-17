<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { ConversationEntry, ConversationToolCall } from '@/remote/protocol'
import ToolCallCard from './ToolCallCard.vue'

const props = withDefaults(defineProps<{
  entries?: ConversationEntry[]
  tools?: ConversationToolCall[]
}>(), {
  entries: () => [],
  tools: () => [],
})

type ActivityEntry = Omit<ConversationEntry, 'kind'> & {
  kind: Extract<ConversationEntry['kind'], { type: 'activity' }>
}
type ActivityRow =
  | { key: string; type: 'activity'; entry: ActivityEntry }
  | { key: string; type: 'tool'; tool: ConversationToolCall }

const open = ref(false)
const userToggled = ref(false)

const rows = computed<ActivityRow[]>(() => {
  const result: ActivityRow[] = []
  for (const entry of props.entries) {
    if (entry.kind.type === 'activity') {
      result.push({
        key: entry.id,
        type: 'activity',
        entry: { ...entry, kind: entry.kind } as ActivityEntry,
      })
    } else if (entry.kind.type === 'toolCall') {
      result.push({ key: entry.id, type: 'tool', tool: entry.kind.toolCall })
    }
  }
  for (const tool of props.tools) result.push({ key: `tool-${tool.id}`, type: 'tool', tool })
  return result
})

const toolCount = computed(() => rows.value.filter((row) => row.type === 'tool').length)
const completedCount = computed(() => rows.value.filter((row) => row.type === 'tool' && row.tool.status === 'completed').length)
const failedCount = computed(() => rows.value.filter((row) => row.type === 'tool' && row.tool.status === 'failed').length)
const runningCount = computed(() => rows.value.filter((row) => row.type === 'tool' && row.tool.status === 'streaming').length)
const activeActivity = computed(() => [...rows.value].reverse().find((row) => row.type === 'activity' && !isTerminal(row.entry.kind.activity.phase)) as Extract<ActivityRow, { type: 'activity' }> | undefined)
const isLive = computed(() => runningCount.value > 0 || Boolean(activeActivity.value))

const title = computed(() => activeActivity.value?.entry.kind.activity.title || (toolCount.value ? 'Tool activity' : 'Activity'))
const summary = computed(() => {
  const parts: string[] = []
  if (toolCount.value) parts.push(`${toolCount.value} tool call${toolCount.value === 1 ? '' : 's'}`)
  if (completedCount.value) parts.push(`${completedCount.value} complete`)
  if (runningCount.value) parts.push(`${runningCount.value} running`)
  if (failedCount.value) parts.push(`${failedCount.value} failed`)
  return parts.join(' · ') || 'Task update'
})

const statusLabel = computed(() => {
  if (failedCount.value) return 'Needs attention'
  if (isLive.value) return 'In progress'
  return 'Complete'
})

function isTerminal(phase: unknown) {
  return ['done', 'failed', 'interrupted'].includes(String(phase ?? ''))
}

function activityMarker(entry: ActivityEntry) {
  const phase = String(entry.kind.activity.phase ?? 'active')
  if (phase === 'done') return '✓'
  if (phase === 'failed') return '!'
  if (phase === 'interrupted') return '■'
  return '⟳'
}

function handleToggle(event: Event) {
  if (event.isTrusted) userToggled.value = true
  open.value = (event.currentTarget as HTMLDetailsElement).open
}

let wasLive = false
watch([isLive, failedCount], ([live, failed]) => {
  if (!userToggled.value && (live || failed > 0)) open.value = true
  if (!userToggled.value && wasLive && !live && failed === 0) open.value = false
  wasLive = live
}, { immediate: true })
</script>

<template>
  <details
    v-if="rows.length"
    class="activity-group"
    :class="{ 'is-live': isLive, 'is-error': failedCount > 0 }"
    :open="open"
    data-testid="activity-group"
    @toggle="handleToggle"
  >
    <summary :aria-label="`${title}: ${summary}`">
      <span class="activity-group-icon" aria-hidden="true">{{ isLive ? '⟳' : failedCount ? '!' : '✓' }}</span>
      <span class="activity-group-heading">
        <strong>{{ title }}</strong>
        <span class="truncate">{{ summary }}</span>
      </span>
      <span class="activity-group-status">{{ statusLabel }}</span>
      <span class="activity-group-disclosure" aria-hidden="true">⌄</span>
    </summary>

    <div v-if="open" class="activity-group-body">
      <div v-for="row in rows" :key="row.key" class="activity-group-row">
        <template v-if="row.type === 'activity'">
          <div class="activity-group-activity" :class="{ 'is-live': !isTerminal(row.entry.kind.activity.phase), 'is-error': String(row.entry.kind.activity.phase) === 'failed' }">
            <span aria-hidden="true">{{ activityMarker(row.entry) }}</span>
            <strong>{{ row.entry.kind.activity.title }}</strong>
            <span v-if="row.entry.kind.activity.detail" class="truncate">{{ row.entry.kind.activity.detail }}</span>
          </div>
        </template>
        <ToolCallCard v-else :tool="row.tool" />
      </div>
    </div>
  </details>
</template>
