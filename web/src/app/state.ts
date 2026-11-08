import { Animation, State, withAnimation } from '@mun/core'
import type { ToolbarControl } from '@/remote/protocol'
import { remoteStore } from '@/store/remoteStore'
import { bindElement } from './dom'

/** Mirror of the shared RemoteStore snapshot; every View reads `remote.value`. */
export const remote = State(remoteStore.getSnapshot())

export type Page = 'chat' | 'files' | 'diff' | 'agents'
export type Sheet = 'none' | 'settings' | 'model'

const wide = () => window.matchMedia('(min-width: 900px)').matches
const desktop = () => window.matchMedia('(min-width: 1280px)').matches
function panelPreference(): boolean | null {
  try {
    const saved = localStorage.getItem('yeet.remote.inspector')
    return saved == null ? null : saved === 'open'
  } catch { return null }
}
let preferredInspector = panelPreference()

export const page = State<Page>('chat')
export const sheet = State<Sheet>('none')
export const sidebarOpen = State(wide())
export const isWide = State(wide())
export const isDesktop = State(desktop())
export const inspectorOpen = State(desktop() && (preferredInspector ?? true))
export const inspectorTab = State<'changes' | 'files'>('changes')
export const workspaceMenu = State(false)
export const toast = State('')
export const sessionQuery = State('')
/** Local draft, initialized only when the shared settings editor opens. */
export const settingsEditorValue = State('')

/** Spinner frames shared with the TUI (`| / - \` every 180 ms). */
const FRAMES = ['|', '/', '-', '\\']
export const tick = State(0)
let runStartedAt = 0

export function isWorking(): boolean {
  const snapshot = remote.value
  return snapshot.state.is_streaming || !!snapshot.conversation?.preparing || (snapshot.agents?.active_count ?? 0) > 0
}

export function spinnerFrame(): string {
  return FRAMES[tick.value % FRAMES.length]
}

export function elapsed(): string {
  if (!runStartedAt) return ''
  const seconds = Math.max(0, Math.floor((Date.now() - runStartedAt) / 1000))
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

window.setInterval(() => {
  const working = isWorking()
  if (working && !runStartedAt) runStartedAt = Date.now()
  if (!working) runStartedAt = 0
  if (working) tick.value += 1
}, 180)

export const ease = Animation.snappy(0.28)

export const connected = () => remote.value.connection === 'connected'

export function toggleSidebar(open = !sidebarOpen.value) {
  withAnimation(ease, () => {
    sidebarOpen.value = open
    if (open && !isWide.value) inspectorOpen.value = false
  })
}

export function refreshInspector() {
  if (!connected()) return
  remoteStore.requestChanges(remote.value.workspaceChanges?.selected ?? null, false)
  if (inspectorTab.value === 'files') remoteStore.requestFiles(remote.value.workspaceFiles?.path ?? '', remote.value.workspaceFiles?.selected ?? null)
}

export function toggleInspector(open = !inspectorOpen.value) {
  preferredInspector = open
  try { localStorage.setItem('yeet.remote.inspector', open ? 'open' : 'closed') } catch { /* Private browsing. */ }
  withAnimation(ease, () => {
    inspectorOpen.value = open
    if (open && !isWide.value) sidebarOpen.value = false
  })
  if (open) refreshInspector()
}

export function selectInspectorTab(tab: 'changes' | 'files') {
  inspectorTab.value = tab
  if (tab === 'files' && connected()) remoteStore.requestFiles(remote.value.workspaceFiles?.path ?? '', remote.value.workspaceFiles?.selected ?? null)
}

export function openPage(next: Page) {
  page.value = next
  if (next === 'files') remoteStore.requestFiles(remote.value.workspaceFiles?.path ?? '', null)
  if (next === 'diff') remoteStore.requestChanges(remote.value.workspaceChanges?.selected ?? null, false)
  if (!isWide.value) toggleSidebar(false)
  if (!isDesktop.value && inspectorOpen.value) toggleInspector(false)
}

export function openSheet(next: Sheet) {
  if (next === 'settings') remoteStore.sendSettingsUi({ type: 'refresh' })
  if (next !== 'none') {
    if (!isWide.value) toggleSidebar(false)
    if (!isDesktop.value && inspectorOpen.value) toggleInspector(false)
  }
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

/** `/Users/name/Code/Yeet` → `~/Code/Yeet` for headers. */
export function displayPath(path: string | null | undefined): string {
  return (path ?? '').replace(/^\/(Users|home)\/[^/]+/, '~')
}

export function workspaceLine(): string {
  const snapshot = remote.value
  const path = displayPath(snapshot.currentWorkspace?.path || snapshot.state.workspace_root)
  const branch = snapshot.workspaceFiles?.branch || snapshot.workspaceChanges?.branch || ''
  return [path, branch].filter(Boolean).join(' · ')
}

export function filteredSessions() {
  const needle = sessionQuery.value.trim().toLowerCase()
  const sessions = remote.value.currentWorkspaceSessions
  return needle ? sessions.filter(session => (session.title || '').toLowerCase().includes(needle)) : sessions
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

export const conversationRef = bindElement(pinToBottom)

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
  // Follow the visible viewport rather than placing the composer under the iOS keyboard.
  const viewport = window.visualViewport
  const sizeViewport = () => document.documentElement.style.setProperty('--app-height', `${viewport?.height ?? window.innerHeight}px`)
  viewport?.addEventListener('resize', sizeViewport)
  window.addEventListener('resize', sizeViewport)
  sizeViewport()
  let lastWorkspace = remote.value.state.workspace_root
  let wasStreaming = remote.value.state.is_streaming
  let refreshFrame = 0
  remoteStore.subscribe(() => {
    const previous = remote.value
    const previousEditor = remote.value.settings?.editor
    const snapshot = remoteStore.getSnapshot()
    const editor = snapshot.settings?.editor
    if (editor && (!previousEditor || previousEditor.kind !== editor.kind)) {
      settingsEditorValue.value = editor.value
    } else if (!editor && previousEditor) {
      settingsEditorValue.value = ''
    }
    remote.value = snapshot
    if (inspectorOpen.value && snapshot.connection === 'connected' && (
      previous.connection !== 'connected' || lastWorkspace !== snapshot.state.workspace_root ||
      (wasStreaming && !snapshot.state.is_streaming)
    )) {
      // Welcome clears request cursors in the store's frame-batched queue.
      // Issue reads after that batch, not synchronously from onOpen.
      cancelAnimationFrame(refreshFrame)
      refreshFrame = requestAnimationFrame(() => { if (inspectorOpen.value) refreshInspector() })
    }
    lastWorkspace = snapshot.state.workspace_root
    wasStreaming = snapshot.state.is_streaming
    consumeEffects(onEditDraft)
  })
  const media = window.matchMedia('(min-width: 900px)')
  media.addEventListener('change', () => {
    isWide.value = media.matches
    toggleSidebar(media.matches)
  })
  const desktopMedia = window.matchMedia('(min-width: 1280px)')
  desktopMedia.addEventListener('change', () => {
    isDesktop.value = desktopMedia.matches
    withAnimation(ease, () => { inspectorOpen.value = desktopMedia.matches && (preferredInspector ?? true) })
    if (inspectorOpen.value) refreshInspector()
  })
  window.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      if (sheet.value !== 'none') openSheet('none')
      else if (inspectorOpen.value && !isDesktop.value) toggleInspector(false)
      else if (sidebarOpen.value && !isWide.value) toggleSidebar(false)
    }
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'b') {
      event.preventDefault()
      if (event.shiftKey) toggleInspector()
      else toggleSidebar()
    }
  })
  remoteStore.init()
}
