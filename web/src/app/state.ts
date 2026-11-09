import { Animation, State, withAnimation } from '@mun/core'
import type { ToolbarControl } from '@/remote/protocol'
import { remoteStore } from '@/store/remoteStore'
import { bindElement } from './dom'

/** Mirror of the shared RemoteStore snapshot; every View reads `remote.value`. */
export const remote = State(remoteStore.getSnapshot())

export type Page = 'chat' | 'files' | 'diff' | 'agents' | 'tasks' | 'debate' | 'extensions'
export type Sheet = 'none' | 'settings' | 'model' | 'reasoning' | 'permissions' | 'auth' | 'providers' | 'capabilities' | 'agent-settings' | 'session' | 'workspace'

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
export const managedSession = State<{ id: string; title: string } | null>(null)

export function manageSession(id: string, title: string) {
  managedSession.value = { id, title }
  openSheet('session')
}

/** Route every destination advertised by the Remote host to a real surface. */
export function navigateDestination(destination: string) {
  if (destination === 'models') openSheet('model')
  else if (destination === 'sessions') { openSheet('none'); openPage('chat'); toggleSidebar(true) }
  else if (destination === 'agents') { openSheet('none'); openPage('agents') }
  else if (['settings', 'reasoning', 'permissions', 'auth', 'providers', 'capabilities'].includes(destination)) openSheet(destination as Sheet)
}

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

// A short, non-bouncy curve keeps navigation and disclosures responsive.
export const ease = Animation.easeOut(0.2)

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
  if (next === 'tasks') remoteStore.send({ type: 'request_tasks' })
  if (!isWide.value) toggleSidebar(false)
  if (!isDesktop.value && inspectorOpen.value) toggleInspector(false)
}

export function openSheet(next: Sheet) {
  if (next === 'settings') remoteStore.sendSettingsUi({ type: 'refresh' })
  if (next === 'model') remoteStore.send({ type: 'request_models' })
  if (next === 'auth') remoteStore.send({ type: 'request_auth' })
  if (next === 'providers') remoteStore.send({ type: 'request_providers' })
  if (next === 'capabilities') remoteStore.send({ type: 'request_capabilities' })
  if (next === 'permissions') remoteStore.send({ type: 'request_sandbox' })
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

export const showJumpToLatest = State(false)
let conversationElement: HTMLElement | null = null

export function jumpToLatest() {
  if (!conversationElement) return
  conversationElement.scrollTop = conversationElement.scrollHeight
  conversationElement.focus({ preventScroll: true })
  showJumpToLatest.value = false
}

/** Follow output (including shadow-DOM markdown growth), never drag a reader down. */
function pinToBottom(element: HTMLElement) {
  conversationElement = element
  showJumpToLatest.value = false
  let pinned = true
  const follow = () => { if (pinned) element.scrollTop = element.scrollHeight }
  const onScroll = () => {
    pinned = element.scrollHeight - element.scrollTop - element.clientHeight < 80
    showJumpToLatest.value = !pinned
  }
  const context = () => {
    const snapshot = remoteStore.getSnapshot()
    return `${snapshot.state.workspace_root}:${snapshot.state.current_session_id}:${snapshot.sessionResetRevision}`
  }
  let lastContext = context()
  let frame = 0
  const unsubscribe = remoteStore.subscribe(() => {
    const next = context()
    if (next === lastContext) return
    lastContext = next
    pinned = true
    showJumpToLatest.value = false
    cancelAnimationFrame(frame)
    frame = requestAnimationFrame(follow)
  })
  const observer = new MutationObserver(follow)
  const resize = new ResizeObserver(follow)
  if (element.firstElementChild) resize.observe(element.firstElementChild)
  resize.observe(element)
  element.addEventListener('scroll', onScroll, { passive: true })
  observer.observe(element, { childList: true, subtree: true, characterData: true })
  follow()
  return () => {
    unsubscribe()
    cancelAnimationFrame(frame)
    observer.disconnect()
    resize.disconnect()
    element.removeEventListener('scroll', onScroll)
    if (conversationElement === element) conversationElement = null
  }
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
    if (effect?.destination) navigateDestination(effect.destination)
  }
  const home = snapshot.homeEffect
  if (home && home.id !== handledHome) {
    handledHome = home.id
    const target = remoteStore.consumeHomeEffect(home.id)
    if (target) openPage('chat')
  }
}

export function startRemote(onEditDraft: (text: string) => void) {
  // Safari can scroll the document as well as pan the visual viewport.
  // Anchor the shell in document coordinates rather than fighting caret scroll.
  const viewport = window.visualViewport
  let restingHeight = window.innerHeight
  let keyboardOpen = false
  const sizeViewport = () => {
    const root = document.documentElement
    const editing = document.activeElement?.matches('.composer textarea') ?? false
    if (!editing && !keyboardOpen) restingHeight = window.innerHeight
    keyboardOpen = (editing || keyboardOpen) && Math.max(restingHeight, window.innerHeight) - (viewport?.height ?? window.innerHeight) > 80
    root.dataset.keyboardOpen = keyboardOpen ? 'true' : 'false'
    root.style.setProperty('--app-height', `${viewport?.height ?? window.innerHeight}px`)
    root.style.setProperty('--app-width', `${viewport?.width ?? window.innerWidth}px`)
    root.style.setProperty('--app-top', `${viewport?.pageTop ?? window.scrollY}px`)
    root.style.setProperty('--app-left', `${viewport?.pageLeft ?? window.scrollX}px`)
  }
  viewport?.addEventListener('resize', sizeViewport)
  viewport?.addEventListener('scroll', sizeViewport)
  window.addEventListener('resize', sizeViewport)
  window.addEventListener('scroll', sizeViewport, { passive: true })
  // Safari can report the final keyboard geometry after its focus event.
  let settleFrame = 0
  const settleViewport = () => {
    cancelAnimationFrame(settleFrame)
    const until = performance.now() + 1000
    const tick = () => {
      sizeViewport()
      if (performance.now() < until) settleFrame = requestAnimationFrame(tick)
    }
    tick()
  }
  document.addEventListener('focusin', settleViewport)
  document.addEventListener('focusout', settleViewport)
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
