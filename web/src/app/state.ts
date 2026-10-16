import { Animation, State, withAnimation } from '@mun/core'
import type { ToolbarControl } from '@/remote/protocol'
import { remoteStore } from '@/store/remoteStore'

/** Mirror of the shared RemoteStore snapshot; every View reads `remote.value`. */
export const remote = State(remoteStore.getSnapshot())

export type Page = 'chat' | 'files' | 'diff' | 'agents'
export type Sheet = 'none' | 'settings' | 'model'

const wide = () => window.matchMedia('(min-width: 900px)').matches

export const page = State<Page>('chat')
export const sheet = State<Sheet>('none')
export const sidebarOpen = State(wide())
export const isWide = State(wide())
export const workspaceMenu = State(false)
export const toast = State('')

export const ease = Animation.snappy(0.28)

export const connected = () => remote.value.connection === 'connected'

export function toggleSidebar(open = !sidebarOpen.value) {
  withAnimation(ease, () => { sidebarOpen.value = open })
}

export function openPage(next: Page) {
  page.value = next
  if (next === 'files') remoteStore.requestFiles(remote.value.workspaceFiles?.path ?? '', null)
  if (next === 'diff') remoteStore.requestChanges(remote.value.workspaceChanges?.selected ?? null, false)
  if (!isWide.value) toggleSidebar(false)
}

export function openSheet(next: Sheet) {
  withAnimation(ease, () => { sheet.value = next })
}

export function showToast(message: string) {
  withAnimation(ease, () => { toast.value = message })
  window.setTimeout(() => {
    if (toast.value === message) withAnimation(ease, () => { toast.value = '' })
  }, 1800)
}

export function toolbarControl(id: string): ToolbarControl | null {
  for (const group of remote.value.ui?.toolbar?.groups ?? []) {
    const control = group.controls.find(item => item.id === id)
    if (control) return control
  }
  return null
}

export function newSession() {
  if (remote.value.home) remoteStore.sendHomeUi({ type: 'new_session' })
  else remoteStore.newSession()
  openPage('chat')
}

export function openSession(id: string) {
  remoteStore.loadSession(id)
  openPage('chat')
}

export function chooseWorkspace(path: string, id: string) {
  const toolbar = remote.value.ui?.toolbar
  if (toolbar?.workspace_choices && toolbar.workspace_source) {
    remoteStore.sendToolbarUi({ type: 'choose_workspace', value: { id, source_workspace: toolbar.workspace_source } })
  } else {
    remoteStore.switchWorkspace(path)
  }
  workspaceMenu.value = false
}

export function workspaces(): { id: string; path: string; label: string; selected: boolean }[] {
  const toolbar = remote.value.ui?.toolbar
  if (toolbar?.workspace_choices) return toolbar.workspace_choices
  const current = remote.value.state.workspace_root
  return remote.value.state.known_workspaces.map(item => ({
    id: item.id, path: item.path, label: item.display_name, selected: item.is_current || item.path === current,
  }))
}

export function age(iso: string): string {
  const seconds = Math.max(0, (Date.now() - Date.parse(iso)) / 1000)
  if (!Number.isFinite(seconds)) return ''
  if (seconds < 60) return 'now'
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`
  return `${Math.floor(seconds / 86400)}d`
}

export function shortModel(model: string): string {
  return model.split('/').pop() || model
}

export function contextLabel(): string {
  const percent = remote.value.contextPercent
  return percent == null ? '' : `${Math.round(percent)}% context`
}

/** Keeps a scroll container pinned to the bottom while the user has not scrolled up. */
export function pinToBottom(element: HTMLElement) {
  let pinned = true
  const onScroll = () => { pinned = element.scrollHeight - element.scrollTop - element.clientHeight < 80 }
  const observer = new MutationObserver(() => { if (pinned) element.scrollTop = element.scrollHeight })
  element.addEventListener('scroll', onScroll, { passive: true })
  observer.observe(element, { childList: true, subtree: true, characterData: true })
  element.scrollTop = element.scrollHeight
  return () => { observer.disconnect(); element.removeEventListener('scroll', onScroll) }
}

async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text); showToast('Copied') }
  catch { showToast('Copy failed') }
}

let handledConversation = 0
let handledSettings = 0
let handledHome = 0

/** One-shot effects the Rust UI projections hand back to the browser. */
function consumeEffects(onEditDraft: (text: string) => void) {
  const snapshot = remote.value
  const conversation = snapshot.conversationEffect
  if (conversation && conversation.revision !== handledConversation) {
    handledConversation = conversation.revision
    const effect = remoteStore.consumeConversationEffect(conversation.revision)
    if (effect?.copy) void copyText(effect.copy.text)
    if (effect?.edit_draft != null) onEditDraft(effect.edit_draft)
  }
  const settings = snapshot.settingsEffect
  if (settings && settings.revision !== handledSettings) {
    handledSettings = settings.revision
    const effect = remoteStore.consumeSettingsEffect(settings.revision)
    if (effect?.destination === 'models') openSheet('model')
    else if (effect?.destination === 'agents') { openSheet('none'); openPage('agents') }
  }
  const home = snapshot.homeEffect
  if (home && home.id !== handledHome) {
    handledHome = home.id
    const target = remoteStore.consumeHomeEffect(home.id)
    if (target) openPage('chat')
  }
}

export function startRemote(onEditDraft: (text: string) => void) {
  remoteStore.subscribe(() => {
    remote.value = remoteStore.getSnapshot()
    consumeEffects(onEditDraft)
  })
  const media = window.matchMedia('(min-width: 900px)')
  media.addEventListener('change', () => {
    isWide.value = media.matches
    toggleSidebar(media.matches)
  })
  window.addEventListener('keydown', event => {
    if (event.key === 'Escape' && sheet.value !== 'none') openSheet('none')
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'b') { event.preventDefault(); toggleSidebar() }
  })
  remoteStore.init()
}
