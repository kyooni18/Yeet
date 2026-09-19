/** Reuse existing intent without model-generated narration or partial JSON guesses. */
export function toolPurpose(argumentsJson: string): string | null {
  try {
    const args: unknown = JSON.parse(argumentsJson)
    if (!args || typeof args !== 'object' || Array.isArray(args)) return null
    const purpose = (args as Record<string, unknown>).purpose
    if (typeof purpose !== 'string') return null
    const clean = purpose.replace(/[\u0000-\u001f\u007f-\u009f]/g, ' ').replace(/\s+/g, ' ').trim()
    if (!clean) return null
    const chars = Array.from(clean)
    return chars.length > 160 ? `${chars.slice(0, 159).join('')}…` : clean
  } catch {
    return null
  }
}
