const draftsByContext = new Map<string, string>()

export function readComposerDraft(contextKey: string): string {
  return draftsByContext.get(contextKey) ?? ''
}

export function writeComposerDraft(contextKey: string, value: string): void {
  if (value) draftsByContext.set(contextKey, value)
  else draftsByContext.delete(contextKey)
}
