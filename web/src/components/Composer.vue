<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { readComposerDraft, writeComposerDraft } from '@/composables/composerDrafts'
import { useMediaQuery } from '@/composables/useMediaQuery'
import { useRemoteStore } from '@/stores/remote'
import PermissionPrompt from './PermissionPrompt.vue'

const remote = useRemoteStore()
const touchFirst = useMediaQuery('(hover: none) and (pointer: coarse)')
const draft = ref('')
const textarea = ref<HTMLTextAreaElement | null>(null)
const composerZone = ref<HTMLElement | null>(null)
const composing = ref(false)
let composerHost: HTMLElement | null = null
let composerResizeObserver: ResizeObserver | null = null
const draftContextKey = computed(() => JSON.stringify([
  remote.state.workspace_root ?? '',
  remote.state.current_session_id ?? null,
  remote.state.current_session_id == null ? remote.sessionResetRevision : 0,
]))

const attachedSkillCount = computed(() => remote.state.available_capabilities.filter((capability) => capability.kind === 'skill' && capability.enabled).length)

const controlSummary = computed(() => {
  const model = remote.state.active_model || 'no model selected'
  const reasoning = remote.state.active_reasoning_level || 'auto'
  const skills = attachedSkillCount.value ? `, ${attachedSkillCount.value} attached ${attachedSkillCount.value === 1 ? 'skill' : 'skills'}` : ''
  return `Session controls: ${model}, ${reasoning} reasoning, ${remote.permissionMode} permissions${skills}`
})

function resize() {
  const element = textarea.value
  if (!element) return
  if (!element.value) {
    element.style.height = ''
    return
  }
  element.style.height = '0px'
  element.style.height = `${Math.min(element.scrollHeight, 180)}px`
}

function updateOverlayClearance() {
  const zone = composerZone.value
  if (!zone) return
  composerHost ??= zone.closest<HTMLElement>('.conversation-pane')
  if (!composerHost) return

  const rect = zone.getBoundingClientRect()
  const visualViewport = window.visualViewport
  const viewportBottom = (visualViewport?.offsetTop ?? 0) + (visualViewport?.height ?? window.innerHeight)
  composerHost.style.setProperty('--composer-overlay-clearance', `${Math.max(0, viewportBottom - rect.top)}px`)
}

onMounted(() => {
  const zone = composerZone.value
  if (!zone) return
  composerHost = zone.closest<HTMLElement>('.conversation-pane')
  composerResizeObserver = new ResizeObserver(updateOverlayClearance)
  composerResizeObserver.observe(zone)
  window.addEventListener('resize', updateOverlayClearance)
  window.visualViewport?.addEventListener('resize', updateOverlayClearance)
  window.visualViewport?.addEventListener('scroll', updateOverlayClearance)
  updateOverlayClearance()
})

onBeforeUnmount(() => {
  composerResizeObserver?.disconnect()
  window.removeEventListener('resize', updateOverlayClearance)
  window.visualViewport?.removeEventListener('resize', updateOverlayClearance)
  window.visualViewport?.removeEventListener('scroll', updateOverlayClearance)
  composerHost?.style.removeProperty('--composer-overlay-clearance')
})
watch(draft, (value) => {
  writeComposerDraft(draftContextKey.value, value)
  void nextTick(resize)
}, { flush: 'sync' })

watch(draftContextKey, (key) => {
  draft.value = readComposerDraft(key)
  void nextTick(resize)
}, { flush: 'sync', immediate: true })

function submit() {
  const text = draft.value.trim()
  if (!text || remote.state.is_streaming || remote.connection !== 'connected') return
  if (remote.submit(text)) {
    draft.value = ''
    void nextTick(resize)
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key !== 'Enter' || event.shiftKey || composing.value || draft.value.includes('\n')) return
  if (event.isComposing) return
  if (touchFirst.value && !event.metaKey && !event.ctrlKey) return
  event.preventDefault()
  submit()
}
</script>

<template>
  <div ref="composerZone" class="composer-zone">
    <div class="composer-notices">
      <PermissionPrompt />
      <div v-if="remote.state.error_message" class="inline-error" role="alert" tabindex="0">
        <span>{{ remote.state.error_message }}</span>
      </div>
      <div
        v-if="remote.connection !== 'connected' && remote.connection !== 'auth-required'"
        class="connection-banner"
        :role="remote.connection === 'failed' ? 'alert' : 'status'"
        :tabindex="remote.connectionError ? 0 : undefined"
        aria-live="polite"
      >
        <span class="status-dot" :class="`status-${remote.connection}`"></span>
        <span>
          {{ remote.connection === 'offline' ? 'Offline' : remote.connection === 'failed' ? 'Connection failed' : 'Reconnecting to Yeet…' }}
          <template v-if="remote.connectionError"> — {{ remote.connectionError }}</template>
        </span>
      </div>
    </div>

    <div class="composer" data-testid="composer">
      <textarea
        ref="textarea"
        v-model="draft"
        rows="1"
        aria-label="Message Yeet"
        placeholder="Ask Yeet to investigate, build, edit, or run something…"
        :enterkeyhint="touchFirst ? 'enter' : 'send'"
        :disabled="remote.connection === 'auth-required'"
        @keydown="onKeydown"
        @compositionstart="composing = true"
        @compositionend="composing = false"
      ></textarea>

      <div class="composer-toolbar">
        <div class="composer-tools">
          <button
            class="composer-context"
            :aria-label="controlSummary"
            :title="controlSummary"
            @click="remote.mobileStatusOpen = true"
          >
            <span>{{ remote.state.active_reasoning_level || 'auto' }}</span>
            <template v-if="attachedSkillCount">
              <span>·</span>
              <span class="composer-skill-count">{{ attachedSkillCount }} {{ attachedSkillCount === 1 ? 'skill' : 'skills' }}</span>
            </template>
            <span class="composer-permission">· {{ remote.permissionMode }}</span>
          </button>
        </div>
        <button
          v-if="remote.state.is_streaming"
          class="stop-button"
          data-testid="interrupt"
          aria-label="Stop current task"
          @click="remote.interrupt"
        ><span></span></button>
        <button
          v-else
          class="send-button"
          data-testid="submit"
          aria-label="Send message"
          :title="touchFirst ? 'Send message' : 'Send (Enter)'"
          :disabled="!draft.trim() || remote.connection !== 'connected'"
          @click="submit"
        >↑</button>
      </div>
    </div>
  </div>
</template>


<style scoped>
.composer textarea::selection,
.composer textarea::-moz-selection {
  background: transparent;
  color: inherit;
}

@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .composer textarea {
    min-height: 44px;
    font-size: 16px;
  }
  .composer-toolbar { min-height: 44px; }
  .composer-context { min-height: 44px; }
  .send-button,
  .stop-button {
    width: 44px;
    height: 44px;
  }
}
</style>
