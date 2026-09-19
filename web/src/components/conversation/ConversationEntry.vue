<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ConversationEntry } from '@/remote/protocol'
import YeetMark from '@/components/YeetMark.vue'
import { useRemoteStore } from '@/stores/remote'
import MarkdownDocument from './MarkdownDocument.vue'
import ToolCallCard from './ToolCallCard.vue'
import ActivityGroup from './ActivityGroup.vue'
import { copyTextToClipboard } from '@/utils/clipboard'

const props = defineProps<{ entry: ConversationEntry }>()
const remote = useRemoteStore()

const messageCopyStatus = ref<'idle' | 'copied' | 'failed'>('idle')
let messageCopyGeneration = 0
async function copyMessage() {
  if (props.entry.kind.type !== 'assistant') return
  const generation = ++messageCopyGeneration
  const copied = await copyTextToClipboard(props.entry.kind.content)
  if (generation !== messageCopyGeneration) return
  messageCopyStatus.value = copied ? 'copied' : 'failed'
  window.setTimeout(() => {
    if (generation === messageCopyGeneration) messageCopyStatus.value = 'idle'
  }, 1500)
}

const reasoningOpen = ref(false)
const semanticOpen = ref(false)
const showFullMcp = ref(false)
const reasoningBodyElement = ref<HTMLElement | null>(null)
const skillBodyElement = ref<HTMLElement | null>(null)
const mcpOutputElement = ref<HTMLElement | null>(null)
const MCP_OUTPUT_PREVIEW_LIMIT = 16_000

const mcpCopyStatus = ref<'idle' | 'copied' | 'failed'>('idle')
let mcpCopyGeneration = 0

function handleReasoningToggle(event: Event) {
  reasoningOpen.value = (event.currentTarget as HTMLDetailsElement).open
}

function handleSemanticToggle(event: Event) {
  semanticOpen.value = (event.currentTarget as HTMLDetailsElement).open
}

function focusDisclosureBody(event: KeyboardEvent, target: HTMLElement | null) {
  if (event.key !== 'Tab' || event.shiftKey || !target) return
  event.preventDefault()
  target.focus()
}

const activityPhase = computed(() => props.entry.kind.type === 'activity'
  ? String(props.entry.kind.activity.phase ?? 'active').replaceAll('"', '')
  : '')

const latestReasoningEntry = (entries: typeof remote.entries) => {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index]
    if (entry?.kind.type === 'reasoning') return entry
  }
  return null
}

const isLatestReasoning = computed(() => {
  if (props.entry.kind.type !== 'reasoning') return false
  return latestReasoningEntry(remote.entries)?.id === props.entry.id
})

function plainPreview(value: string): string {
  return value
    .replace(/```[\s\S]*?```/g, ' code ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/[*_~#>|]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

const reasoningPreview = computed(() => {
  if (props.entry.kind.type !== 'reasoning') return ''
  return plainPreview(props.entry.kind.summary || props.entry.kind.content || '')
})

const semanticPreview = computed(() => {
  if (props.entry.kind.type === 'skill' || props.entry.kind.type === 'mcp') {
    return plainPreview(props.entry.kind.content)
  }
  return ''
})

const mcpOutputIsTruncated = computed(() => props.entry.kind.type === 'mcp'
  && props.entry.kind.content.length > MCP_OUTPUT_PREVIEW_LIMIT)

const renderedMcpOutput = computed(() => {
  if (props.entry.kind.type !== 'mcp') return ''
  const content = props.entry.kind.content
  if (!mcpOutputIsTruncated.value || showFullMcp.value) return content
  return `${content.slice(0, MCP_OUTPUT_PREVIEW_LIMIT)}\n\n… ${content.length - MCP_OUTPUT_PREVIEW_LIMIT} more characters hidden`
})

async function copyMcpOutput() {
  if (props.entry.kind.type !== 'mcp') return
  const generation = ++mcpCopyGeneration
  const copied = await copyTextToClipboard(props.entry.kind.content)
  if (props.entry.kind.type !== 'mcp' || generation !== mcpCopyGeneration) return
  mcpCopyStatus.value = copied ? 'copied' : 'failed'
  window.setTimeout(() => {
    if (generation === mcpCopyGeneration) mcpCopyStatus.value = 'idle'
  }, 1500)
}

const mcpCopyButtonText = computed(() => {
  if (mcpCopyStatus.value === 'copied') return 'Copied'
  if (mcpCopyStatus.value === 'failed') return 'Copy failed'
  return mcpOutputIsTruncated.value ? 'Copy full output' : 'Copy output'
})

const mcpCopyAnnouncement = computed(() => {
  if (mcpCopyStatus.value === 'idle' || props.entry.kind.type !== 'mcp') return ''
  const target = props.entry.kind.isError ? 'MCP error' : 'MCP output'
  return mcpCopyStatus.value === 'copied' ? `${target} copied` : `${target} could not be copied`
})
</script>

<template>
  <article class="conversation-entry" :class="`entry-${entry.kind.type}`" :data-entry-id="entry.id">
    <template v-if="entry.kind.type === 'user'">
      <div class="user-message"><MarkdownDocument :content="entry.kind.content" /></div>
    </template>

    <template v-else-if="entry.kind.type === 'assistant'">
      <div class="assistant-row">
        <div class="assistant-avatar"><YeetMark :size="22" /></div>
        <div class="assistant-content">
          <div class="assistant-message">
            <MarkdownDocument :content="entry.kind.content" />
            <span v-if="entry.uiStreaming" class="stream-caret" aria-label="Streaming"></span>
          </div>
          <ActivityGroup v-if="entry.kind.toolCalls?.length" :tools="entry.kind.toolCalls" />
          <div v-if="entry.kind.content && !entry.uiStreaming" class="message-actions">
            <button type="button" aria-label="Copy response" @click="copyMessage">
              {{ messageCopyStatus === 'copied' ? 'Copied' : messageCopyStatus === 'failed' ? 'Copy failed' : 'Copy response' }}
            </button>
            <span class="sr-only" role="status">{{ messageCopyStatus === 'copied' ? 'Response copied' : messageCopyStatus === 'failed' ? 'Response could not be copied' : '' }}</span>
          </div>
        </div>
      </div>
    </template>

    <template v-else-if="entry.kind.type === 'reasoning'">
      <details
        class="reasoning-card"
        :class="{ 'is-latest': isLatestReasoning, 'is-streaming': entry.uiStreaming }"
        :data-latest-reasoning="isLatestReasoning ? 'true' : undefined"
        @toggle="handleReasoningToggle"
      >
        <summary @keydown="focusDisclosureBody($event, reasoningBodyElement)">
          <span class="reasoning-heading">
            <strong>{{ entry.uiStreaming ? 'Thinking' : 'Thoughts' }}</strong>
            <span v-if="reasoningPreview" class="truncate" :title="reasoningPreview">{{ reasoningPreview }}</span>
          </span>
          <span v-if="entry.uiStreaming" class="spinner"></span>
          <span class="reasoning-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div ref="reasoningBodyElement" v-if="reasoningOpen" class="reasoning-body" tabindex="0" role="group" aria-label="Reasoning details"><MarkdownDocument :content="entry.kind.content || entry.kind.summary || ''" /></div>
      </details>
    </template>

    <template v-else-if="entry.kind.type === 'activity'">
      <div class="activity-row" :class="`activity-${activityPhase}`">
        <span class="activity-pulse" :class="{ live: !['done', 'failed', 'interrupted', 'finishing'].includes(activityPhase) }"></span>
        <strong>{{ entry.kind.activity.title }}</strong>
        <span v-if="entry.kind.activity.detail" class="activity-detail truncate">{{ entry.kind.activity.detail }}</span>
      </div>
    </template>

    <ToolCallCard v-else-if="entry.kind.type === 'toolCall'" :tool="entry.kind.toolCall" />

    <template v-else-if="entry.kind.type === 'skill'">
      <details class="semantic-card skill-card" @toggle="handleSemanticToggle">
        <summary @keydown="focusDisclosureBody($event, skillBodyElement)">
          <span class="semantic-symbol" aria-hidden="true">◆</span>
          <span class="semantic-heading">
            <strong>Skill · {{ entry.kind.name }}</strong>
            <span v-if="semanticPreview" class="truncate" :title="semanticPreview">{{ semanticPreview }}</span>
          </span>
          <em class="semantic-status">{{ entry.kind.status || 'loaded' }}</em>
          <span class="semantic-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div ref="skillBodyElement" v-if="semanticOpen" class="semantic-card-body" tabindex="0" role="group" :aria-label="`Skill ${entry.kind.name} output`">
          <MarkdownDocument v-if="entry.kind.content" :content="entry.kind.content" />
          <p v-else class="semantic-empty-output">No additional skill details were provided.</p>
        </div>
      </details>
    </template>

    <template v-else-if="entry.kind.type === 'mcp'">
      <details class="semantic-card mcp-card" :class="{ 'is-error': entry.kind.isError }" @toggle="handleSemanticToggle">
        <summary @keydown="focusDisclosureBody($event, mcpOutputElement)">
          <span class="semantic-symbol" aria-hidden="true">↔</span>
          <span class="semantic-heading">
            <strong>MCP · {{ entry.kind.server }}</strong>
            <span class="truncate" :title="entry.kind.name">{{ entry.kind.name }}<template v-if="semanticPreview"> · {{ semanticPreview }}</template></span>
          </span>
          <em class="semantic-status" :class="{ 'is-error': entry.kind.isError }">{{ entry.kind.isError ? 'Failed' : 'Complete' }}</em>
          <span class="semantic-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div v-if="semanticOpen" class="semantic-output-wrap">
          <pre v-if="entry.kind.content" ref="mcpOutputElement" class="semantic-output" tabindex="0" role="group" :aria-label="entry.kind.isError ? 'MCP error output' : 'MCP output'">{{ renderedMcpOutput }}</pre>
          <p
            v-else
            ref="mcpOutputElement"
            class="semantic-output semantic-empty-output"
            tabindex="0"
            role="group"
            :aria-label="entry.kind.isError ? 'MCP error details' : 'MCP output'"
          >{{ entry.kind.isError ? 'This MCP call failed without error details.' : 'This MCP call completed without output.' }}</p>

          <button v-if="entry.kind.content" class="semantic-output-toggle" type="button" :aria-label="entry.kind.isError ? 'Copy MCP error' : 'Copy MCP output'" @click="copyMcpOutput">{{ mcpCopyButtonText }}</button>
          <button v-if="mcpOutputIsTruncated" class="semantic-output-toggle" type="button" :aria-expanded="showFullMcp" @click="showFullMcp = !showFullMcp">{{ showFullMcp ? 'Collapse output' : `Show full output (${entry.kind.content.length.toLocaleString()} chars)` }}</button>

          <span class="sr-only" role="status" aria-live="polite" aria-atomic="true">{{ mcpCopyAnnouncement }}</span>
        </div>
      </details>
    </template>

    <template v-else-if="entry.kind.type === 'system' || entry.kind.type === 'error'">
      <div class="system-message" :class="{ 'is-error': entry.kind.type === 'error' }">
        <span>{{ entry.kind.type === 'error' ? '!' : 'i' }}</span>
        <MarkdownDocument :content="entry.kind.content" />
      </div>
    </template>
  </article>
</template>
