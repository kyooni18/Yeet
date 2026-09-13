<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useRemoteStore } from '@/stores/remote'
import ProviderIcon from './ProviderIcon.vue'

type PickerVariant = 'topbar' | 'panel'

interface ModelItem {
  id: string
  name: string
  provider: string
  providerLabel: string
  contextLabel: string | null
  searchText: string
}

const props = withDefaults(defineProps<{
  variant?: PickerVariant
}>(), {
  variant: 'topbar',
})

const remote = useRemoteStore()
const root = ref<HTMLElement | null>(null)
const trigger = ref<HTMLButtonElement | null>(null)
const searchInput = ref<HTMLInputElement | null>(null)
const open = ref(false)
const query = ref('')
const providerFilter = ref('all')
const activeIndex = ref(0)
const listboxId = `model-picker-${Math.random().toString(36).slice(2, 9)}`

const providerLabels: Record<string, string> = {
  openai: 'OpenAI',
  'codex-cli': 'Codex CLI',
  anthropic: 'Claude (Anthropic)',
  claude: 'Claude Web',
  google: 'Google',
  gemini: 'Gemini',
  'gemini-web': 'Gemini Web',
  xai: 'xAI',
  mistral: 'Mistral',
  openrouter: 'OpenRouter',
  groq: 'Groq',
  ollama: 'Ollama',
  deepseek: 'DeepSeek',
  azure: 'Azure',
  bedrock: 'Bedrock',
  vertex: 'Vertex',
  meta: 'Meta',
  local: 'Local',
  other: 'Other',
}

function normalizeProvider(value: string): string {
  const provider = value.trim().toLowerCase()
  if (['openai-responses', 'openai-chat', 'chatgpt'].includes(provider)) return 'openai'
  if (['codex', 'codex-cli'].includes(provider)) return 'codex-cli'
  if (provider === 'claude') return 'claude'
  if (provider === 'gemini') return 'gemini'
  if (['google-ai', 'googleai'].includes(provider)) return 'google'
  if (['grok'].includes(provider)) return 'xai'
  if (['aws', 'amazon-bedrock'].includes(provider)) return 'bedrock'
  if (['google-vertex', 'vertex-ai'].includes(provider)) return 'vertex'
  return provider || 'other'
}

function inferProvider(id: string): string {
  const lower = id.toLowerCase()
  if (/^(codex)/.test(lower)) return 'codex-cli'
  if (/^(gpt-|o[134]-|chatgpt)/.test(lower)) return 'openai'
  if (lower.startsWith('claude')) return 'claude'
  if (lower.startsWith('gemini')) return 'gemini'
  if (/^(mistral|codestral)/.test(lower)) return 'mistral'
  if (lower.startsWith('grok')) return 'xai'
  if (lower.startsWith('deepseek')) return 'deepseek'
  if (lower.startsWith('llama')) return 'meta'
  return 'other'
}

function providerLabel(provider: string): string {
  if (providerLabels[provider]) return providerLabels[provider]
  return provider
    .split(/[-_.]+/g)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ') || 'Other'
}

function formatContextLabel(contextLength: number | null | undefined): string | null {
  if (!contextLength || contextLength <= 0) return null
  if (contextLength >= 1_000_000) return `${Math.round(contextLength / 100_000) / 10}M context`
  if (contextLength >= 1_000) return `${Math.round(contextLength / 100) / 10}k context`
  return `${contextLength} context`
}

function fallbackModel(id: string): ModelItem {
  const slash = id.indexOf('/')
  const provider = slash > 0 ? normalizeProvider(id.slice(0, slash)) : inferProvider(id)
  const name = slash > 0 ? id.slice(slash + 1) : id
  const label = providerLabel(provider)
  return {
    id,
    name,
    provider,
    providerLabel: label,
    contextLabel: null,
    searchText: `${id} ${name} ${provider} ${label}`.toLowerCase(),
  }
}

function catalogModel(entry: { id: string; provider: string; model: string; context_length?: number | null }): ModelItem {
  const provider = normalizeProvider(entry.provider)
  const label = providerLabel(provider)
  const name = entry.model.trim() || entry.id
  const contextLabel = formatContextLabel(entry.context_length)
  return {
    id: entry.id,
    name,
    provider,
    providerLabel: label,
    contextLabel,
    searchText: `${entry.id} ${name} ${provider} ${label} ${contextLabel ?? ''}`.toLowerCase(),
  }
}

const models = computed(() => {
  const seen = new Set<string>()
  const result: ModelItem[] = []

  for (const entry of remote.state.model_catalog) {
    if (!entry.id || seen.has(entry.id)) continue
    seen.add(entry.id)
    result.push(catalogModel(entry))
  }

  for (const id of remote.state.available_models) {
    if (!id || seen.has(id)) continue
    seen.add(id)
    result.push(fallbackModel(id))
  }

  const active = remote.state.active_model
  if (active && !seen.has(active)) result.unshift(fallbackModel(active))
  return result
})

const activeModel = computed(() => {
  const active = remote.state.active_model
  if (!active) return null
  return models.value.find((model) => model.id === active) ?? fallbackModel(active)
})

const providers = computed(() => {
  const seen = new Set<string>()
  const result: Array<{ id: string; label: string }> = []
  for (const model of models.value) {
    if (seen.has(model.provider)) continue
    seen.add(model.provider)
    result.push({ id: model.provider, label: model.providerLabel })
  }
  return result
})

const filteredModels = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return models.value.filter((model) => {
    if (providerFilter.value !== 'all' && model.provider !== providerFilter.value) return false
    return !needle || model.searchText.includes(needle)
  })
})

function optionId(index: number): string {
  return `${listboxId}-option-${index}`
}

function resetActiveIndex(): void {
  const selected = filteredModels.value.findIndex((model) => model.id === remote.state.active_model)
  activeIndex.value = Math.max(0, selected)
}

async function revealActiveOption(): Promise<void> {
  await nextTick()
  document.getElementById(optionId(activeIndex.value))?.scrollIntoView({ block: 'nearest' })
}

async function openPicker(direction: 'selected' | 'first' | 'last' = 'selected'): Promise<void> {
  if (open.value) return
  remote.requestModels()
  query.value = ''
  providerFilter.value = 'all'
  open.value = true
  await nextTick()
  if (direction === 'first') activeIndex.value = 0
  else if (direction === 'last') activeIndex.value = Math.max(0, filteredModels.value.length - 1)
  else resetActiveIndex()
  await nextTick()
  searchInput.value?.focus()
  void revealActiveOption()
}

function closePicker(returnFocus = false): void {
  if (!open.value) return
  open.value = false
  query.value = ''
  providerFilter.value = 'all'
  if (returnFocus) void nextTick(() => trigger.value?.focus())
}

function togglePicker(): void {
  if (open.value) closePicker()
  else void openPicker()
}

function selectModel(model: ModelItem): void {
  remote.selectModel(model.id)
  closePicker(true)
}

function setProvider(provider: string): void {
  providerFilter.value = provider
  resetActiveIndex()
  void revealActiveOption()
  searchInput.value?.focus()
}

function moveActive(delta: number): void {
  const count = filteredModels.value.length
  if (!count) return
  activeIndex.value = (activeIndex.value + delta + count) % count
  void revealActiveOption()
}

function onTriggerKeydown(event: KeyboardEvent): void {
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    void openPicker('first')
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    void openPicker('last')
  } else if (event.key === 'Escape' && open.value) {
    event.preventDefault()
    closePicker(true)
  }
}

function onSearchKeydown(event: KeyboardEvent): void {
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    moveActive(1)
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    moveActive(-1)
  } else if (event.key === 'Home') {
    event.preventDefault()
    activeIndex.value = 0
    void revealActiveOption()
  } else if (event.key === 'End') {
    event.preventDefault()
    activeIndex.value = Math.max(0, filteredModels.value.length - 1)
    void revealActiveOption()
  } else if (event.key === 'Enter') {
    event.preventDefault()
    const model = filteredModels.value[activeIndex.value]
    if (model) selectModel(model)
  } else if (event.key === 'Escape') {
    event.preventDefault()
    closePicker(true)
  }
}

function onDocumentPointerDown(event: PointerEvent): void {
  if (open.value && root.value && !root.value.contains(event.target as Node)) closePicker()
}

function onDocumentFocusIn(event: FocusEvent): void {
  if (open.value && root.value && !root.value.contains(event.target as Node)) closePicker()
}

watch(filteredModels, () => {
  if (!open.value) return
  const count = filteredModels.value.length
  if (!count) activeIndex.value = 0
  else if (activeIndex.value >= count) activeIndex.value = count - 1
})

watch(open, (isOpen) => {
  if (isOpen) {
    document.addEventListener('pointerdown', onDocumentPointerDown)
    document.addEventListener('focusin', onDocumentFocusIn)
  } else {
    document.removeEventListener('pointerdown', onDocumentPointerDown)
    document.removeEventListener('focusin', onDocumentFocusIn)
  }
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onDocumentPointerDown)
  document.removeEventListener('focusin', onDocumentFocusIn)
})
</script>

<template>
  <div ref="root" class="model-picker" :class="`model-picker--${props.variant}`" data-testid="model-picker">
    <button
      ref="trigger"
      class="model-picker-trigger"
      type="button"
      data-testid="model-picker-trigger"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :aria-controls="open ? listboxId : undefined"
      :aria-label="`Model: ${activeModel?.name || 'Choose model'}${activeModel ? `, provider: ${activeModel.providerLabel}` : ''}`"
      @click="togglePicker"
      @keydown="onTriggerKeydown"
    >
      <ProviderIcon :provider="activeModel?.provider || 'other'" :size="props.variant === 'panel' ? 28 : 24" />
      <span class="model-picker-trigger-copy">
        <span class="model-picker-trigger-name">{{ activeModel?.name || 'Choose model' }}</span>
        <span v-if="props.variant === 'panel'" class="model-picker-trigger-provider">{{ activeModel?.providerLabel || 'Model' }}</span>
      </span>
      <span class="model-picker-chevron" aria-hidden="true">⌄</span>
    </button>

    <Transition name="model-picker-popover">
      <section v-if="open" class="model-picker-popover" data-testid="model-picker-popover" aria-label="Choose model">
        <div class="model-picker-search-row">
          <input
            ref="searchInput"
            v-model="query"
            class="model-picker-search"
            data-testid="model-search"
            type="search"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            placeholder="Search models"
            aria-label="Search models"
            :aria-controls="listboxId"
            :aria-activedescendant="filteredModels.length ? optionId(activeIndex) : undefined"
            @keydown="onSearchKeydown"
          >
          <span v-if="remote.state.is_loading_models" class="spinner" aria-label="Loading models"></span>
        </div>

        <div v-if="providers.length > 1" class="model-picker-providers" aria-label="Filter by provider">
          <button
            type="button"
            :class="{ active: providerFilter === 'all' }"
            :aria-pressed="providerFilter === 'all'"
            @click="setProvider('all')"
          >All</button>
          <button
            v-for="provider in providers"
            :key="provider.id"
            type="button"
            :class="{ active: providerFilter === provider.id }"
            :aria-pressed="providerFilter === provider.id"
            @click="setProvider(provider.id)"
          >{{ provider.label }}</button>
        </div>

        <div
          :id="listboxId"
          class="model-picker-list"
          role="listbox"
          aria-label="Models"
          :aria-busy="remote.state.is_loading_models"
        >
          <div v-if="remote.state.is_loading_models && models.length === 0" class="model-picker-empty" role="status">
            Loading models…
          </div>
          <div v-else-if="models.length === 0" class="model-picker-empty" role="status">
            No models available
          </div>
          <div v-else-if="filteredModels.length === 0" class="model-picker-empty" role="status">
            No models match
          </div>
          <button
            v-for="(model, index) in filteredModels"
            v-else
            :id="optionId(index)"
            :key="model.id"
            type="button"
            class="model-picker-option"
            :class="{ active: index === activeIndex, selected: model.id === remote.state.active_model }"
            role="option"
            :aria-selected="model.id === remote.state.active_model"
            :data-model="model.id"
            @mousemove="activeIndex = index"
            @focus="activeIndex = index"
            @click="selectModel(model)"
          >
            <ProviderIcon :provider="model.provider" :size="26" />
            <span class="model-picker-option-copy">
              <strong>{{ model.name }}</strong>
              <span>{{ model.providerLabel }}<template v-if="model.contextLabel"> · {{ model.contextLabel }}</template></span>
            </span>
            <span v-if="model.id === remote.state.active_model" class="model-picker-check" aria-hidden="true">✓</span>
          </button>
        </div>
      </section>
    </Transition>
  </div>
</template>

<style scoped>
.model-picker {
  position: relative;
  min-width: 0;
}

.model-picker-trigger {
  display: flex;
  width: 100%;
  min-width: 0;
  min-height: 40px;
  align-items: center;
  gap: 8px;
  border: 0;
  border-radius: 9px;
  background: transparent;
  color: var(--text);
  text-align: left;
  cursor: pointer;
}

.model-picker-trigger:hover,
.model-picker-trigger[aria-expanded="true"] {
  background: var(--surface);
}

.model-picker--topbar .model-picker-trigger {
  max-width: min(36vw, 280px);
  padding: 0 8px;
}

.model-picker--panel .model-picker-trigger {
  min-height: 48px;
  padding: 7px 10px;
  border: 1px solid var(--border-strong);
  background: var(--surface);
}

.model-picker-trigger-copy,
.model-picker-option-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
}

.model-picker-trigger-name {
  overflow: hidden;
  font-size: 12px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-picker-trigger-provider {
  margin-top: 2px;
  color: var(--dim);
  font-size: 10px;
}

.model-picker-chevron {
  flex: 0 0 auto;
  color: var(--dim);
  font-size: 13px;
  transition: transform 120ms ease;
}

.model-picker-trigger[aria-expanded="true"] .model-picker-chevron {
  transform: rotate(180deg);
}

.model-picker-popover {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  z-index: 140;
  display: flex;
  width: min(380px, calc(100vw - 24px));
  max-height: min(520px, calc(var(--visual-viewport-height) - 74px));
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border-strong);
  border-radius: 14px;
  background: rgba(15, 10, 12, .985);
  box-shadow: 0 22px 60px rgba(0, 0, 0, .46), 0 0 28px rgba(255, 68, 43, .035);
}

.model-picker--panel .model-picker-popover {
  position: relative;
  top: auto;
  left: auto;
  width: 100%;
  max-height: min(52dvh, 420px);
  margin-top: 7px;
  box-shadow: none;
}

.model-picker-search-row {
  display: flex;
  min-height: 52px;
  align-items: center;
  gap: 8px;
  padding: 8px;
  border-bottom: 1px solid var(--border);
}

.model-picker-search {
  width: 100%;
  min-width: 0;
  min-height: 38px !important;
  background: var(--surface-soft) !important;
  font-size: 12px;
}

.model-picker-search-row .spinner {
  margin-right: 5px;
}

.model-picker-providers {
  display: flex;
  flex: 0 0 auto;
  gap: 5px;
  overflow-x: auto;
  padding: 7px 8px;
  border-bottom: 1px solid var(--border);
  scrollbar-width: none;
}

.model-picker-providers::-webkit-scrollbar {
  display: none;
}

.model-picker-providers button {
  min-height: 30px;
  flex: 0 0 auto;
  padding: 0 9px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--muted);
  font-size: 10.5px;
  cursor: pointer;
}

.model-picker-providers button:hover {
  background: var(--surface);
  color: var(--soft);
}

.model-picker-providers button.active {
  background: rgba(255, 94, 63, .11);
  color: var(--text);
}

.model-picker-list {
  min-height: 0;
  overflow-y: auto;
  padding: 6px;
}

.model-picker-option {
  display: grid;
  width: 100%;
  min-height: 44px;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: center;
  gap: 9px;
  padding: 5px 7px;
  border: 0;
  border-radius: 9px;
  background: transparent;
  color: var(--soft);
  text-align: left;
  cursor: pointer;
}

.model-picker-option:hover,
.model-picker-option.active {
  background: var(--surface);
  color: var(--text);
}

.model-picker-option.selected {
  color: var(--text);
}

.model-picker-option-copy strong {
  overflow: hidden;
  font-size: 12px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-picker-option-copy span {
  margin-top: 2px;
  overflow: hidden;
  color: var(--dim);
  font-size: 9.5px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-picker-check {
  padding-right: 3px;
  color: var(--brand);
  font-size: 12px;
}

.model-picker-empty {
  padding: 25px 12px;
  color: var(--dim);
  font-size: 11px;
  text-align: center;
}

.model-picker-popover-enter-active,
.model-picker-popover-leave-active {
  transition: opacity 110ms ease, transform 110ms ease;
}

.model-picker-popover-enter-from,
.model-picker-popover-leave-to {
  opacity: 0;
  transform: translateY(-3px);
}

@media (max-width: 899px) {
  .model-picker--topbar .model-picker-trigger {
    min-height: 50px;
    gap: 10px;
    padding-right: 10px;
    padding-left: 9px;
    border-radius: 14px;
  }
  .model-picker--topbar .model-picker-trigger-name { font-size: 17px; }
  .model-picker--topbar .model-picker-chevron { font-size: 17px; }
  .model-picker--topbar :deep(.provider-icon) { width: 31px !important; height: 31px !important; }

  .model-picker--topbar .model-picker-popover {
    position: fixed;
    top: calc(var(--visual-viewport-top) + 72px);
    right: max(8px, env(safe-area-inset-right));
    left: max(8px, env(safe-area-inset-left));
    width: auto;
    max-height: min(62dvh, calc(var(--visual-viewport-height) - 80px));
    border-radius: 15px;
  }
  .model-picker-search-row { min-height: 48px; padding: 7px; }
  .model-picker-search { min-height: 38px !important; font-size: 12px; }
  .model-picker-providers { gap: 5px; padding: 6px 7px; }
  .model-picker-providers button { min-height: 34px; padding: 0 10px; font-size: 10.5px; }
  .model-picker-list { padding: 5px; }
  .model-picker-option { min-height: 44px; gap: 8px; padding: 4px 6px; border-radius: 9px; }
  .model-picker-option-copy strong { font-size: 12px; }
  .model-picker-option-copy span { font-size: 9.5px; }
  .model-picker-check { font-size: 12px; }
}

@media (max-width: 480px) {
  .model-picker--topbar .model-picker-trigger {
    max-width: 48vw;
  }
}
</style>
