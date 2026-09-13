<script setup lang="ts">
import { computed } from 'vue'
import { useRemoteStore } from '@/stores/remote'

const remote = useRemoteStore()
const permission = computed(() => remote.pendingPermission)
const title = computed(() => remote.state.pending_shell_permission ? 'Shell permission requested' : 'Native app permission requested')
const detail = computed(() => {
  const item = permission.value
  if (!item) return ''
  if ('command' in item) return item.command
  return `${item.appName || item.server} · ${item.tool}`
})
</script>

<template>
  <div v-if="permission" class="permission-prompt" data-testid="permission-prompt" role="alertdialog" aria-live="assertive">
    <div class="permission-icon">!</div>
    <div class="permission-copy">
      <strong>{{ title }}</strong>
      <span>{{ permission.reason }}</span>
      <code>{{ detail }}</code>
    </div>
    <div class="permission-actions">
      <button class="secondary-button" @click="remote.denyPermission">Deny</button>
      <button class="primary-button" @click="remote.allowPermission">Allow</button>
    </div>
  </div>
</template>
