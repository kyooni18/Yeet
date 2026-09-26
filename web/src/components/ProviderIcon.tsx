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

const sources: Record<string, string> = {
  openai: openaiIcon,
  opencode: opencodeIcon,
  'codex-cli': codexIcon,
  codex: codexIcon,
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
const monochrome = new Set(['openai', 'opencode', 'anthropic', 'groq', 'ollama', 'vertex', 'xai', 'other'])

export function ProviderIcon({
  provider,
  size = 18,
  background = false,
}: {
  provider?: string | null
  size?: number
  background?: boolean
}) {
  const normalized = provider?.trim().toLowerCase() || 'other'
  const source = sources[normalized] ?? sources.other
  return (
    <span
      className={`provider-mark${background ? ' provider-mark--background' : ''}${monochrome.has(normalized) ? ' provider-mark--mono' : ''}`}
      data-provider={normalized}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      <img src={source} alt="" draggable={false} />
    </span>
  )
}
