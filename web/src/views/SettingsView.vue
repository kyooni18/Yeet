<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useRemoteStore } from '@/stores/remote'
import ModelPicker from '@/components/ModelPicker.vue'

const remote = useRemoteStore()
const route = useRoute()
const router = useRouter()

const sections = [
  ['runtime', 'Runtime'],
  ['providers', 'Providers'],
  ['capabilities', 'Capabilities'],
  ['sandbox', 'Sandbox'],
  ['sessions', 'Sessions'],
] as const

const section = computed(() => {
  const value = typeof route.params.section === 'string' ? route.params.section : 'runtime'
  return sections.some(([id]) => id === value) ? value : 'runtime'
})

const reasoningLevels = ['auto', 'low', 'medium', 'high']
const apiKeys = reactive<Record<string, string>>({})
const providerId = ref('')
const providerUrl = ref('')
const providerNeedsKey = ref(true)
const workspacePath = ref('')
const networkHost = ref('')
const networkPort = ref('')
const environmentKey = ref('')
const environmentValue = ref('')
const secretId = ref('')

const browserProviders = new Set(['codex-cli', 'gemini-web', 'claude'])
const apiKeyProviders = new Set(['openai', 'anthropic', 'gemini'])

const sandbox = computed(() => remote.state.sandbox_settings)

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

function chooseSection(id: string) {
  void router.replace(`/settings/${id}`)
}

function saveApiKey(provider: string) {
  const key = apiKeys[provider]?.trim()
  if (!key) return
  remote.send({ type: 'auth_set_api_key', provider, key })
  apiKeys[provider] = ''
}

function saveProvider() {
  if (!providerId.value.trim() || !providerUrl.value.trim()) return
  remote.send({
    type: 'save_provider',
    id: providerId.value.trim(),
    base_url: providerUrl.value.trim(),
    require_api_key: providerNeedsKey.value,
  })
  providerId.value = ''
  providerUrl.value = ''
}

function addWorkspacePath() {
  if (!workspacePath.value.trim()) return
  remote.updateSandbox({ type: 'add_workspace_path', path: workspacePath.value.trim() })
  workspacePath.value = ''
}

function addNetwork() {
  if (!networkHost.value.trim()) return
  const parsed = networkPort.value.trim() ? Number(networkPort.value) : null
  remote.updateSandbox({
    type: 'add_network',
    host: networkHost.value.trim(),
    port: parsed != null && Number.isFinite(parsed) ? parsed : null,
  })
  networkHost.value = ''
  networkPort.value = ''
}

function setEnvironment() {
  if (!environmentKey.value.trim()) return
  remote.updateSandbox({ type: 'set_environment', key: environmentKey.value.trim(), value: environmentValue.value })
  environmentKey.value = ''
  environmentValue.value = ''
}

function addSecret() {
  if (!secretId.value.trim()) return
  remote.updateSandbox({ type: 'add_secret', id: secretId.value.trim() })
  secretId.value = ''
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
</script>

<template>
  <div class="settings-app">
    <header class="settings-topbar">
      <button class="icon-button" aria-label="Back to conversation" @click="router.push('/')">←</button>
      <div>
        <span class="settings-kicker">Yeet</span>
        <h1>Settings</h1>
      </div>
      <span class="settings-connection"><span class="status-dot" :class="`status-${remote.connection}`"></span><span>{{ remote.connection }}</span></span>
    </header>

    <div class="settings-layout">
      <nav class="settings-nav" aria-label="Settings sections">
        <button
          v-for="[id, label] in sections"
          :key="id"
          :class="{ active: section === id }"
          :aria-current="section === id ? 'page' : undefined"
          @click="chooseSection(id)"
        >{{ label }}</button>
      </nav>

      <main class="settings-content">
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
              <div class="segmented-control">
                <button v-for="level in reasoningLevels" :key="level" :class="{ active: remote.state.active_reasoning_level === level }" @click="remote.selectReasoning(level)">{{ level }}</button>
              </div>
            </div>
            <div class="setting-row">
              <div><strong>Infinity</strong><span>Keep starting bounded execution epochs and retry transient provider failures until explicitly stopped.</span></div>
              <button class="switch-control" role="switch" :aria-checked="remote.state.infinity_mode" :class="{ on: remote.state.infinity_mode }" @click="remote.setInfinity(!remote.state.infinity_mode)"><span></span></button>
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

          <div class="setting-card surface-card">
            <div v-if="showOpenAiFlex" class="setting-row">
              <div><strong>OpenAI Flex</strong><span>Use Flex processing when supported by the active OpenAI provider.</span></div>
              <button class="switch-control" role="switch" :aria-checked="remote.state.openai_flex" :class="{ on: remote.state.openai_flex }" @click="remote.setOpenAiFlex(!remote.state.openai_flex)"><span></span></button>
            </div>
            <div class="setting-row">
              <div><strong>Foundation memory</strong><span>{{ remote.state.foundation_memory_server }} · {{ remote.state.foundation_memory_connected ? 'connected' : 'not connected' }}</span></div>
              <button class="switch-control" role="switch" :aria-checked="remote.state.foundation_memory_enabled" :class="{ on: remote.state.foundation_memory_enabled }" @click="remote.setFoundationMemory(!remote.state.foundation_memory_enabled)"><span></span></button>
            </div>
          </div>
        </section>

        <section v-else-if="section === 'providers'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">CREDENTIALS & QUOTAS</span><h2>Providers</h2><p>Authentication, upstream endpoints, and provider-reported usage.</p></div>

          <div class="provider-grid">
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
                <button v-if="browserProviders.has(provider.provider) && !provider.authenticated" class="secondary-button" @click="remote.send({ type: 'auth_login', provider: provider.provider })">Sign in</button>
                <button v-else-if="browserProviders.has(provider.provider)" class="secondary-button" @click="remote.send({ type: 'auth_logout', provider: provider.provider })">Sign out</button>
              </div>
              <form v-if="apiKeyProviders.has(provider.provider)" class="inline-form" @submit.prevent="saveApiKey(provider.provider)">
                <input v-model="apiKeys[provider.provider]" type="password" autocomplete="off" :placeholder="`Set ${providerLabel(provider.provider)} API key`" />
                <button class="secondary-button">Set key</button>
              </form>
            </article>
          </div>

          <div class="setting-card surface-card">
            <div class="card-heading"><div><strong>OpenAI-compatible endpoints</strong><span>Custom providers saved by Yeet for this workspace.</span></div></div>
            <div v-for="provider in remote.state.provider_configurations" :key="provider.id" class="list-setting-row">
              <div><strong>{{ provider.id }}</strong><span class="truncate">{{ provider.base_url }}</span></div>
              <span>{{ provider.require_api_key ? 'API key' : 'No key' }}</span>
              <button class="danger-text-button" @click="remote.send({ type: 'remove_provider', id: provider.id })">Remove</button>
            </div>
            <form class="provider-form" @submit.prevent="saveProvider">
              <input v-model="providerId" placeholder="provider-id" aria-label="Provider ID" />
              <input v-model="providerUrl" type="url" placeholder="https://api.example.com/v1" aria-label="Base URL" />
              <label class="check-label"><input v-model="providerNeedsKey" type="checkbox" /> Requires API key</label>
              <button class="primary-button">Save provider</button>
            </form>
          </div>
        </section>

        <section v-else-if="section === 'capabilities'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">ATTACHED SYSTEMS</span><h2>Capabilities</h2><p>Attach skills to the current session, and enable or disable the other available systems.</p></div>
          <div class="setting-card surface-card">
            <div v-if="!remote.state.available_capabilities.length" class="settings-empty">No capabilities have been reported yet.</div>
            <div v-for="capability in remote.state.available_capabilities" :key="capability.id" class="setting-row capability-row">
              <div class="capability-copy">
                <span class="capability-kind">{{ capability.kind }}</span>
                <strong>{{ capability.name }}</strong>
                <span>{{ capability.description }}</span>
              </div>
              <button class="switch-control" role="switch" :aria-checked="capability.enabled" :class="{ on: capability.enabled }" @click="remote.toggleCapability(capability.id)"><span></span></button>
            </div>
          </div>
        </section>

        <section v-else-if="section === 'sandbox'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">EXECUTION BOUNDARY</span><h2>Sandbox & permissions</h2><p>Workspace access, approval policy, network allowlist, environment, secrets, and resource limits.</p></div>

          <template v-if="sandbox">
            <div class="setting-card surface-card">
              <div class="setting-row stacked-mobile">
                <div><strong>Preset</strong><span>Apply a complete policy baseline.</span></div>
                <div class="segmented-control">
                  <button v-for="preset in ['safe', 'balanced', 'unlimited']" :key="preset" :class="{ active: sandbox.preset === preset }" @click="remote.updateSandbox({ type: 'apply_preset', preset })">{{ preset }}</button>
                </div>
              </div>
              <div class="setting-row stacked-mobile">
                <div><strong>Execution mode</strong><span>Sandbox commands or allow unrestricted execution.</span></div>
                <select :value="sandbox.execution_mode" @change="remote.updateSandbox({ type: 'set_execution_mode', mode: ($event.target as HTMLSelectElement).value })">
                  <option value="sandboxed">Sandboxed</option><option value="unlimited">Unlimited</option>
                </select>
              </div>
              <div class="setting-row">
                <div><strong>Auto approve</strong><span>Automatically approve actions allowed by policy.</span></div>
                <button class="switch-control" role="switch" :aria-checked="sandbox.auto_approve" :class="{ on: sandbox.auto_approve }" @click="remote.updateSandbox({ type: 'set_auto_approve', enabled: !sandbox.auto_approve })"><span></span></button>
              </div>
              <div class="setting-row">
                <div><strong>Scratch writable</strong><span>Permit writes to Yeet's scratch area.</span></div>
                <button class="switch-control" role="switch" :aria-checked="sandbox.scratch_writable" :class="{ on: sandbox.scratch_writable }" @click="remote.updateSandbox({ type: 'set_scratch_writable', enabled: !sandbox.scratch_writable })"><span></span></button>
              </div>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Workspace access</strong><span>Control which project paths tools may read.</span></div>
                <select :value="sandbox.workspace_mode" @change="remote.updateSandbox({ type: 'set_workspace_mode', mode: ($event.target as HTMLSelectElement).value })"><option value="none">None</option><option value="all">All</option><option v-if="sandbox.workspace_mode === 'paths'" value="paths" disabled>Selected paths</option></select>
              </div>
              <div v-for="path in sandbox.workspace_paths" :key="path" class="list-setting-row"><code>{{ path }}</code><button class="danger-text-button" @click="remote.updateSandbox({ type: 'remove_workspace_path', path })">Remove</button></div>
              <form class="inline-form" @submit.prevent="addWorkspacePath"><input v-model="workspacePath" placeholder="relative/path" /><button class="secondary-button">Add path</button></form>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Network allowlist</strong><span>Only listed endpoints are reachable from sandboxed work.</span></div></div>
              <div v-for="item in sandbox.network_allow" :key="`${item.host}:${item.port || '*'}`" class="list-setting-row"><code>{{ item.host }}{{ item.port ? `:${item.port}` : '' }}</code><button class="danger-text-button" @click="remote.updateSandbox({ type: 'remove_network', host: item.host, port: item.port })">Remove</button></div>
              <form class="inline-form network-form" @submit.prevent="addNetwork"><input v-model="networkHost" placeholder="api.example.com" /><input v-model="networkPort" inputmode="numeric" placeholder="port (optional)" /><button class="secondary-button">Allow</button></form>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Environment</strong><span>Project-scoped environment values passed to permitted execution.</span></div></div>
              <div v-for="item in sandbox.environment" :key="item.key" class="list-setting-row"><code>{{ item.key }}</code><span class="masked-value">{{ item.value }}</span><button class="danger-text-button" @click="remote.updateSandbox({ type: 'remove_environment', key: item.key })">Remove</button></div>
              <form class="inline-form" @submit.prevent="setEnvironment"><input v-model="environmentKey" placeholder="NAME" /><input v-model="environmentValue" placeholder="value" /><button class="secondary-button">Set</button></form>
            </div>

            <div class="setting-card surface-card">
              <div class="card-heading"><div><strong>Secrets</strong><span>Secret identifiers are exposed without rendering secret values.</span></div></div>
              <div v-for="id in sandbox.secret_ids" :key="id" class="list-setting-row"><code>{{ id }}</code><button class="danger-text-button" @click="remote.updateSandbox({ type: 'remove_secret', id })">Remove</button></div>
              <form class="inline-form" @submit.prevent="addSecret"><input v-model="secretId" placeholder="SECRET_ID" /><button class="secondary-button">Attach secret</button></form>
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
              <button class="danger-outline-button" @click="remote.updateSandbox({ type: 'reset' })">Reset sandbox policy</button>
            </div>
          </template>
          <div v-else class="settings-empty large">Sandbox state has not been reported by the backend.</div>
        </section>

        <section v-else-if="section === 'sessions'" class="settings-section">
          <div class="section-heading"><span class="eyebrow">PERSISTED WORK</span><h2>Sessions</h2><p>Restore an existing conversation or create a clean session without affecting other saved runs.</p></div>
          <div class="setting-card surface-card">
            <div class="card-heading"><div><strong>{{ remote.state.saved_sessions.length }} saved sessions</strong><span>Current: {{ remote.currentSession?.title || 'new session' }}</span></div><button class="primary-button" @click="remote.newSession">New session</button></div>
            <button v-for="sessionItem in remote.state.saved_sessions" :key="sessionItem.id" class="settings-session-row" :class="{ active: sessionItem.id === remote.state.current_session_id }" @click="remote.loadSession(sessionItem.id)">
              <span><strong>{{ sessionItem.title || 'Untitled session' }}</strong><small>{{ sessionItem.model }} · {{ sessionItem.message_count }} messages</small></span>
              <span>{{ sessionItem.updated_at }}</span>
            </button>
          </div>
        </section>
      </main>
    </div>
  </div>
</template>
