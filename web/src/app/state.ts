import { State } from '@mun/core'
import { remoteStore } from '@/store/remoteStore'

/** Mirror of the shared RemoteStore snapshot; every View reads `remote.value`. */
export const remote = State(remoteStore.getSnapshot())
remoteStore.subscribe(() => { remote.value = remoteStore.getSnapshot() })

export type Page = 'chat' | 'files' | 'diff'
export const page = State<Page>('chat')
export const sidebarOpen = State(window.matchMedia('(min-width: 900px)').matches)
export const draft = State('')

export const connected = () => remote.value.connection === 'connected'

export function openPage(next: Page) {
  page.value = next
  if (next === 'files') remoteStore.requestFiles(remote.value.workspaceFiles?.path ?? '', null)
  if (next === 'diff') remoteStore.requestChanges(remote.value.workspaceChanges?.selected ?? null, false)
  if (window.matchMedia('(max-width: 899px)').matches) sidebarOpen.value = false
}

export function send() {
  const text = draft.value.trim()
  if (!text || !remoteStore.submit(text)) return
  draft.value = ''
}

export function age(iso: string): string {
  const seconds = Math.max(0, (Date.now() - Date.parse(iso)) / 1000)
  if (!Number.isFinite(seconds)) return ''
  if (seconds < 60) return 'now'
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}
