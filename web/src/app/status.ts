import type { BridgeState } from '@/remote/protocol'

export const tokenCount = (value: unknown): string => typeof value === 'number'
  ? new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 }).format(value) : 'Not reported'

export function activeProvider(state: BridgeState) {
  const model = state.active_model.toLowerCase()
  const providers = state.auth_providers ?? []
  return providers.find(item => model.startsWith(item.provider.toLowerCase() + '/') || model.startsWith(item.provider.toLowerCase() + ':'))
    ?? (model.startsWith('gpt-') || model.startsWith('o3') || model.startsWith('o4') ? providers.find(item => item.provider === 'openai') : undefined)
    ?? (model.startsWith('claude-') ? providers.find(item => item.provider === 'anthropic') : undefined)
}

export function subscription(state: BridgeState): boolean {
  const provider = activeProvider(state)
  // OAuth alone is not proof of a subscription; use the provider's plan report.
  const plan = provider?.usage?.plan?.toLowerCase()
  return !!plan && !['api', 'payg', 'pay-as-you-go', 'metered', 'free'].includes(plan)
}

export function resetLabel(value: string | null | undefined): string {
  if (!value) return 'Reset time not reported'
  const date = new Date(value)
  return Number.isFinite(date.getTime()) ? 'Resets ' + date.toLocaleString() : 'Reset time unavailable'
}
