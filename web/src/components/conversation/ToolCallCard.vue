<script setup lang="ts">
import { computed, inject, ref, type ComputedRef } from 'vue'
import type { ConversationToolCall } from '@/remote/protocol'
import { copyTextToClipboard } from '@/utils/clipboard'
import { toolPurpose } from '@/utils/toolPurpose'

const props = defineProps<{ tool: ConversationToolCall }>()
const promotedPurpose = inject<ComputedRef<string>>('promotedToolPurpose', computed(() => ''))
const expanded = ref(false)
const showFullResult = ref(false)
const argumentsCopyButton = ref<HTMLButtonElement | null>(null)
const resultCopyButton = ref<HTMLButtonElement | null>(null)
const RESULT_PREVIEW_LIMIT = 12_000
type CopyPart = 'arguments' | 'result'
type CopyStatus = 'copied' | 'failed'
const copyFeedback = ref<{ part: CopyPart; status: CopyStatus; generation: number } | null>(null)
let copyFeedbackGeneration = 0

function focusElement(target: HTMLElement | null) {
  target?.focus()
}
const DETAIL_KEYS = ['path', 'query', 'command', 'url', 'capability', 'server'] as const

const toolName = computed(() => props.tool.name?.trim() || 'Tool call')

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
  // Intent is the summary; raw arguments remain available in the expanded card.
  const purpose = toolPurpose(props.tool.arguments)
  if (purpose) return purpose === promotedPurpose.value ? null : purpose
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

async function copyText(value: string, part: CopyPart) {
  const generation = ++copyFeedbackGeneration
  const copied = await copyTextToClipboard(value)
  copyFeedback.value = { part, status: copied ? 'copied' : 'failed', generation }
  window.setTimeout(() => {
    if (copyFeedback.value?.generation === generation) copyFeedback.value = null
  }, 1500)
}

function copyButtonText(part: CopyPart): string {
  if (copyFeedback.value?.part !== part) return 'Copy'
  return copyFeedback.value.status === 'copied' ? 'Copied' : 'Copy failed'
}

const resultNoun = computed(() => {
  if (props.tool.error) return 'error'
  if (props.tool.status === 'suppressed') return 'suppression reason'
  return 'result'
})
const resultHeading = computed(() => resultNoun.value === 'suppression reason' ? 'Reason' : resultNoun.value[0].toUpperCase() + resultNoun.value.slice(1))
const resultCopyLabel = computed(() => resultNoun.value === 'suppression reason' ? 'Copy suppression reason' : `Copy tool ${resultNoun.value}`)
const resultOutputLabel = computed(() => `Tool ${resultNoun.value}${props.tool.error ? ' output' : ''}`)

const copyAnnouncement = computed(() => {
  const feedback = copyFeedback.value
  if (!feedback) return ''
  const target = feedback.part === 'arguments' ? 'Tool arguments' : `Tool ${resultNoun.value}`
  return feedback.status === 'copied' ? `${target} copied` : `${target} could not be copied`
})

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
  if (props.tool.status === 'running') return `Running${timing}`
  return duration.value
})


const noResultMessage = computed(() => {
  if (props.tool.status === 'running') return 'Waiting for tool output…'
  if (props.tool.status === 'failed') return 'This tool failed without error details.'
  if (props.tool.status === 'suppressed') return 'This tool was suppressed without a reason.'
  return 'This tool completed without output.'
})

const summaryLabel = computed(() => [toolName.value, detail.value, props.tool.status, duration.value].filter(Boolean).join(' · '))
</script>

<template>
  <article class="tool-card" :class="`tool-${tool.status}`" data-testid="tool-card">
    <button
      class="tool-card-summary"
      type="button"
      :aria-expanded="expanded"
      :aria-label="summaryLabel"
      data-tool-toggle
      @click="expanded = !expanded"
    >
      <span class="tool-status-icon" aria-hidden="true">
        <span v-if="tool.status === 'running'" class="spinner"></span>
        <span v-else-if="tool.status === 'completed'">✓</span>
        <span v-else-if="tool.status === 'suppressed'">⊘</span>
        <span v-else>!</span>
      </span>
      <span class="tool-heading">
        <span class="tool-purpose truncate" :title="detail || toolName">{{ detail || toolName.replaceAll('_', ' ') }}</span>
      </span>
      <span v-if="statusLabel" class="tool-duration" :class="{ 'is-error': tool.status === 'failed' }">{{ statusLabel }}</span>
      <span class="disclosure" :class="{ open: expanded }" aria-hidden="true">⌄</span>
    </button>

    <div v-if="expanded" class="tool-card-details" data-testid="tool-details">
      <section>
        <div class="tool-detail-heading">
          <span>{{ toolName }} · Arguments</span>
          <span class="tool-detail-actions">
            <span>{{ tool.status }}</span>
            <button ref="argumentsCopyButton" type="button" class="tool-copy-button" data-copy-tool="arguments" aria-label="Copy tool arguments" @click="copyText(prettyArguments, 'arguments')">{{ copyButtonText('arguments') }}</button>
          </span>
        </div>
        <pre tabindex="0" role="group" aria-label="Tool arguments" @keydown.shift.tab.prevent="focusElement(argumentsCopyButton)">{{ prettyArguments }}</pre>
      </section>
      <section v-if="renderedResult">
        <div class="tool-detail-heading">
          <span>{{ resultHeading }}</span>
          <span class="tool-detail-actions">
            <button ref="resultCopyButton" type="button" class="tool-copy-button" data-copy-tool="result" :aria-label="resultCopyLabel" @click="copyText(resultText, 'result')">{{ copyButtonText('result') }}</button>
          </span>
        </div>
        <pre tabindex="0" role="group" :aria-label="resultOutputLabel" :class="{ 'tool-error-output': tool.error }" @keydown.shift.tab.prevent="focusElement(resultCopyButton)">{{ renderedResult }}</pre>
        <button
          v-if="resultIsTruncated"
          class="tool-output-toggle"
          type="button"
          :aria-expanded="showFullResult"
          @click="showFullResult = !showFullResult"
        >{{ showFullResult ? 'Collapse output' : `Show full output (${resultText.length.toLocaleString()} chars)` }}</button>
      </section>
      <p v-else class="tool-no-result" role="status" aria-live="polite">{{ noResultMessage }}</p>
      <span class="sr-only" role="status" aria-live="polite" aria-atomic="true">{{ copyAnnouncement }}</span>
    </div>
  </article>
</template>
