<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ConversationEntry } from '@/remote/protocol'
import YeetMark from '@/components/YeetMark.vue'
import MarkdownDocument from './MarkdownDocument.vue'
import ToolCallCard from './ToolCallCard.vue'

const props = defineProps<{ entry: ConversationEntry }>()

const reasoningOpen = ref(false)
const semanticOpen = ref(false)
const showFullMcp = ref(false)
const MCP_OUTPUT_PREVIEW_LIMIT = 16_000

function handleReasoningToggle(event: Event) {
  reasoningOpen.value = (event.currentTarget as HTMLDetailsElement).open
}

function handleSemanticToggle(event: Event) {
  semanticOpen.value = (event.currentTarget as HTMLDetailsElement).open
}

const activityPhase = computed(() => props.entry.kind.type === 'activity'
  ? String(props.entry.kind.activity.phase ?? 'active').replaceAll('"', '')
  : '')

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
          <ToolCallCard v-for="tool in entry.kind.toolCalls || []" :key="tool.id" :tool="tool" />
        </div>
      </div>
    </template>

    <template v-else-if="entry.kind.type === 'reasoning'">
      <details class="reasoning-card" @toggle="handleReasoningToggle">
        <summary>
          <span class="reasoning-symbol" aria-hidden="true">◇</span>
          <span class="reasoning-heading">
            <strong>{{ entry.uiStreaming ? 'Reasoning' : 'Reasoning trace' }}</strong>
            <span v-if="reasoningPreview" class="truncate" :title="reasoningPreview">{{ reasoningPreview }}</span>
          </span>
          <span v-if="entry.uiStreaming" class="spinner"></span>
          <span class="reasoning-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div v-if="reasoningOpen" class="reasoning-body"><MarkdownDocument :content="entry.kind.content || entry.kind.summary || ''" /></div>
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
        <summary>
          <span class="semantic-symbol" aria-hidden="true">◆</span>
          <span class="semantic-heading">
            <strong>Skill · {{ entry.kind.name }}</strong>
            <span v-if="semanticPreview" class="truncate" :title="semanticPreview">{{ semanticPreview }}</span>
          </span>
          <em class="semantic-status">{{ entry.kind.status || 'loaded' }}</em>
          <span class="semantic-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div v-if="semanticOpen" class="semantic-card-body"><MarkdownDocument :content="entry.kind.content" /></div>
      </details>
    </template>

    <template v-else-if="entry.kind.type === 'mcp'">
      <details class="semantic-card mcp-card" :class="{ 'is-error': entry.kind.isError }" @toggle="handleSemanticToggle">
        <summary>
          <span class="semantic-symbol" aria-hidden="true">↔</span>
          <span class="semantic-heading">
            <strong>MCP · {{ entry.kind.server }}</strong>
            <span class="truncate" :title="entry.kind.name">{{ entry.kind.name }}<template v-if="semanticPreview"> · {{ semanticPreview }}</template></span>
          </span>
          <em class="semantic-status" :class="{ 'is-error': entry.kind.isError }">{{ entry.kind.isError ? 'Failed' : 'Complete' }}</em>
          <span class="semantic-disclosure" aria-hidden="true">⌄</span>
        </summary>
        <div v-if="semanticOpen" class="semantic-output-wrap">
          <pre class="semantic-output">{{ renderedMcpOutput }}</pre>
          <button v-if="mcpOutputIsTruncated" class="semantic-output-toggle" type="button" @click="showFullMcp = !showFullMcp">{{ showFullMcp ? 'Collapse output' : `Show full output (${entry.kind.content.length.toLocaleString()} chars)` }}</button>
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
