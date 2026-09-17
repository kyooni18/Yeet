<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import type { ConversationEntry as ConversationEntryData } from '@/remote/protocol'
import { useRemoteStore } from '@/stores/remote'
import YeetMark from '@/components/YeetMark.vue'
import ConversationEntry from './ConversationEntry.vue'
import MarkdownDocument from './MarkdownDocument.vue'
import ActivityGroup from './ActivityGroup.vue'

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

const transcriptBlocks = computed<TranscriptBlock[]>(() => {
  const blocks: TranscriptBlock[] = []
  let grouped: ConversationEntryData[] = []
  const flush = () => {
    if (grouped.length) blocks.push({ type: 'activity-group', entries: grouped })
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

function plainTaskPreview(value: string) {
  return value
    .replace(/```[\s\S]*?```/g, ' code ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/[*_~#>|]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

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

const taskStatus = computed(() => {
  if (!remote.state.is_streaming) return null
  const reasoning = activeTaskReasoning.value
  const activity = activeTaskActivity.value
  const content = remote.state.active_reasoning_text || reasoning?.kind.content || ''
  const summary = plainTaskPreview(
    remote.state.active_reasoning_summary || reasoning?.kind.summary || content || activity?.kind.activity.detail || '',
  )

  if (!reasoning && !activity && !remote.state.active_assistant_entry_id) return null

  return {
    label: reasoning ? 'Thinking' : activity?.kind.activity.title || 'Working',
    summary: summary || (remote.state.active_assistant_entry_id ? 'Writing a response' : 'Latest task update'),
    content,
    disclosureLabel: reasoning ? 'Latest reasoning' : 'Task details',
  }
})

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
      return { title: 'What can Yeet do for you?', detail: 'Ask it to build, investigate, use tools, inspect files, or continue where you left off.' }
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
      <details
        v-if="taskStatus"
        class="task-status-card"
        :class="{ 'is-open': taskStatusOpen }"
        @toggle="handleTaskStatusToggle"
      >
        <summary :aria-label="`Show ${taskStatus.disclosureLabel.toLocaleLowerCase()}`">
          <span class="task-status-icon" aria-hidden="true"><span class="task-status-pulse"></span></span>
          <span class="task-status-copy">
            <strong>{{ taskStatus.label }}</strong>
            <span class="truncate">{{ taskStatus.summary }}</span>
          </span>
          <span class="task-status-label">{{ taskStatus.disclosureLabel }}</span>
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
</style>
