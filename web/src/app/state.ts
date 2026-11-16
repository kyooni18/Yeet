import { Animation, State, withAnimation } from '@mun/core'
import type { ToolbarControl } from '@/remote/protocol'
import { remoteStore } from '@/store/remoteStore'
import { bindElement } from './dom'

/** Full snapshot for effects and small controls. Heavy views use stable slices. */
export const remote = State(remoteStore.getSnapshot())
export const connection = State(remote.value.connection)
export const runtimeState = State(remote.value.state)
export const conversationView = State(remote.value.conversation)
export const workspaceFiles = State(remote.value.workspaceFiles)
export const workspaceChanges = State(remote.value.workspaceChanges)
export const toolbarView = State(remote.value.ui?.toolbar)
export const currentSessions = State(remote.value.currentWorkspaceSessions)
export const activeAgents = State(remote.value.agents?.active_count ?? 0)
export const statusSnapshot = State(remote.value)
export const streaming = State(remote.value.state.is_streaming)

/** Session list entry. The client shows one list, never one list per workspace. */
export interface ListedSession {
  id: string
  title: string
  updated_at: string
  model: string
  message_count: number
  workspace_path: string
  workspace_label: string
}

function workspaceLabel(state: typeof remote.value.state, path: string): string {
  const workspace = state.known_workspaces.find(item => item.id === path || item.path === path)
  return workspace?.display_name || path.split('/').filter(Boolean).pop() || path
}

function sessionCatalogFrom(snapshot: typeof remote.value): ListedSession[] {
  const byId = new Map<string, ListedSession>()
  const add = (session: any, workspacePath: string) => {
    if (session?.id && !byId.has(session.id)) {
      byId.set(session.id, { ...session, workspace_path: workspacePath, workspace_label: workspaceLabel(snapshot.state, workspacePath) })
    }
  }
  for (const group of snapshot.state.workspace_session_groups ?? []) {
    const workspace = snapshot.state.known_workspaces.find(item => item.id === group.workspace_id)
    const path = workspace?.path || group.workspace_id
    for (const session of group.sessions) add(session, path)
  }
  // Servers without a workspace catalog still list the current workspace.
  if (!byId.size) for (const session of snapshot.currentWorkspaceSessions) add(session, snapshot.state.workspace_root || '')
  return [...byId.values()].sort((left, right) => (right.updated_at || '').localeCompare(left.updated_at || ''))
}

export const sessionCatalog = State(sessionCatalogFrom(remoteStore.getSnapshot()))

/** Only label rows when the single list actually mixes workspaces. */
export function spansWorkspaces(): boolean {
  return new Set(sessionCatalog.value.map(session => session.workspace_path)).size > 1
}

/** A cross-workspace pick waits for its workspace handshake before loading. */
let pendingSession: { id: string; workspace: string } | null = null
// One linear pass, not a findIndex for every item on every editor acknowledgement.
// Every projection arrives as freshly parsed JSON, so unchanged rows are reused
// by content: views keyed on an identical object skip re-rendering entirely,
// which keeps streaming cost proportional to what changed, not transcript size.
// Only the newest rows are mounted; older ones are revealed on request, so DOM
// size and per-update work stay bounded during long sessions.
const TRANSCRIPT_WINDOW = 120
type SpacedItem = NonNullable<typeof remote.value.conversation>['items'][number] & { spacing: string }
let spacedCache = new Map<string, { signature: string; item: SpacedItem }>()
let transcriptLimit = TRANSCRIPT_WINDOW
// Mün's keyed reconciler misplaces rows when a list is prepended to twice in a
// row, so each reveal remounts the window under fresh keys instead.
let transcriptGeneration = 0
export function transcriptKey(item: { id: string | number }): string {
  return transcriptGeneration + ':' + item.id
}
export const hiddenTranscriptCount = State(0)
function spacedItems(view: typeof remote.value.conversation): SpacedItem[] {
  const items = view?.items ?? []
  const start = Math.max(0, items.length - transcriptLimit)
  hiddenTranscriptCount.value = start
  const next = new Map<string, { signature: string; item: SpacedItem }>()
  const result: SpacedItem[] = []
  for (let index = start; index < items.length; index++) {
    const item = items[index]
    const spacing = !index || (item.type !== 'entry' && items[index - 1].type !== 'entry') ? '0' : '24px'
    const signature = spacing + JSON.stringify(item)
    const key = String(item.id)
    const cached = spacedCache.get(key)
    const entry = cached && cached.signature === signature ? cached : { signature, item: Object.freeze({ ...item, spacing }) }
    next.set(key, entry)
    result.push(entry.item)
  }
  spacedCache = next
  return result
}
export const transcriptItems = State(spacedItems(remote.value.conversation))

const TRANSCRIPT_ROWS = ':scope > .message-block, :scope > .activity-group, :scope > .reasoning-trace'
/** Reveal older rows without moving what the reader is looking at. */
export function showEarlierTranscript() {
  const element = conversationElement
  const content = element?.firstElementChild
  const top = (node: Element) => node.getBoundingClientRect().top
  const previousFirst = content?.querySelector(TRANSCRIPT_ROWS)
  const offset = element && previousFirst ? top(previousFirst) - top(element) : 0
  const hiddenBefore = hiddenTranscriptCount.value
  transcriptLimit += TRANSCRIPT_WINDOW
  transcriptGeneration += 1
  transcriptItems.value = spacedItems(conversationView.value)
  const revealed = hiddenBefore - hiddenTranscriptCount.value
  if (!element || !content || !previousFirst || revealed <= 0) return
  // The row that was first is now `revealed` rows down. Re-pin it for a few
  // frames: rows near the viewport take their real height once laid out.
  let frames = 4
  const settle = () => {
    const anchor = content.querySelectorAll(TRANSCRIPT_ROWS)[revealed]
    if (anchor) element.scrollTop += top(anchor) - top(element) - offset
    if (--frames > 0) requestAnimationFrame(settle)
  }
  requestAnimationFrame(settle)
}

function resetTranscriptWindow() {
  if (transcriptLimit === TRANSCRIPT_WINDOW) return
  transcriptLimit = TRANSCRIPT_WINDOW
  transcriptItems.value = spacedItems(conversationView.value)
}

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
export const inspectorTab = State<'status' | 'changes' | 'files'>('status')
export const toast = State('')
export const sessionQuery = State('')
/** Local draft, initialized only when the shared settings editor opens. */
export const settingsEditorValue = State('')
export const managedSession = State<{ id: string; title: string } | null>(null)

export const sessionTitle = State('')
export const sessionWorkspace = State('')
export const sessionConfirming = State(false)

export function manageSession(id: string, title: string) {
  sessionTitle.value = title
  sessionWorkspace.value = ''
  sessionConfirming.value = false
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

export const connected = () => connection.value === 'connected'

export function toggleSidebar(open = !sidebarOpen.value) {
  withAnimation(ease, () => {
    sidebarOpen.value = open
    if (open && !isWide.value) inspectorOpen.value = false
  })
}

export function refreshInspector() {
  if (!connected()) return
  if (inspectorTab.value === 'status') remoteStore.refreshProviderUsage()
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

export function selectInspectorTab(tab: 'status' | 'changes' | 'files') {
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
  for (const group of toolbarView.value?.groups ?? []) {
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

export function openSession(id: string, workspacePath?: string) {
  const target = (workspacePath ?? '').trim()
  if (target && target !== remote.value.state.workspace_root) {
    // A session belongs to its own workspace: land there before loading it.
    pendingSession = { id, workspace: target }
    remoteStore.switchWorkspace(target)
  } else {
    remoteStore.loadSession(id)
  }
  openPage('chat')
}

export function chooseWorkspace(path: string, id: string) {
  const toolbar = remote.value.ui?.toolbar
  if (toolbar?.workspace_choices && toolbar.workspace_source) {
    remoteStore.sendToolbarUi({ type: 'choose_workspace', value: { id, source_workspace: toolbar.workspace_source } })
  } else {
    remoteStore.switchWorkspace(path)
  }
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
  const sessions = sessionCatalog.value
  if (!needle) return sessions
  return sessions.filter(session => [session.title, session.model, session.workspace_label, session.workspace_path]
    .some(value => (value || '').toLowerCase().includes(needle)))
}

export function shortModel(model: string): string {
  return model.split('/').pop() || model
}

export function contextLabel(): string {
  const percent = statusSnapshot.value.contextPercent
  return percent == null ? '' : `${Math.round(percent)}% context`
}

export function contextDescription(): string {
  const state = runtimeState.value
  return state.current_context_tokens == null || state.active_model_context_length == null
    ? 'Context usage not reported'
    : `${state.current_context_tokens.toLocaleString()} / ${state.active_model_context_length.toLocaleString()} context tokens`
}

export function isNewChat(): boolean {
  const view = conversationView.value
  return !streaming.value && !view?.items.length && !view?.streaming_text
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
  // Streaming mutates the DOM in many tasks per frame, and reading scrollHeight
  // after each one forces a layout. Mutations settle once per frame instead.
  let followFrame = 0
  const scheduleFollow = () => {
    if (followFrame || !pinned) return
    followFrame = requestAnimationFrame(() => { followFrame = 0; follow() })
  }
  const onScroll = () => {
    pinned = element.scrollHeight - element.scrollTop - element.clientHeight < 80
    if (showJumpToLatest.value !== !pinned) showJumpToLatest.value = !pinned
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
    resetTranscriptWindow()
    pinned = true
    showJumpToLatest.value = false
    cancelAnimationFrame(frame)
    frame = requestAnimationFrame(follow)
  })
  const observer = new MutationObserver(scheduleFollow)
  // Resize callbacks already run once per frame on a clean layout. Following
  // there, before paint, means the next scroll event sees a settled position
  // and a replaced transcript cannot unpin the reader.
  const resize = new ResizeObserver(follow)
  if (element.firstElementChild) resize.observe(element.firstElementChild)
  resize.observe(element)
  element.addEventListener('scroll', onScroll, { passive: true })
  observer.observe(element, { childList: true, subtree: true, characterData: true })
  follow()
  return () => {
    unsubscribe()
    cancelAnimationFrame(frame)
    cancelAnimationFrame(followFrame)
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
  let priorSnapshot = remoteStore.getSnapshot()
  let catalogInput: { groups: unknown; sessions: unknown; workspaces: unknown } = { groups: null, sessions: null, workspaces: null }
  remoteStore.subscribe(() => {
    // Compare raw store references, not Mün's reactive proxies.
    const previous = priorSnapshot
    const previousEditor = previous.settings?.editor
    const snapshot = remoteStore.getSnapshot()
    priorSnapshot = snapshot
    const editor = snapshot.settings?.editor
    if (editor && (!previousEditor || previousEditor.kind !== editor.kind)) {
      settingsEditorValue.value = editor.value
    } else if (!editor && previousEditor) {
      settingsEditorValue.value = ''
    }
    // The store replaces these containers instead of mutating them. A shallow
    // freeze stops Mün's ownership walk at their root, so a streaming update no
    // longer costs a traversal of every transcript entry and projected row.
    if (snapshot.entries && !Object.isFrozen(snapshot.entries)) Object.freeze(snapshot.entries)
    if (snapshot.conversation && !Object.isFrozen(snapshot.conversation)) Object.freeze(snapshot.conversation)
    remote.value = snapshot
    connection.value = snapshot.connection
    runtimeState.value = snapshot.state
    workspaceFiles.value = snapshot.workspaceFiles
    workspaceChanges.value = snapshot.workspaceChanges
    toolbarView.value = snapshot.ui?.toolbar
    if (catalogInput.groups !== snapshot.state.workspace_session_groups
      || catalogInput.sessions !== snapshot.currentWorkspaceSessions
      || catalogInput.workspaces !== snapshot.state.known_workspaces) {
      catalogInput = {
        groups: snapshot.state.workspace_session_groups,
        sessions: snapshot.currentWorkspaceSessions,
        workspaces: snapshot.state.known_workspaces,
      }
      sessionCatalog.value = sessionCatalogFrom(snapshot)
    }
    if (pendingSession && snapshot.connection === 'connected' && snapshot.state.workspace_root === pendingSession.workspace) {
      const pending = pendingSession
      pendingSession = null
      remoteStore.loadSession(pending.id)
    }
    currentSessions.value = snapshot.currentWorkspaceSessions
    activeAgents.value = snapshot.agents?.active_count ?? 0
    streaming.value = snapshot.state.is_streaming
    if (snapshot.conversation !== previous.conversation) {
      conversationView.value = snapshot.conversation
      transcriptItems.value = spacedItems(snapshot.conversation)
    }
    if (snapshot.state !== previous.state || snapshot.connection !== previous.connection || snapshot.agents !== previous.agents) {
      statusSnapshot.value = snapshot
    }
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
