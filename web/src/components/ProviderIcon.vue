<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(defineProps<{
  provider: string
  size?: number
}>(), {
  size: 24,
})

const normalized = computed(() => props.provider.trim().toLowerCase() || 'other')

const glyph = computed(() => {
  switch (normalized.value) {
    case 'openai': return '◎'
    case 'codex-cli': return 'CX'
    case 'anthropic': return 'A'
    case 'claude': return 'C'
    case 'google': return 'G'
    case 'gemini': return '✦'
    case 'gemini-web': return 'GW'
    case 'xai': return 'x'
    case 'mistral': return 'M'
    case 'openrouter': return 'OR'
    case 'groq': return 'GQ'
    case 'ollama': return 'O'
    case 'deepseek': return 'DS'
    case 'azure': return 'Az'
    case 'bedrock': return 'AWS'
    case 'vertex': return 'V'
    case 'meta': return 'M'
    case 'local': return 'L'
    default: return normalized.value.slice(0, 2).toUpperCase() || '•'
  }
})
</script>

<template>
  <span
    class="provider-icon"
    :class="{ 'provider-icon--wide': glyph.length > 1 }"
    :data-provider="normalized"
    :style="{ width: `${size}px`, height: `${size}px` }"
    aria-hidden="true"
  >{{ glyph }}</span>
</template>

<style scoped>
.provider-icon {
  display: inline-grid;
  place-items: center;
  flex: 0 0 auto;
  border: 1px solid var(--border-strong);
  border-radius: 7px;
  background: rgba(255, 255, 255, .035);
  color: var(--soft);
  font-size: 11px;
  font-weight: 700;
  line-height: 1;
  letter-spacing: -.04em;
}

.provider-icon--wide {
  font-size: 8px;
  letter-spacing: -.08em;
}
</style>
