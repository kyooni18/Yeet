<script setup lang="ts">
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useRemoteStore } from '@/stores/remote'
import ModelPicker from '@/components/ModelPicker.vue'

const remote = useRemoteStore()
const route = useRoute()
const router = useRouter()
const settingsReadOnly = computed(() => remote.connection !== 'connected')

const sections = [
  ['runtime', 'Runtime'],
  ['providers', 'Providers'],
  ['capabilities', 'Capabilities'],
  ['sandbox', 'Sandbox'],
  ['sessions', 'Sessions'],
] as const

type SettingsSection = (typeof sections)[number][0]

function isSettingsSection(value: unknown): value is SettingsSection {
  return typeof value === 'string' && sections.some(([id]) => id === value)
}

const section = computed<SettingsSection>(() => (
  isSettingsSection(route.params.section) ? route.params.section : 'runtime'
))

watch(() => route.params.section, (value) => {
  if (value == null || isSettingsSection(value)) return
  void router.replace({ name: 'settings', params: { section: 'runtime' }, query: route.query, hash: route.hash })
}, { immediate: true })

const sectionNav = ref<HTMLElement | null>(null)
const pendingProviderRemoval = ref<string | null>(null)
const pendingSandboxReset = ref(false)
const sandboxResetTrigger = ref<HTMLButtonElement | null>(null)
const sandboxResetCancel = ref<HTMLButtonElement | null>(null)

function revealActiveSection() {
  const nav = sectionNav.value
  const active = nav?.querySelector<HTMLElement>('[aria-current="page"]')
  if (!nav || !active || nav.scrollWidth <= nav.clientWidth) return

  const edgePadding = 12
  const left = active.offsetLeft
  const right = left + active.offsetWidth
  const visibleLeft = nav.scrollLeft
  const visibleRight = visibleLeft + nav.clientWidth

  if (left < visibleLeft + edgePadding) {
    nav.scrollTo({ left: Math.max(0, left - edgePadding) })
  } else if (right > visibleRight - edgePadding) {
    nav.scrollTo({ left: right - nav.clientWidth + edgePadding })
  }
}

watch(section, async () => {
  pendingProviderRemoval.value = null
  pendingSandboxReset.value = false
  await nextTick()
  revealActiveSection()
}, { immediate: true })

const reasoningLevels = ['auto', 'low', 'medium', 'high']
const apiKeys = reactive<Record<string, string>>({})
const providerId = ref('')
const providerUrl = ref('')
const providerNeedsKey = ref(true)
const workspacePath = ref('')
const networkHost = ref('')
const networkPort = ref('')
const networkHostError = computed(() => {
  const host = networkHost.value.trim().toLowerCase()
  if (!host) return ''
  const labels = host.split('.')
  const valid = host.length <= 253
    && !host.startsWith('.')
    && !host.endsWith('.')
    && labels.every((label) => label.length <= 63 && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label))
  return valid ? '' : 'Host must use DNS-style labels without leading or trailing dots or hyphens.'
})
const networkPortError = computed(() => {
  const value = networkPort.value.trim()
  if (!value) return ''
  if (!/^\d+$/.test(value)) return 'Port must be a whole number from 1 to 65535.'
  const parsed = Number(value)
  return Number.isInteger(parsed) && parsed >= 1 && parsed <= 65535
    ? ''
    : 'Port must be a whole number from 1 to 65535.'
})
const environmentKey = ref('')
const environmentValue = ref('')
const secretId = ref('')
const workspacePathError = computed(() => {
  const value = workspacePath.value.trim()
  if (!value) return ''
  const components = value.split('/').filter((component) => component && component !== '.')
  const valid = value.length <= 4096
    && !value.includes('\0')
    && !value.includes('\\')
    && !value.startsWith('/')
    && !value.endsWith('/')
    && value !== '.'
    && !value.startsWith('./')
    && components.length > 0
    && !components.some((component) => component === '..')
  return valid ? '' : 'Use a workspace-relative path without parent traversal, a leading slash, or backslashes.'
})
const environmentKeyError = computed(() => {
  const key = environmentKey.value.trim()
  if (!key) return ''
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(key)
    ? ''
    : 'Environment names must start with a letter or underscore and contain only letters, numbers, and underscores.'
})
const environmentValueError = computed(() => environmentValue.value.includes('\0')
  ? 'Environment values cannot contain null characters.'
  : '')
const secretIdError = computed(() => {
  const id = secretId.value.trim()
  if (!id) return ''
  return id.length <= 128 && /^[A-Za-z0-9._-]+$/.test(id)
    ? ''
    : 'Secret IDs may contain only letters, numbers, dots, hyphens, and underscores (128 characters max).'
})

const browserProviders = new Set(['codex-cli', 'gemini-web', 'claude'])
const apiKeyProviders = new Set(['openai', 'anthropic', 'gemini'])

const sandbox = computed(() => remote.state.sandbox_settings)

const primaryCapabilityIds = new Set(['web-search', 'builtin:computer-use', 'builtin:skyline'])
const primaryCapabilities = computed(() => remote.state.available_capabilities.filter((capability) => primaryCapabilityIds.has(capability.id)))
const skillCapabilities = computed(() => remote.state.available_capabilities.filter((capability) => capability.kind === 'skill'))
const mcpCapabilities = computed(() => remote.state.available_capabilities.filter((capability) => capability.kind === 'mcp'))
const advancedCapabilities = computed(() => remote.state.available_capabilities.filter((capability) => (
  !primaryCapabilityIds.has(capability.id) && capability.kind !== 'skill' && capability.kind !== 'mcp'
)))
const enabledCapabilityCount = (capabilities: typeof remote.state.available_capabilities) => capabilities.filter((capability) => capability.enabled).length

const sandboxPolicyFingerprint = computed(() => JSON.stringify(sandbox.value))

watch(sandboxPolicyFingerprint, async (next, previous) => {
  if (!pendingSandboxReset.value || next === previous) return
  pendingSandboxReset.value = false
  await nextTick()
  sandboxResetTrigger.value?.focus({ preventScroll: true })
})


const providerConfigurationsFingerprint = computed(() => JSON.stringify(remote.state.provider_configurations))

watch(providerConfigurationsFingerprint, async (next, previous) => {
  const id = pendingProviderRemoval.value
  if (!id || next === previous) return
  pendingProviderRemoval.value = null
  await focusProviderRemovalControl(id, '.provider-remove-trigger')
})

const activeCatalogModel = computed(() => remote.state.model_catalog.find((item) => item.id === remote.state.active_model) ?? null)
const activeProvider = computed(() => {
  const structured = activeCatalogModel.value?.provider.trim().toLowerCase()
  if (structured) return structured
  const model = remote.state.active_model.trim().toLowerCase()
  const slash = model.indexOf('/')
  if (slash > 0) return model.slice(0, slash)
  return /^(gpt-|o[134]-|chatgpt)/.test(model) ? 'openai' : ''
})
const showOpenAiFlex = computed(() => activeProvider.value === 'openai')
const authProviderStatus = computed(() => remote.state.auth_notice ?? (
  remote.state.auth_working
    ? (remote.state.auth_providers.length ? 'Refreshing authentication providers…' : 'Loading authentication providers…')
    : null
))
const customProviderStatus = computed(() => remote.state.providers_notice ?? (
  remote.state.providers_working
    ? (remote.state.provider_configurations.length ? 'Refreshing custom providers…' : 'Loading custom providers…')
    : null
))
const runtimeSettingsStatus = computed(() => remote.state.settings_notice ?? (remote.state.settings_working ? 'Saving workspace settings…' : null))
const sandboxSettingsStatus = computed(() => remote.state.sandbox_notice ?? (remote.state.sandbox_working ? 'Saving sandbox policy…' : null))

function chooseSection(id: string) {
  void router.replace(`/settings/${id}`)
}


function loadSavedSession(id: string) {
  if (id === remote.state.current_session_id) return
  remote.loadSession(id)
}

function saveApiKey(provider: string) {
  const key = apiKeys[provider]?.trim()
  if (!key) return
  if (remote.send({ type: 'auth_set_api_key', provider, key })) apiKeys[provider] = ''
}

function saveProvider() {
  if (!providerId.value.trim() || !providerUrl.value.trim()) return
  const sent = remote.send({
    type: 'save_provider',
    id: providerId.value.trim(),
    base_url: providerUrl.value.trim(),
    require_api_key: providerNeedsKey.value,
  })
  if (!sent) return
  providerId.value = ''
  providerUrl.value = ''
}

async function focusProviderRemovalControl(id: string, selector: string) {
  await nextTick()
  const actions = Array.from(document.querySelectorAll<HTMLElement>('[data-provider-config-id]'))
    .find((item) => item.dataset.providerConfigId === id)
  actions?.querySelector<HTMLButtonElement>(selector)?.focus({ preventScroll: true })
}

async function armProviderRemoval(id: string) {
  pendingProviderRemoval.value = id
  await focusProviderRemovalControl(id, '.provider-remove-cancel')
}

async function cancelProviderRemoval(id: string) {
  if (pendingProviderRemoval.value !== id) return
  pendingProviderRemoval.value = null
  await focusProviderRemovalControl(id, '.provider-remove-trigger')
}

async function finishProviderRemoval(id: string) {
  if (pendingProviderRemoval.value !== id) return
  pendingProviderRemoval.value = null
  await focusProviderRemovalControl(id, '.provider-remove-trigger')
}

async function armSandboxReset() {
  pendingSandboxReset.value = true
  await nextTick()
  sandboxResetCancel.value?.focus({ preventScroll: true })
}

async function cancelSandboxReset() {
  if (!pendingSandboxReset.value) return
  pendingSandboxReset.value = false
  await nextTick()
  sandboxResetTrigger.value?.focus({ preventScroll: true })
}

async function confirmSandboxReset() {
  if (!pendingSandboxReset.value) return
  const sent = remote.updateSandbox({ type: 'reset' })
  if (!sent) return
  pendingSandboxReset.value = false
  await nextTick()
  if (!remote.state.sandbox_working) sandboxResetTrigger.value?.focus({ preventScroll: true })
}

function addWorkspacePath() {
  const path = workspacePath.value.trim()
  if (!path || workspacePathError.value) return
  if (remote.updateSandbox({ type: 'add_workspace_path', path })) workspacePath.value = ''
}

function addNetwork() {
  const host = networkHost.value.trim()
  if (!host || networkHostError.value || networkPortError.value) return
  const rawPort = networkPort.value.trim()
  const port = rawPort ? Number(rawPort) : null
  const sent = remote.updateSandbox({
    type: 'add_network',
    host,
    port,
  })
  if (!sent) return
  networkHost.value = ''
  networkPort.value = ''
}

function setEnvironment() {
  const key = environmentKey.value.trim()
  if (!key || environmentKeyError.value || environmentValueError.value) return
  const sent = remote.updateSandbox({ type: 'set_environment', key, value: environmentValue.value })
  if (!sent) return
  environmentKey.value = ''
  environmentValue.value = ''
}

function addSecret() {
  const id = secretId.value.trim()
  if (!id || secretIdError.value) return
  if (remote.updateSandbox({ type: 'add_secret', id })) secretId.value = ''
}

function setLimit(name: string, value: string) {
  const parsed = Number(value)
  if (Number.isFinite(parsed) && parsed >= 0) remote.updateSandbox({ type: 'set_limit', name, value: Math.floor(parsed) })
}

function usageWidth(value: number) {
  return `${Math.max(0, Math.min(100, value))}%`
}

function providerLabel(provider: string): string {
  switch (provider) {
    case 'openai': return 'OpenAI'
    case 'codex-cli': return 'Codex CLI'
    case 'anthropic': return 'Claude (Anthropic API)'
    case 'claude': return 'Claude Web'
    case 'gemini': return 'Gemini API'
    case 'gemini-web': return 'Gemini Web'
    default: return provider
  }
}

function sessionRelativeTime(value: string): string {
  const time = Date.parse(value)
  if (!Number.isFinite(time)) return 'Unknown'
  const minutes = Math.floor((Date.now() - time) / 60_000)
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  return `${Math.floor(hours / 24)}d`
}

function sessionTimestampTitle(value: string): string {
  const time = Date.parse(value)
  if (!Number.isFinite(time)) return value
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(time))
}

function savedSessionCountLabel(count: number): string {
  return `${count} saved ${count === 1 ? 'session' : 'sessions'}`
}

function messageCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'message' : 'messages'}`
}
</script>

<template>
  <div class="settings-app">
    <header class="settings-topbar">
      <button class="icon-button" type="button" aria-label="Back to conversation" @click="router.replace('/')">←</button>
      <div>
        <span class="settings-kicker">Yeet</span>
        <h1>Settings</h1>
      </div>
      <span class="settings-connection"><span class="status-dot" :class="`status-${remote.connection}`"></span><span>{{ remote.connection }}</span></span>
    </header>

    <div class="settings-layout">
      <nav ref="sectionNav" class="settings-nav" aria-label="Settings sections">
        <button
          v-for="[id, label] in sections"
          :key="id"
          type="button"
          :class="{ active: section === id }"
          :aria-current="section === id ? 'page' : undefined"
          @click="chooseSection(id)"
        >{{ label }}</button>
      </nav>

      <main class="settings-content">
        <p v-if="settingsReadOnly" class="settings-readonly-notice" role="status">Settings controls are read-only until Yeet Remote reconnects.</p>
        <fieldset class="settings-control-scope" :disabled="settingsReadOnly">
        <section v-if="section === 'runtime'" class="settings-section">
          <div class="section-heading">
            <span class="eyebrow">EXECUTION PROFILE</span>
            <h2>Runtime</h2>
            <p>Model, reasoning, memory, provider execution mode, and current context.</p>
          </div>

          <div class="setting-card surface-card">
            <div class="setting-row stacked-mobile runtime-model-row" data-testid="runtime-model-picker">
              <div><strong>Model</strong><span>The model used for new work in this workspace.</span></div>
              <ModelPicker variant="panel" />
            </div>
            <div class="setting-row stacked-mobile">
              <div><strong>Reasoning</strong><span>Controls how much reasoning effort the model may use.</span></div>
              <div class="segmented-control" role="group" aria-label="Reasoning level">
                <button v-for="level in reasoningLevels" :key="level" type="button" :class="{ active: remote.state.active_reasoning_level === level }" :aria-pressed="remote.state.active_reasoning_level === level" @click="remote.selectReasoning(level)">{{ level }}</button>
              </div>
            </div>
            <div class="setting-row">
              <div><strong>Goal</strong><span>Keep starting bounded execution epochs until a strict success judge accepts concrete evidence.</span></div>
              <button class="switch-control" type="button" role="switch" aria-label="Goal" :aria-checked="remote.state.goal_mode" :class="{ on: remote.state.goal_mode }" @click="remote.setGoal(!remote.state.goal_mode)"><span></span></button>
            </div>
            <div class="setting-row">
              <div><strong>Permissions</strong><span>Current execution approval posture for this workspace.</span></div>
              <button class="settings-link-button" type="button" @click="chooseSection('sandbox')">{{ remote.permissionMode }} →</button>
            </div>
            <div class="setting-row">
              <div><strong>Context</strong><span>Current working context usage for the selected model.</span></div>
              <strong class="setting-value">{{ remote.contextPercent == null ? 'Unavailable' : `${remote.contextPercent}%` }}</strong>
            </div>
          </div>
          <div v-if="runtimeSettingsStatus" class="settings-feedback" :class="{ 'is-working': remote.state.settings_working }" role="status" aria-live="polite" data-testid="runtime-settings-status">{{ runtimeSettingsStatus }}</div>

          <div class="setting-card surface-card" :aria-busy="remote.state.settings_working">
            <div v-if="showOpenAiFlex" class="setting-row">
              <div><strong>OpenAI Flex</strong><span>Use Flex processing when supported by the active OpenAI provider.</span></div>
              <button class="switch-control" type="button" role="switch" aria-label="OpenAI Flex" :disabled="remote.state.settings_working" :aria-checked="remote.state.openai_flex" :class="{ on: remote.state.openai_flex }" @click="remote.setOpenAiFlex(!remote.state.openai_flex)"><span></span></button>
            </div>
            <div class="setting-row">
              <div><strong>Foundation memory</strong><span>{{ remote.state.foundation_memory_server }} · {{ remote.state.foundation_memory_connected ? 'connected' : 'not connected' }}</span></div>
              <button class="switch-control" type="button" role="switch" aria-label="Foundation memory" :disabled="remote.state.settings_working" :aria-checked="remote.state.foundation_memory_enabled" :class="{ on: remote.state.foundation_memory_enabled }" @click="remote.setFoundationMemory(!remote.state.foundation_memory_enabled)"><span></span></button>
            </div>
          </div>
        </section>

        <section v-else-if="section === 'providers'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">CREDENTIALS & QUOTAS</span><h2>Providers</h2><p>Authentication, upstream endpoints, and provider-reported usage.</p></div>
          <div v-if="authProviderStatus" class="settings-feedback" :class="{ 'is-working': remote.state.auth_working }" role="status" aria-live="polite" data-testid="auth-provider-status">{{ authProviderStatus }}</div>

          <div class="provider-grid" :aria-busy="remote.state.auth_working">
            <article v-for="provider in remote.state.auth_providers" :key="provider.provider" class="provider-card surface-card">
              <div class="provider-heading">
                <div><strong>{{ providerLabel(provider.provider) }}</strong><span>{{ provider.method || 'not configured' }}</span></div>
                <span class="provider-state" :class="{ authenticated: provider.authenticated }">{{ provider.authenticated ? 'Authenticated' : 'Not authenticated' }}</span>
              </div>

              <div v-if="provider.usage?.available" class="usage-stack">
                <div v-for="window in provider.usage.windows" :key="window.id" class="usage-window">
                  <div><span>{{ window.label }}</span><strong>{{ window.remainingPercent }}% left</strong></div>
                  <div class="usage-track"><span :style="{ width: usageWidth(window.usedPercent) }"></span></div>
                  <small v-if="window.resetsAt">resets {{ window.resetsAt }}</small>
                </div>
              </div>
              <p v-else-if="provider.usage?.message" class="muted compact">{{ provider.usage.message }}</p>
              <p v-if="provider.error" class="error-text">{{ provider.error }}</p>

              <div class="provider-actions">
                <button v-if="browserProviders.has(provider.provider) && !provider.authenticated" class="secondary-button" type="button" :disabled="remote.state.auth_working" :aria-label="`Sign in to ${providerLabel(provider.provider)}`" @click="remote.send({ type: 'auth_login', provider: provider.provider })">Sign in</button>
                <button v-else-if="browserProviders.has(provider.provider)" class="secondary-button" type="button" :disabled="remote.state.auth_working" :aria-label="`Sign out of ${providerLabel(provider.provider)}`" @click="remote.send({ type: 'auth_logout', provider: provider.provider })">Sign out</button>
              </div>
              <form v-if="apiKeyProviders.has(provider.provider)" class="inline-form" @submit.prevent="saveApiKey(provider.provider)">
                <input v-model="apiKeys[provider.provider]" type="password" autocomplete="off" :disabled="remote.state.auth_working" :aria-label="`${providerLabel(provider.provider)} API key`" :placeholder="`Set ${providerLabel(provider.provider)} API key`" />
                <button class="secondary-button" type="submit" :disabled="remote.state.auth_working" :aria-label="`Set API key for ${providerLabel(provider.provider)}`">Set key</button>
              </form>
            </article>
          </div>

          <div class="setting-card surface-card">
            <div class="card-heading"><div><strong>OpenAI-compatible endpoints</strong><span>Custom providers saved by Yeet for this workspace.</span></div></div>
            <div v-if="customProviderStatus" class="settings-feedback" :class="{ 'is-working': remote.state.providers_working }" role="status" aria-live="polite" data-testid="custom-provider-status">{{ customProviderStatus }}</div>
            <fieldset class="provider-config-controls" :disabled="remote.state.providers_working" :aria-busy="remote.state.providers_working">
            <div v-for="provider in remote.state.provider_configurations" :key="provider.id" class="list-setting-row">
              <div><strong>{{ provider.id }}</strong><span class="truncate">{{ provider.base_url }}</span></div>
              <span>{{ provider.require_api_key ? 'API key' : 'No key' }}</span>
              <div class="provider-remove-actions" :data-provider-config-id="provider.id">
                <button
                  v-if="pendingProviderRemoval !== provider.id"
                  class="danger-text-button provider-remove-trigger"
                  type="button"
                  :aria-label="`Remove provider ${provider.id}`"
                  @click="armProviderRemoval(provider.id)"
                >Remove</button>
                <div
                  v-else
                  class="provider-remove-confirmation"
                  role="group"
                  :aria-label="`Confirm removal of provider ${provider.id}`"
                  aria-live="polite"
                >
                  <span class="provider-remove-warning">Remove saved endpoint?</span>
                  <span class="provider-remove-confirm-action" @click="finishProviderRemoval(provider.id)">
              <button class="danger-text-button" type="button" :aria-label="`Remove provider ${provider.id}`" @click="remote.send({ type: 'remove_provider', id: provider.id })">Remove</button>
                  </span>
                  <button class="settings-link-button provider-remove-cancel" type="button" :aria-label="`Cancel removal of provider ${provider.id}`" @click="cancelProviderRemoval(provider.id)">Cancel</button>
                </div>
              </div>
            </div>
            <form class="provider-form" @submit.prevent="saveProvider">
              <input v-model="providerId" placeholder="provider-id" aria-label="Provider ID" />
              <input v-model="providerUrl" type="url" placeholder="https://api.example.com/v1" aria-label="Base URL" />
              <label class="check-label"><input v-model="providerNeedsKey" type="checkbox" /> Requires API key</label>
              <button class="primary-button">Save provider</button>
            </form>
            </fieldset>
          </div>
        </section>

        <section v-else-if="section === 'capabilities'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">ATTACHED SYSTEMS</span><h2>Capabilities</h2><p>Keep everyday session controls simple. Skills, connected systems, and low-level runtime features stay available when needed.</p></div>
          <div v-if="remote.state.is_streaming" class="settings-feedback" role="status" aria-live="polite" data-testid="capabilities-locked">Capability changes are locked while a response is running.</div>
          <div v-if="remote.state.is_loading_capabilities" class="settings-feedback" role="status" aria-live="polite" data-testid="capabilities-loading">{{ remote.state.available_capabilities.length ? 'Refreshing capabilities…' : 'Loading capabilities…' }}</div>
          <div v-else-if="!remote.state.available_capabilities.length" class="setting-card surface-card"><div class="settings-empty">No capabilities have been reported yet.</div></div>
          <template v-else>
            <div v-if="primaryCapabilities.length" class="setting-card surface-card capability-primary-card" :aria-busy="remote.state.is_loading_capabilities">
              <div v-for="capability in primaryCapabilities" :key="capability.id" class="setting-row capability-row">
                <div class="capability-copy">
                  <strong>{{ capability.name }}</strong>
                  <span>{{ capability.description }}</span>
                </div>
                <button class="switch-control" type="button" role="switch" :aria-label="capability.name" :disabled="remote.state.is_loading_capabilities || remote.state.is_streaming" :aria-checked="capability.enabled" :class="{ on: capability.enabled }" @click="remote.toggleCapability(capability.id)"><span></span></button>
              </div>
            </div>

            <details v-if="skillCapabilities.length" class="capability-group surface-card">
              <summary>
                <span><strong>Skills</strong><small>Attach workflows to this session</small></span>
                <span class="capability-count">{{ enabledCapabilityCount(skillCapabilities) }}/{{ skillCapabilities.length }} attached</span>
              </summary>
              <div class="capability-group-body">
                <div v-for="capability in skillCapabilities" :key="capability.id" class="setting-row capability-row">
                  <div class="capability-copy"><strong>{{ capability.name }}</strong><span>{{ capability.description }}</span></div>
                  <button class="switch-control" type="button" role="switch" :aria-label="capability.name" :disabled="remote.state.is_loading_capabilities || remote.state.is_streaming" :aria-checked="capability.enabled" :class="{ on: capability.enabled }" @click="remote.toggleCapability(capability.id)"><span></span></button>
                </div>
              </div>
            </details>

            <details v-if="mcpCapabilities.length" class="capability-group surface-card">
              <summary>
                <span><strong>Connected systems</strong><small>MCP servers available to this session</small></span>
                <span class="capability-count">{{ enabledCapabilityCount(mcpCapabilities) }}/{{ mcpCapabilities.length }} enabled</span>
              </summary>
              <div class="capability-group-body">
                <div v-for="capability in mcpCapabilities" :key="capability.id" class="setting-row capability-row">
                  <div class="capability-copy"><strong>{{ capability.name }}</strong><span>{{ capability.description }}</span></div>
                  <button class="switch-control" type="button" role="switch" :aria-label="capability.name" :disabled="remote.state.is_loading_capabilities || remote.state.is_streaming" :aria-checked="capability.enabled" :class="{ on: capability.enabled }" @click="remote.toggleCapability(capability.id)"><span></span></button>
                </div>
              </div>
            </details>

            <details v-if="advancedCapabilities.length" class="capability-group surface-card capability-advanced">
              <summary>
                <span><strong>Advanced</strong><small>Built-ins and runtime-managed features</small></span>
                <span class="capability-count">{{ enabledCapabilityCount(advancedCapabilities) }}/{{ advancedCapabilities.length }} enabled</span>
              </summary>
              <div class="capability-group-body">
                <div v-for="capability in advancedCapabilities" :key="capability.id" class="setting-row capability-row">
                  <div class="capability-copy"><span class="capability-kind">{{ capability.kind }}</span><strong>{{ capability.name }}</strong><span>{{ capability.description }}</span></div>
                  <button class="switch-control" type="button" role="switch" :aria-label="capability.name" :disabled="remote.state.is_loading_capabilities || remote.state.is_streaming" :aria-checked="capability.enabled" :class="{ on: capability.enabled }" @click="remote.toggleCapability(capability.id)"><span></span></button>
                </div>
              </div>
            </details>
          </template>
        </section>

        <section v-else-if="section === 'sandbox'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">EXECUTION BOUNDARY</span><h2>Sandbox & permissions</h2><p>Workspace access, approval policy, network allowlist, environment, secrets, and resource limits.</p></div>
          <div v-if="sandboxSettingsStatus" class="settings-feedback" :class="{ 'is-working': remote.state.sandbox_working }" role="status" aria-live="polite" data-testid="sandbox-settings-status">{{ sandboxSettingsStatus }}</div>

          <fieldset v-if="sandbox" class="sandbox-control-scope" :disabled="remote.state.sandbox_working" :aria-busy="remote.state.sandbox_working">
            <div class="setting-card surface-card">
              <div class="setting-row stacked-mobile">
                <div><strong>Preset</strong><span>Apply a complete policy baseline.</span></div>
                <div class="segmented-control" role="group" aria-label="Sandbox preset">
                  <button v-for="preset in ['safe', 'balanced', 'unlimited']" :key="preset" type="button" :class="{ active: sandbox.preset === preset }" :aria-pressed="sandbox.preset === preset" @click="remote.updateSandbox({ type: 'apply_preset', preset })">{{ preset }}</button>
                </div>
              </div>
              <div class="setting-row stacked-mobile">
                <div><strong>Execution mode</strong><span>Sandbox commands or allow unrestricted execution.</span></div>
                <select aria-label="Execution mode" :value="sandbox.execution_mode" @change="remote.updateSandbox({ type: 'set_execution_mode', mode: ($event.target as HTMLSelectElement).value })">
                  <option value="sandboxed">Sandboxed</option><option value="unlimited">Unlimited</option>
                </select>
              </div>
              <div class="setting-row">
                <div><strong>Auto approve</strong><span>Automatically approve actions allowed by policy.</span></div>
                <button class="switch-control" type="button" role="switch" aria-label="Auto approve" :aria-checked="sandbox.auto_approve" :class="{ on: sandbox.auto_approve }" @click="remote.updateSandbox({ type: 'set_auto_approve', enabled: !sandbox.auto_approve })"><span></span></button>
              </div>
              <div class="setting-row">
                <div><strong>Scratch writable</strong><span>Permit writes to Yeet's scratch area.</span></div>
                <button class="switch-control" type="button" role="switch" aria-label="Scratch writable" :aria-checked="sandbox.scratch_writable" :class="{ on: sandbox.scratch_writable }" @click="remote.updateSandbox({ type: 'set_scratch_writable', enabled: !sandbox.scratch_writable })"><span></span></button>
              </div>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Workspace access</strong><span>Control which project paths tools may read.</span></div>
                <select aria-label="Workspace access mode" :value="sandbox.workspace_mode" @change="remote.updateSandbox({ type: 'set_workspace_mode', mode: ($event.target as HTMLSelectElement).value })"><option value="none">None</option><option value="all">All</option><option v-if="sandbox.workspace_mode === 'paths'" value="paths" disabled>Selected paths</option></select>
              </div>
              <div v-for="path in sandbox.workspace_paths" :key="path" class="list-setting-row"><code>{{ path }}</code><button class="danger-text-button" type="button" :aria-label="`Remove workspace path ${path}`" @click="remote.updateSandbox({ type: 'remove_workspace_path', path })">Remove</button></div>
              <form class="inline-form" @submit.prevent="addWorkspacePath">
                <input
                  v-model="workspacePath"
                  aria-label="Workspace path"
                  placeholder="relative/path"
                  :aria-invalid="workspacePathError ? 'true' : undefined"
                  :aria-describedby="workspacePathError ? 'workspace-path-error' : undefined"
                />
                <button class="secondary-button" :disabled="!workspacePath.trim() || Boolean(workspacePathError)">Add path</button>
              </form>
              <p v-if="workspacePathError" id="workspace-path-error" class="error-text settings-form-error" role="alert">{{ workspacePathError }}</p>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Network allowlist</strong><span>Only listed endpoints are reachable from sandboxed work.</span></div></div>
              <div v-for="item in sandbox.network_allow" :key="`${item.host}:${item.port || '*'}`" class="list-setting-row"><code>{{ item.host }}{{ item.port ? `:${item.port}` : '' }}</code><button class="danger-text-button" type="button" :aria-label="`Remove network ${item.host}${item.port ? `:${item.port}` : ''}`" @click="remote.updateSandbox({ type: 'remove_network', host: item.host, port: item.port })">Remove</button></div>
              <form class="inline-form network-form" @submit.prevent="addNetwork">
                <input
                  v-model="networkHost"
                  aria-label="Network host"
                  placeholder="api.example.com"
                  :aria-invalid="networkHostError ? 'true' : undefined"
                  :aria-describedby="networkHostError ? 'network-host-error' : undefined"
                />
                <input
                  v-model="networkPort"
                  aria-label="Network port"
                  inputmode="numeric"
                  placeholder="port (optional)"
                  :aria-invalid="networkPortError ? 'true' : undefined"
                  :aria-describedby="networkPortError ? 'network-port-error' : undefined"
                />
                <button class="secondary-button" :disabled="!networkHost.trim() || Boolean(networkHostError || networkPortError)">Allow</button>
              </form>
              <p v-if="networkHostError" id="network-host-error" class="error-text settings-form-error" role="alert">{{ networkHostError }}</p>
              <p v-if="networkPortError" id="network-port-error" class="error-text settings-form-error" role="alert">{{ networkPortError }}</p>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Environment</strong><span>Project-scoped environment values passed to permitted execution.</span></div></div>
              <div v-for="item in sandbox.environment" :key="item.key" class="list-setting-row"><code>{{ item.key }}</code><span class="masked-value">{{ item.value }}</span><button class="danger-text-button" type="button" :aria-label="`Remove environment variable ${item.key}`" @click="remote.updateSandbox({ type: 'remove_environment', key: item.key })">Remove</button></div>
              <form class="inline-form" @submit.prevent="setEnvironment">
                <input
                  v-model="environmentKey"
                  aria-label="Environment variable name"
                  placeholder="NAME"
                  :aria-invalid="environmentKeyError ? 'true' : undefined"
                  :aria-describedby="environmentKeyError ? 'environment-key-error' : undefined"
                />
                <input
                  v-model="environmentValue"
                  aria-label="Environment variable value"
                  placeholder="value"
                  :aria-invalid="environmentValueError ? 'true' : undefined"
                  :aria-describedby="environmentValueError ? 'environment-value-error' : undefined"
                />
                <button class="secondary-button" :disabled="!environmentKey.trim() || Boolean(environmentKeyError || environmentValueError)">Set</button>
              </form>
              <p v-if="environmentKeyError" id="environment-key-error" class="error-text settings-form-error" role="alert">{{ environmentKeyError }}</p>
              <p v-if="environmentValueError" id="environment-value-error" class="error-text settings-form-error" role="alert">{{ environmentValueError }}</p>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Secrets</strong><span>Secret identifiers are exposed without rendering secret values.</span></div></div>
              <div v-for="id in sandbox.secret_ids" :key="id" class="list-setting-row"><code>{{ id }}</code><button class="danger-text-button" type="button" :aria-label="`Remove secret ${id}`" @click="remote.updateSandbox({ type: 'remove_secret', id })">Remove</button></div>
              <form class="inline-form" @submit.prevent="addSecret">
                <input
                  v-model="secretId"
                  aria-label="Secret ID"
                  placeholder="SECRET_ID"
                  :aria-invalid="secretIdError ? 'true' : undefined"
                  :aria-describedby="secretIdError ? 'secret-id-error' : undefined"
                />
                <button class="secondary-button" :disabled="!secretId.trim() || Boolean(secretIdError)">Attach secret</button>
              </form>
              <p v-if="secretIdError" id="secret-id-error" class="error-text settings-form-error" role="alert">{{ secretIdError }}</p>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Limits</strong><span>Hard resource ceilings for sandboxed processes.</span></div></div>
              <div class="limit-grid">
                <label><span>Wall time (seconds)</span><input type="number" min="0" :value="sandbox.limits.wall_time_seconds" @change="setLimit('wall_time_seconds', ($event.target as HTMLInputElement).value)" /></label>
                <label><span>Max stdout bytes</span><input type="number" min="0" :value="sandbox.limits.max_stdout_bytes" @change="setLimit('max_stdout_bytes', ($event.target as HTMLInputElement).value)" /></label>
                <label><span>Max stderr bytes</span><input type="number" min="0" :value="sandbox.limits.max_stderr_bytes" @change="setLimit('max_stderr_bytes', ($event.target as HTMLInputElement).value)" /></label>
                <label><span>Max memory bytes</span><input type="number" min="0" :value="sandbox.limits.max_memory_bytes" @change="setLimit('max_memory_bytes', ($event.target as HTMLInputElement).value)" /></label>
                <label><span>Max processes</span><input type="number" min="0" :value="sandbox.limits.max_processes" @change="setLimit('max_processes', ($event.target as HTMLInputElement).value)" /></label>
              </div>
              <button
                v-if="!pendingSandboxReset"
                ref="sandboxResetTrigger"
                class="danger-outline-button sandbox-reset-trigger"
                type="button"
                @click="armSandboxReset"
              >Reset sandbox policy</button>
              <div
                v-else
                class="sandbox-reset-confirmation"
                role="group"
                aria-label="Confirm sandbox policy reset"
                aria-live="polite"
              >
                <button
                  ref="sandboxResetCancel"
                  class="danger-outline-button sandbox-reset-cancel"
                  type="button"
                  aria-label="Cancel sandbox reset"
                  @click="cancelSandboxReset"
                >Cancel reset</button>
                <button
                  class="danger-text-button sandbox-reset-confirm"
                  type="button"
                  aria-label="Confirm sandbox reset"
                  @click="confirmSandboxReset"
                >Reset now</button>
              </div>
            </div>
          </fieldset>
          <div v-else class="settings-empty large">Sandbox state has not been reported by the backend.</div>
        </section>

        <section v-else-if="section === 'sessions'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">PERSISTED WORK</span><h2>Sessions</h2><p>Restore an existing conversation or create a clean session without affecting other saved runs.</p></div>
          <div class="setting-card surface-card">
            <div class="card-heading"><div><strong>{{ savedSessionCountLabel(remote.state.saved_sessions.length) }}</strong><span>Current: {{ remote.currentSession?.title || 'new session' }}</span></div><button class="primary-button" type="button" @click="remote.newSession">New session</button></div>
            <div v-if="!remote.state.saved_sessions.length" class="settings-empty">No saved sessions yet. Start a new session when you want a clean conversation.</div>
            <button v-for="sessionItem in remote.state.saved_sessions" :key="sessionItem.id" class="settings-session-row" type="button" :class="{ active: sessionItem.id === remote.state.current_session_id }" :aria-current="sessionItem.id === remote.state.current_session_id ? 'page' : undefined" :aria-disabled="sessionItem.id === remote.state.current_session_id ? 'true' : undefined" @click="loadSavedSession(sessionItem.id)">
              <span><strong>{{ sessionItem.title || 'Untitled session' }}</strong><small>{{ sessionItem.model }} · {{ messageCountLabel(sessionItem.message_count) }}</small></span>
              <span class="session-updated"><time :datetime="sessionItem.updated_at" :title="sessionTimestampTitle(sessionItem.updated_at)">{{ sessionRelativeTime(sessionItem.updated_at) }}</time></span>
            </button>
          </div>
        </section>
        </fieldset>
      </main>
    </div>
  </div>
</template>
