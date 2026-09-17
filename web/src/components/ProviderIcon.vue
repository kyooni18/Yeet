<script setup lang="ts">
import { computed } from 'vue'
import anthropicIcon from '@lobehub/icons-static-svg/icons/anthropic.svg?url'
import awsIcon from '@lobehub/icons-static-svg/icons/aws-color.svg?url'
import azureIcon from '@lobehub/icons-static-svg/icons/azure-color.svg?url'
import claudeIcon from '@lobehub/icons-static-svg/icons/claude-color.svg?url'
import codexIcon from '@lobehub/icons-static-svg/icons/codex-color.svg?url'
import deepseekIcon from '@lobehub/icons-static-svg/icons/deepseek-color.svg?url'
import geminiIcon from '@lobehub/icons-static-svg/icons/gemini-color.svg?url'
import googleIcon from '@lobehub/icons-static-svg/icons/google-color.svg?url'
import groqIcon from '@lobehub/icons-static-svg/icons/groq.svg?url'
import metaIcon from '@lobehub/icons-static-svg/icons/meta-color.svg?url'
import mistralIcon from '@lobehub/icons-static-svg/icons/mistral-color.svg?url'
import ollamaIcon from '@lobehub/icons-static-svg/icons/ollama.svg?url'
import openaiIcon from '@lobehub/icons-static-svg/icons/openai.svg?url'
import opencodeIcon from '@lobehub/icons-static-svg/icons/opencode.svg?url'
import openrouterIcon from '@lobehub/icons-static-svg/icons/openrouter-color.svg?url'
import sparkIcon from '@lobehub/icons-static-svg/icons/spark.svg?url'
import vertexIcon from '@lobehub/icons-static-svg/icons/vertexai.svg?url'
import xaiIcon from '@lobehub/icons-static-svg/icons/xai.svg?url'

const props = withDefaults(defineProps<{
  provider: string
  size?: number
}>(), {
  size: 24,
})

const normalized = computed(() => props.provider.trim().toLowerCase() || 'other')
const iconSources: Record<string, string> = {
  openai: openaiIcon,
  opencode: opencodeIcon,
  'codex-cli': codexIcon,
  anthropic: anthropicIcon,
  claude: claudeIcon,
  google: googleIcon,
  gemini: geminiIcon,
  'gemini-web': geminiIcon,
  xai: xaiIcon,
  mistral: mistralIcon,
  openrouter: openrouterIcon,
  groq: groqIcon,
  ollama: ollamaIcon,
  deepseek: deepseekIcon,
  azure: azureIcon,
  bedrock: awsIcon,
  vertex: vertexIcon,
  meta: metaIcon,
  local: ollamaIcon,
  other: sparkIcon,
}

const iconSource = computed(() => iconSources[normalized.value] ?? null)
const monochromeProviders = new Set(['openai', 'opencode', 'anthropic', 'groq', 'ollama', 'vertex', 'xai', 'other'])
</script>

<template>
  <span
    class="provider-icon"
    :class="{ 'provider-icon--monochrome': monochromeProviders.has(normalized) }"
    :data-provider="normalized"
    :style="{ width: `${size}px`, height: `${size}px` }"
    aria-hidden="true"
  >
    <img v-if="iconSource" :src="iconSource" alt="" draggable="false">
  </span>
</template>

<style scoped>
.provider-icon {
  display: inline-grid;
  place-items: center;
  flex: 0 0 auto;
  overflow: hidden;
  border: 1px solid var(--border-strong);
  border-radius: 7px;
  background: rgba(255, 255, 255, .035);
}
.provider-icon img {
  width: 72%;
  height: 72%;
  object-fit: contain;
}
.provider-icon--monochrome img { filter: brightness(0) invert(1); }
.provider-icon[data-provider="openai"],
.provider-icon[data-provider="codex-cli"] { color: #f4f0ed; }
.provider-icon[data-provider="anthropic"],
.provider-icon[data-provider="claude"] { color: #d08b59; }
.provider-icon[data-provider="gemini"],
.provider-icon[data-provider="gemini-web"] { color: #8ab4ff; }
.provider-icon[data-provider="mistral"] { color: #ff725e; }
.provider-icon[data-provider="xai"] { color: #f5f5f5; }
.provider-icon[data-provider="meta"] { color: #78a9ff; }
.provider-icon[data-provider="azure"],
.provider-icon[data-provider="vertex"] { color: #5aa7ff; }
.provider-icon[data-provider="bedrock"] { color: #ffad57; }
.provider-icon[data-provider="openrouter"] { color: #b88cff; }
.provider-icon[data-provider="local"],
.provider-icon[data-provider="ollama"] { color: #c8d1d8; }
.provider-icon[data-provider="google"] { color: #82aaff; }
.provider-icon[data-provider="deepseek"] { color: #62a7ff; }
.provider-icon[data-provider="groq"] { color: #f6d365; }
</style>
