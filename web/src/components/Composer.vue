<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useRemoteStore } from '@/stores/remote'
import PermissionPrompt from './PermissionPrompt.vue'

const remote = useRemoteStore()
const draft = ref('')
const textarea = ref<HTMLTextAreaElement | null>(null)
const composing = ref(false)

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
  element.style.height = '0px'
  element.style.height = `${Math.min(element.scrollHeight, 180)}px`
}

watch(draft, () => void nextTick(resize))

function submit() {
  const text = draft.value.trim()
  if (!text || remote.state.is_streaming || remote.connection !== 'connected') return
  if (remote.submit(text)) {
    draft.value = ''
    void nextTick(resize)
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key !== 'Enter' || event.shiftKey || composing.value) return
  if (event.isComposing) return
  event.preventDefault()
  submit()
}
</script>

<template>
  <div class="composer-zone">
    <PermissionPrompt />
    <div v-if="remote.state.error_message" class="inline-error" role="alert">
      <span>{{ remote.state.error_message }}</span>
    </div>
    <div v-if="remote.connection !== 'connected' && remote.connection !== 'auth-required'" class="connection-banner">
      <span class="status-dot" :class="`status-${remote.connection}`"></span>
      {{ remote.connection === 'offline' ? 'Offline' : remote.connection === 'failed' ? 'Connection failed' : 'Reconnecting to Yeet…' }}
    </div>

    <div class="composer" data-testid="composer">
      <textarea
        ref="textarea"
        v-model="draft"
        rows="1"
        aria-label="Message Yeet"
        placeholder="Ask Yeet to investigate, build, edit, or run something…"
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
          title="Send (Enter)"
          :disabled="!draft.trim() || remote.connection !== 'connected'"
          @click="submit"
        >↑</button>
      </div>
    </div>
  </div>
</template>
