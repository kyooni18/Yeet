export function shortModelName(value: string): string {
  const clean = value.trim()
  if (!clean) return 'Model'
  const slash = clean.lastIndexOf('/')
  const name = slash >= 0 ? clean.slice(slash + 1) : clean
  return name
    .replace(/^openai:/i, '')
    .replace(/^anthropic:/i, '')
    .replace(/^google:/i, '')
}

export function reasoningName(value: string): string {
  switch (value.toLowerCase()) {
    case 'low': return 'Low'
    case 'medium': return 'Medium'
    case 'high': return 'High'
    case 'max': return 'Max'
    case 'xhigh':
    case 'extra-high':
    case 'extra_high': return 'Extra High'
    default: return 'Auto'
  }
}

export function formatTokens(value: number): string {
  if (!Number.isFinite(value)) return '0'
  const abs = Math.abs(value)
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(abs >= 10_000_000 ? 0 : 1)}M`
  if (abs >= 1_000) return `${(value / 1_000).toFixed(abs >= 100_000 ? 0 : 1)}K`
  return Math.round(value).toLocaleString()
}

export function formatReset(value?: string | null): string {
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const diff = date.getTime() - Date.now()
  if (diff <= 0) return 'resetting'
  const minutes = Math.round(diff / 60_000)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.round(minutes / 60)
  if (hours < 48) return `${hours}h`
  return `${Math.round(hours / 24)}d`
}

/** Keep the web effort selector aligned with the native model capabilities. */
export function reasoningLevelsForModel(model: string): string[] {
  const segments = model.split('/')
  const name = ['openrouter', 'opencode', 'opencode-go'].includes(segments[0] ?? '')
    ? segments.slice(segments.length > 2 ? 2 : 1).join('/') : segments.slice(1).join('/')
  const version = /^gpt-(\d+)(?:\.(\d+))?/.exec(name)
  const extended = (version && (Number(version[1]) > 5 || (Number(version[1]) === 5 && Number(version[2] ?? 0) >= 6)))
    || /^claude-(?:(?:fable|mythos|opus|sonnet)-5|opus-4[.-][78])/.test(name)
  return extended ? ['auto', 'low', 'medium', 'high', 'xhigh', 'max'] : ['auto', 'low', 'medium', 'high']
}
