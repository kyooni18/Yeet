<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ConversationToolCall } from '@/remote/protocol'

const props = defineProps<{ tool: ConversationToolCall }>()
const expanded = ref(false)
const showFullResult = ref(false)
const RESULT_PREVIEW_LIMIT = 12_000
const copiedPart = ref<'arguments' | 'result' | null>(null)
const DETAIL_KEYS = ['path', 'query', 'command', 'url', 'capability', 'server'] as const

function looseArgument(key: string): string | null {
  const raw = props.tool.arguments || ''
  try {
    const args = JSON.parse(raw) as Record<string, unknown>
    if (typeof args[key] === 'string' && args[key]) return String(args[key])
  } catch {
    const match = raw.match(new RegExp(`"${key}"\\s*:\\s*"([^"\\n\\r]*)`))
    if (match?.[1]) return match[1].replaceAll('\\\\"', '"').replaceAll('\\\\n', ' ')
  }
  return null
}

const detail = computed(() => {
  if (props.tool.detail) return props.tool.detail
  for (const key of DETAIL_KEYS) {
    const value = looseArgument(key)
    if (value) return value
  }
  const raw = props.tool.arguments?.trim()
  if (raw && raw.length < 80 && !raw.startsWith('{')) return raw
  return null
})

const prettyArguments = computed(() => {
  try { return JSON.stringify(JSON.parse(props.tool.arguments || '{}'), null, 2) }
  catch { return props.tool.arguments || '{}' }
})

const resultText = computed(() => {
  if (props.tool.error) return props.tool.error
  if (typeof props.tool.result === 'string') return props.tool.result
  if (props.tool.result == null) return ''
  try { return JSON.stringify(props.tool.result, null, 2) }
  catch { return String(props.tool.result) }
})

async function copyText(value: string, part: 'arguments' | 'result') {
  try {
    await navigator.clipboard.writeText(value)
    copiedPart.value = part
    window.setTimeout(() => {
      if (copiedPart.value === part) copiedPart.value = null
    }, 1200)
  } catch {
    copiedPart.value = null
  }
}

const resultIsTruncated = computed(() => resultText.value.length > RESULT_PREVIEW_LIMIT)
const renderedResult = computed(() => {
  if (!resultIsTruncated.value || showFullResult.value) return resultText.value
  return `${resultText.value.slice(0, RESULT_PREVIEW_LIMIT)}\n\n… ${resultText.value.length - RESULT_PREVIEW_LIMIT} more characters hidden`
})

const duration = computed(() => {
  const ms = props.tool.durationMs
  if (ms == null) return ''
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`
})

const statusLabel = computed(() => {
  const timing = duration.value ? ` · ${duration.value}` : ''
  if (props.tool.status === 'failed') return `Failed${timing}`
  if (props.tool.status === 'suppressed') return `Suppressed${timing}`
  if (props.tool.status === 'streaming') return `Running${timing}`
  return `Done${timing}`
})
</script>

<template>
  <article class="tool-card" :class="`tool-${tool.status}`" data-testid="tool-card">
    <button
      class="tool-card-summary"
      type="button"
      :aria-expanded="expanded"
      data-tool-toggle
      @click="expanded = !expanded"
    >
      <span class="tool-status-icon" aria-hidden="true">
        <span v-if="tool.status === 'streaming'" class="spinner"></span>
        <span v-else-if="tool.status === 'completed'">✓</span>
        <span v-else-if="tool.status === 'suppressed'">⊘</span>
        <span v-else>!</span>
      </span>
      <span class="tool-heading">
        <strong>{{ tool.name }}</strong>
        <span v-if="detail" class="truncate" :title="detail">{{ detail }}</span>
      </span>
      <span v-if="statusLabel" class="tool-duration" :class="{ 'is-error': tool.status === 'failed' }">{{ statusLabel }}</span>
      <span class="disclosure" :class="{ open: expanded }" aria-hidden="true">⌄</span>
    </button>

    <div v-if="expanded" class="tool-card-details" data-testid="tool-details">
      <section>
        <div class="tool-detail-heading">
          <span>Arguments</span>
          <span class="tool-detail-actions">
            <span>{{ tool.status }}</span>
            <button type="button" class="tool-copy-button" data-copy-tool="arguments" aria-label="Copy tool arguments" @click="copyText(prettyArguments, 'arguments')">{{ copiedPart === 'arguments' ? 'Copied' : 'Copy' }}</button>
          </span>
        </div>
        <pre>{{ prettyArguments }}</pre>
      </section>
      <section v-if="renderedResult">
        <div class="tool-detail-heading">
          <span>{{ tool.error ? 'Error' : 'Result' }}</span>
          <span class="tool-detail-actions">
            <button type="button" class="tool-copy-button" data-copy-tool="result" :aria-label="tool.error ? 'Copy tool error' : 'Copy tool result'" @click="copyText(resultText, 'result')">{{ copiedPart === 'result' ? 'Copied' : 'Copy' }}</button>
          </span>
        </div>
        <pre :class="{ 'tool-error-output': tool.error }">{{ renderedResult }}</pre>
        <button
          v-if="resultIsTruncated"
          class="tool-output-toggle"
          type="button"
          @click="showFullResult = !showFullResult"
        >{{ showFullResult ? 'Collapse output' : `Show full output (${resultText.length.toLocaleString()} chars)` }}</button>
      </section>
      <p v-else class="tool-no-result">No result payload was included in this update.</p>
    </div>
  </article>
</template>
