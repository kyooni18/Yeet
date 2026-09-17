<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const permission = computed(() => remote.pendingPermission)
const canRespond = computed(() => remote.connection === 'connected')
const describedBy = computed(() => canRespond.value
  ? 'permission-reason permission-detail'
  : 'permission-reason permission-detail permission-connection-status')
const prompt = ref<HTMLElement | null>(null)
const denyButton = ref<HTMLButtonElement | null>(null)
const allowButton = ref<HTMLButtonElement | null>(null)
let returnFocus: HTMLElement | null = null

const title = computed(() => remote.state.pending_shell_permission ? 'Shell permission requested' : 'Native app permission requested')
const detail = computed(() => {
  const item = permission.value
  if (!item) return ''
  if ('command' in item) return item.command
  return `${item.appName || item.server} · ${item.tool}`
})

function handlePromptKeydown(event: KeyboardEvent) {
  if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey) return
  if (!event.shiftKey && event.target === denyButton.value) {
    event.preventDefault()
    allowButton.value?.focus({ preventScroll: true })
  } else if (event.shiftKey && event.target === allowButton.value) {
    event.preventDefault()
    denyButton.value?.focus({ preventScroll: true })
  }
}

watch(permission, (current, previous) => {
  if (current) {
    if (!previous) {
      const active = document.activeElement
      returnFocus = active instanceof HTMLElement && active !== document.body ? active : null
    }
    void nextTick(() => denyButton.value?.focus({ preventScroll: true }))
    return
  }

  if (!previous) return
  const active = document.activeElement
  const shouldRestore = !!prompt.value && active instanceof Node && prompt.value.contains(active)
  const target = returnFocus
  returnFocus = null
  if (shouldRestore && target?.isConnected) {
    void nextTick(() => target.focus({ preventScroll: true }))
  }
})

watch(canRespond, (enabled) => {
  if (!permission.value) return
  const root = prompt.value
  const focusWasInside = !!root && document.activeElement instanceof Node && root.contains(document.activeElement)
  void nextTick(() => {
    const currentRoot = prompt.value
    if (!currentRoot) return
    if (!enabled && focusWasInside) {
      currentRoot.focus({ preventScroll: true })
    } else if (enabled && document.activeElement === currentRoot) {
      denyButton.value?.focus({ preventScroll: true })
    }
  })
})
</script>

<template>
  <div
    v-if="permission"
    ref="prompt"
    class="permission-prompt"
    data-testid="permission-prompt"
    role="alertdialog"
    aria-live="assertive"
    aria-labelledby="permission-title"
    :aria-describedby="describedBy"
    tabindex="-1"
    @keydown="handlePromptKeydown"
  >
    <div class="permission-icon" aria-hidden="true">!</div>
    <div class="permission-copy">
      <strong id="permission-title">{{ title }}</strong>
      <span id="permission-reason">{{ permission.reason }}</span>
      <code id="permission-detail" tabindex="0" aria-label="Requested command or tool">{{ detail }}</code>
      <span v-if="!canRespond" id="permission-connection-status">Remote is disconnected. Reconnect to respond.</span>
    </div>
    <div class="permission-actions">
      <button ref="denyButton" type="button" class="secondary-button" :disabled="!canRespond" @click="remote.denyPermission">Deny</button>
      <button ref="allowButton" type="button" class="primary-button" :disabled="!canRespond" @click="remote.allowPermission">Allow</button>
    </div>
  </div>
</template>


<style scoped>
@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .permission-actions button {
    min-width: 78px;
    min-height: 44px;
  }
}
@media (max-width: 899px) {
  .permission-copy code {
    max-height: min(24dvh, 128px);
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-right: 4px;
  }
  .permission-copy code:focus-visible {
    outline: 2px solid var(--brand);
    outline-offset: 2px;
  }
}
</style>
