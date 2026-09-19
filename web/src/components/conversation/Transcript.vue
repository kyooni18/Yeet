<script setup lang="ts">
import { computed, nextTick, onMounted, provide, ref, watch } from 'vue'
import type { ConversationEntry as ConversationEntryData } from '@/remote/protocol'
import { useRemoteStore } from '@/stores/remote'
import YeetMark from '@/components/YeetMark.vue'
import ConversationEntry from './ConversationEntry.vue'
import MarkdownDocument from './MarkdownDocument.vue'
import ActivityGroup from './ActivityGroup.vue'
import { toolPurpose } from '@/utils/toolPurpose'

const remote = useRemoteStore()
const scroller = ref<HTMLElement | null>(null)
const following = ref(true)
const taskStatusOpen = ref(false)

type ReasoningEntry = ConversationEntryData & { kind: Extract<ConversationEntryData['kind'], { type: 'reasoning' }> }
type ActivityEntry = ConversationEntryData & { kind: Extract<ConversationEntryData['kind'], { type: 'activity' }> }
const isReasoningEntry = (entry: ConversationEntryData): entry is ReasoningEntry => entry.kind.type === 'reasoning'
const isActivityEntry = (entry: ConversationEntryData): entry is ActivityEntry => entry.kind.type === 'activity'

type TranscriptBlock =
  | { type: 'entry'; entry: ConversationEntryData }
  | { type: 'activity-group'; entries: ConversationEntryData[] }

const isGroupedEntry = (entry: ConversationEntryData) => entry.kind.type === 'activity' || entry.kind.type === 'toolCall'
const MAX_COMPACT_ACTIVITY_GROUP_ENTRIES = 12

const transcriptBlocks = computed<TranscriptBlock[]>(() => {
  const blocks: TranscriptBlock[] = []
  let grouped: ConversationEntryData[] = []
  const flush = () => {
    if (grouped.length > MAX_COMPACT_ACTIVITY_GROUP_ENTRIES) {
      for (const entry of grouped) blocks.push({ type: 'entry', entry })
    } else if (grouped.length) {
      blocks.push({ type: 'activity-group', entries: grouped })
    }
    grouped = []
  }

  for (const entry of remote.entries) {
    if (isGroupedEntry(entry)) grouped.push(entry)
    else {
      flush()
      blocks.push({ type: 'entry', entry })
    }
  }
  flush()
  return blocks
})

const activeTaskReasoning = computed(() => {
  const id = remote.state.active_reasoning_entry_id
  if (!id) return null
  const entry = remote.entries.find((candidate) => candidate.id === id)
  return entry && isReasoningEntry(entry) ? entry : null
})

const activeTaskActivity = computed(() => {
  const id = remote.state.active_activity_entry_id
  if (!id) return null
  const entry = remote.entries.find((candidate) => candidate.id === id)
  return entry && isActivityEntry(entry) ? entry : null
})

// Derive history from the transcript so it survives session reloads, rather than
// keeping only the currently active task in local component state.
const taskHistory = computed(() => remote.entries.filter(isActivityEntry).filter((entry) =>
  ['done', 'failed', 'interrupted'].includes(String(entry.kind.activity.phase ?? '')),
))

const taskStatus = computed(() => {
  const turnStart = remote.entries.map((entry) => entry.kind.type).lastIndexOf('user')
  const turn = remote.entries.slice(Math.max(0, turnStart))
  const latestActivity = [...turn].reverse().find(isActivityEntry)
  const phase = latestActivity?.kind.activity.phase
  const terminalLabel = phase === 'done' ? 'Complete' : phase === 'failed' ? 'Failed' : phase === 'interrupted' ? 'Interrupted' : null
  if (!remote.state.is_streaming && !terminalLabel) return null

  const tools = turn.flatMap((entry) => entry.kind.type === 'toolCall' ? [entry.kind.toolCall] : [])
  const completed = tools.filter((tool) => tool.status === 'completed').length
  const failed = tools.filter((tool) => tool.status === 'failed').length
  const progress = [completed ? `${completed} OK` : '', failed ? `${failed} ERR` : ''].filter(Boolean).join(' · ')
  const reasoning = activeTaskReasoning.value
  const activity = activeTaskActivity.value
  if (!remote.state.is_streaming) {
    return {
      label: terminalLabel,
      content: latestActivity?.kind.activity.detail || '',
      disclosureLabel: 'Task details',
      progress,
      state: phase,
    }
  }
  const content = remote.state.active_reasoning_text || reasoning?.kind.content || remote.state.active_reasoning_summary || reasoning?.kind.summary || activity?.kind.activity.detail || ''

  return {
    label: [...tools].reverse().map((tool) => toolPurpose(tool.arguments)).find(Boolean)
      || remote.state.active_reasoning_summary || reasoning?.kind.summary
      || content.trim().split('\n').filter(Boolean).at(-1)?.slice(0, 160)
      || (remote.state.active_assistant_entry_id ? 'Writing response' : 'Working'),
    content,
    disclosureLabel: reasoning ? 'Latest reasoning' : 'Task details',
    progress,
    state: 'working',
  }
})

// Suppress only the intent currently promoted to the live status, not tool history.
provide('promotedToolPurpose', computed(() => remote.state.is_streaming ? taskStatus.value?.label || '' : ''))

function handleTaskStatusToggle(event: Event) {
  taskStatusOpen.value = (event.currentTarget as HTMLDetailsElement).open
}

const emptyState = computed(() => {
  switch (remote.connection) {
    case 'connecting':
      return { title: 'Connecting to Yeet…', detail: 'Loading the current workspace and conversation.' }
    case 'reconnecting':
      return { title: 'Reconnecting to Yeet…', detail: 'Restoring the connection. Your draft is preserved while Yeet comes back.' }
    case 'offline':
      return { title: "You're offline", detail: 'Your draft is preserved. Reconnect to continue this conversation.' }
    case 'failed':
      return { title: "Can't reach Yeet", detail: remote.connectionError || 'Check the Remote server and try again.' }
    case 'auth-required':
      return { title: 'Authorization required', detail: 'Authorize this browser to load the conversation.' }
    default:
      return { title: 'New conversation', detail: 'Send a message to get started.' }
  }
})

const emptyStateIsStatus = computed(() => remote.connection !== 'connected')

function nearBottom() {
  const element = scroller.value
  if (!element) return true
  return element.scrollHeight - element.scrollTop - element.clientHeight < 160
}

function handleScroll() {
  following.value = nearBottom()
}

function scrollLatest(behavior: ScrollBehavior = 'smooth') {
  const element = scroller.value
  if (!element) return
  following.value = true
  if (!remote.entries.length) {
    element.scrollTo({ top: 0, behavior })
    return
  }
  element.scrollTo({ top: element.scrollHeight, behavior })
}

function scrollLatestAndFocus() {
  scroller.value?.focus({ preventScroll: true })
  scrollLatest('auto')
}

function handleTranscriptKeydown(event: KeyboardEvent) {
  const element = scroller.value
  if (!element || event.target !== element || event.altKey || event.ctrlKey || event.metaKey) return

  const pageDistance = Math.max(120, element.clientHeight - 80)
  if (event.key === 'PageUp') {
    event.preventDefault()
    element.scrollBy({ top: -pageDistance, behavior: 'auto' })
  } else if (event.key === 'PageDown') {
    event.preventDefault()
    element.scrollBy({ top: pageDistance, behavior: 'auto' })
  } else if (event.key === 'Home') {
    event.preventDefault()
    following.value = false
    element.scrollTo({ top: 0, behavior: 'auto' })
  } else if (event.key === 'End') {
    event.preventDefault()
    scrollLatest('auto')
  }
}

watch(
  () => [remote.state.conversation_revision, remote.state.active_assistant_text, remote.state.active_reasoning_text, remote.entries.length],
  () => {
    if (!following.value) return
    void nextTick(() => scrollLatest('auto'))
  },
)

watch(
  () => [remote.state.workspace_root, remote.state.current_session_id],
  () => {
    following.value = true
    void nextTick(() => scrollLatest('auto'))
  },
)

onMounted(() => void nextTick(() => scrollLatest('auto')))
</script>

<template>
  <main
    id="conversation-transcript"
    ref="scroller"
    class="transcript-scroller"
    data-testid="transcript"
    tabindex="0"
    aria-label="Conversation transcript"
    @scroll.passive="handleScroll"
    @keydown="handleTranscriptKeydown"
  >
    <div class="transcript">
      <div
        v-if="!remote.entries.length"
        class="empty-transcript"
        :role="emptyStateIsStatus ? 'status' : undefined"
        :aria-live="emptyStateIsStatus ? 'polite' : undefined"
      >
        <div class="empty-mark"><YeetMark :size="54" /></div>
        <h1>{{ emptyState.title }}</h1>
        <p>{{ emptyState.detail }}</p>
      </div>
      <template v-for="block in transcriptBlocks" :key="block.type === 'entry' ? block.entry.id : block.entries[0]?.id">
        <ConversationEntry v-if="block.type === 'entry'" :entry="block.entry" />
        <ActivityGroup v-else :entries="block.entries" />
      </template>
      <section v-if="taskHistory.length" class="task-history" aria-label="Task history">
        <h2>Task history</h2>
        <ul>
          <li v-for="entry in taskHistory" :key="entry.id" :class="`task-state-${entry.kind.activity.phase}`">
            <span class="task-status-icon" aria-hidden="true">{{ entry.kind.activity.phase === 'done' ? '✓' : entry.kind.activity.phase === 'failed' ? '×' : '■' }}</span>
            <span class="task-history-title">{{ entry.kind.activity.title }}</span>
            <span class="task-status-label">{{ entry.kind.activity.phase === 'done' ? 'Complete' : entry.kind.activity.phase === 'failed' ? 'Failed' : 'Interrupted' }}</span>
          </li>
        </ul>
      </section>
      <details
        v-if="taskStatus"
        class="task-status-card"
        :class="[{ 'is-open': taskStatusOpen }, `task-state-${taskStatus.state}`]"
        @toggle="handleTaskStatusToggle"
      >
        <summary :aria-label="`Show ${taskStatus.disclosureLabel.toLocaleLowerCase()}`">
          <span class="task-status-icon" aria-hidden="true">
            <span v-if="taskStatus.state === 'working'" class="task-status-pulse"></span>
            <span v-else>{{ taskStatus.state === 'done' ? '✓' : taskStatus.state === 'failed' ? '×' : '■' }}</span>
          </span>
          <span class="task-status-copy">
            <strong role="status" aria-live="polite">{{ taskStatus.label }}</strong>
            <span v-if="taskStatus.content" class="task-status-summary-text">{{ taskStatus.content }}</span>
          </span>
          <span class="task-status-label">{{ taskStatus.progress }}</span>
          <span class="task-status-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div v-if="taskStatusOpen && taskStatus.content" class="task-status-body" tabindex="0" role="group" :aria-label="`${taskStatus.disclosureLabel} details`">
          <MarkdownDocument :content="taskStatus.content" />
        </div>
      </details>
    </div>
    <button v-if="!following" class="jump-latest" @click="scrollLatestAndFocus">↓ Latest</button>
  </main>
</template>

<style scoped>
.transcript-scroller:focus-visible {
  outline: 2px solid var(--brand);
  outline-offset: -2px;
}
.task-history {
  margin-block: 16px;
}
.task-history h2 {
  margin: 0 0 8px;
  font-size: 0.85rem;
}
.task-history ul {
  display: grid;
  gap: 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.task-history li {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.task-history-title {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
}
.task-status-summary-text {
  display: block;
  min-width: 0;
  max-width: min(56vw, 680px);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--muted);
}
</style>
