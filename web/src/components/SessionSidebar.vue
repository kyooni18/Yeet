<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import type { SessionSummary, WorkspaceSummary } from '@/remote/protocol'
import { useRemoteStore } from '@/stores/remote'
import YeetMark from './YeetMark.vue'

const props = withDefaults(defineProps<{ drawer?: boolean }>(), { drawer: false })
const emit = defineEmits<{ close: [] }>()
const remote = useRemoteStore()
const router = useRouter()
const workspacePathOpen = ref(false)
const workspacePath = ref('')
const workspaceAddButton = ref<HTMLButtonElement | null>(null)
const workspacePathInput = ref<HTMLInputElement | null>(null)
const pendingSession = ref<{ workspacePath: string; sessionId: string } | null>(null)
const expandedWorkspaceIds = ref(new Set<string>())
const SESSION_PREVIEW_LIMIT = 4
const filterQuery = ref('')
const FILTER_DISCOVERY_THRESHOLD = 8

const sessionGroups = computed(() => new Map(
  remote.state.workspace_session_groups.map((group) => [group.workspace_id, group.sessions] as const),
))

const isCurrentWorkspace = (workspace: WorkspaceSummary) =>
  workspace.is_current || workspace.path === remote.state.workspace_root

const sortSessions = (sessions: SessionSummary[]) => [...sessions].sort((a, b) =>
  Date.parse(b.updated_at) - Date.parse(a.updated_at),
)

function currentSessionPreview(sessions: SessionSummary[]) {
  const recent = sessions.slice(0, SESSION_PREVIEW_LIMIT)
  const currentId = remote.state.current_session_id
  if (!currentId || recent.some((session) => session.id === currentId)) return recent

  const current = sessions.find((session) => session.id === currentId)
  if (!current) return recent
  return [...recent.slice(0, SESSION_PREVIEW_LIMIT - 1), current]
}

const workspaceCatalog = computed(() => remote.knownWorkspaces.map((workspace) => {
  const semanticSessions = sessionGroups.value.get(workspace.id)
  const sessions = sortSessions(semanticSessions ?? (isCurrentWorkspace(workspace) ? remote.state.saved_sessions : []))
  const collapsedSessions = isCurrentWorkspace(workspace) ? currentSessionPreview(sessions) : []
  const expanded = expandedWorkspaceIds.value.has(workspace.id)

  return {
    workspace,
    sessions,
    visibleSessions: expanded ? sessions : collapsedSessions,
    expanded,
    canToggleSessions: sessions.length > collapsedSessions.length,
  }
}))

const normalizedFilter = computed(() => filterQuery.value.trim().toLocaleLowerCase())
const showWorkspaceFilter = computed(() => {
  const sessionCount = remote.knownWorkspaces.reduce((sum, workspace) => sum + workspace.session_count, 0)
  return Boolean(normalizedFilter.value) || remote.knownWorkspaces.length >= 4 || sessionCount >= FILTER_DISCOVERY_THRESHOLD
})
const filteredWorkspaceCatalog = computed(() => {
  const query = normalizedFilter.value
  if (!query) return workspaceCatalog.value

  return workspaceCatalog.value.flatMap((item) => {
    const workspaceMatches = `${item.workspace.display_name}\n${item.workspace.path}`.toLocaleLowerCase().includes(query)
    const matchingSessions = item.sessions.filter((session) =>
      (session.title || 'Untitled session').toLocaleLowerCase().includes(query),
    )
    if (!workspaceMatches && !matchingSessions.length) return []

    return [{
      ...item,
      visibleSessions: matchingSessions,
      canToggleSessions: false,
    }]
  })
})

const relativeTime = (value?: string | null) => {
  if (!value) return ''
  const time = Date.parse(value)
  if (!Number.isFinite(time)) return ''
  const minutes = Math.floor((Date.now() - time) / 60_000)
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h`
  return `${Math.floor(hours / 24)}d`
}

const chatCount = (count: number) => `${count} ${count === 1 ? 'chat' : 'chats'}`

function chooseSession(workspace: WorkspaceSummary, id: string) {
  if (isCurrentWorkspace(workspace)) {

    if (id === remote.state.current_session_id) {
      if (props.drawer) emit('close')
      return
    }
    if (!remote.loadSession(id)) return
    emit('close')
    return
  }

  pendingSession.value = { workspacePath: workspace.path, sessionId: id }
  remote.selectWorkspace(workspace)
}

function createSession() {
  if (remote.connection !== 'connected' || !remote.newSession()) return
  pendingSession.value = null
  emit('close')
}

function chooseWorkspace(workspace: WorkspaceSummary) {
  if (isCurrentWorkspace(workspace)) return
  pendingSession.value = null
  remote.selectWorkspace(workspace)
  if (props.drawer) emit('close')
}

function toggleWorkspaceSessions(workspace: WorkspaceSummary) {
  const next = new Set(expandedWorkspaceIds.value)
  if (next.has(workspace.id)) next.delete(workspace.id)
  else next.add(workspace.id)
  expandedWorkspaceIds.value = next
}

function workspaceSessionToggleLabel(workspace: WorkspaceSummary, expanded: boolean) {
  return `${expanded ? 'Collapse' : 'Show'} chats for ${workspace.display_name}`
}

function clearFilter() {
  filterQuery.value = ''
}

function handleFilterEscape(event: KeyboardEvent) {
  if (!filterQuery.value) return
  event.preventDefault()
  event.stopPropagation()
  clearFilter()
}

function handleWorkspaceListKeydown(event: KeyboardEvent) {
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return

  const list = event.currentTarget
  const target = event.target
  if (!(list instanceof HTMLElement) || !(target instanceof HTMLElement)) return

  const controls = Array.from(list.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'))
    .filter((button) => button.getClientRects().length > 0)
  if (!controls.length) return

  const current = target.closest('button')
  const currentIndex = current instanceof HTMLButtonElement ? controls.indexOf(current) : -1
  let nextIndex = currentIndex
  if (event.key === 'Home') nextIndex = 0
  else if (event.key === 'End') nextIndex = controls.length - 1
  else if (event.key === 'ArrowDown') nextIndex = currentIndex < 0 ? 0 : Math.min(currentIndex + 1, controls.length - 1)
  else if (event.key === 'ArrowUp') nextIndex = currentIndex < 0 ? controls.length - 1 : Math.max(currentIndex - 1, 0)

  const next = controls[nextIndex]
  event.preventDefault()
  if (!next || next === current) return
  next.focus({ preventScroll: true })
}

async function openWorkspacePathEditor() {
  workspacePath.value = ''
  workspacePathOpen.value = true
  await nextTick()
  workspacePathInput.value?.focus()
}

async function closeWorkspacePathEditor() {
  workspacePathOpen.value = false
  await nextTick()
  workspaceAddButton.value?.focus()
}

function toggleWorkspacePath() {
  if (workspacePathOpen.value) {
    void closeWorkspacePathEditor()
    return
  }
  void openWorkspacePathEditor()
}

function openWorkspacePath() {
  const path = workspacePath.value.trim()
  if (!path) return
  pendingSession.value = null
  workspacePathOpen.value = false
  remote.switchWorkspace(path)
  if (props.drawer) emit('close')
}

function openSettings() {
  void router.push('/settings')
  if (props.drawer) emit('close')
}

watch(
  () => [
    remote.connection,
    remote.state.workspace_root ?? '',
    remote.state.saved_sessions.map((session) => session.id).join('\u0000'),
    remote.state.known_workspaces.length,
  ] as const,
  () => {
    const pending = pendingSession.value
    if (!pending) return
    if (remote.connection !== 'connected' || remote.state.workspace_root !== pending.workspacePath) return
    if (!remote.state.known_workspaces.length) return

    const sessionExists = remote.state.saved_sessions.some((session) => session.id === pending.sessionId)
    if (!sessionExists) return
    if (!remote.loadSession(pending.sessionId)) return
    pendingSession.value = null
    if (props.drawer) emit('close')
  },
)
</script>

<template>
  <aside class="session-sidebar" :class="{ 'is-drawer': props.drawer }" data-testid="session-sidebar">
    <div class="sidebar-brand">
      <YeetMark :size="26" />
      <strong>Yeet</strong>
      <button v-if="drawer" class="icon-button sidebar-close" aria-label="Close sessions" @click="emit('close')">×</button>
    </div>

    <button
      class="new-session-button"
      data-testid="new-session"
      :disabled="remote.connection !== 'connected'"
      @click="createSession"
    >
      <span aria-hidden="true">＋</span><span>New chat</span>
    </button>

    <div class="sidebar-navigation">
      <div class="workspace-heading">
        <span>Workspaces</span>
        <button
          ref="workspaceAddButton"
          class="workspace-add-button"
          type="button"
          data-testid="open-workspace-path"
          :aria-label="workspacePathOpen ? 'Cancel open workspace' : 'Open workspace by path'"
          aria-controls="workspace-open-form"
          :aria-expanded="workspacePathOpen"
          :title="workspacePathOpen ? 'Cancel open workspace' : 'Open workspace by path'"
          @click="toggleWorkspacePath"
        >{{ workspacePathOpen ? '×' : '＋' }}</button>
      </div>

      <form
        v-if="workspacePathOpen"
        id="workspace-open-form"
        class="workspace-open-form"
        data-testid="workspace-open-form"
        @submit.prevent="openWorkspacePath"
        @keydown.esc.stop.prevent="closeWorkspacePathEditor"
      >
        <input
          ref="workspacePathInput"
          v-model="workspacePath"
          data-testid="workspace-path-input"
          aria-label="Workspace path"
          autocomplete="off"
          spellcheck="false"
          placeholder="~/Code/project"
        />
        <button type="submit" :disabled="!workspacePath.trim()">Open</button>
      </form>

      <div v-if="showWorkspaceFilter" class="workspace-filter-shell">
        <input
          v-model="filterQuery"
          type="search"
          data-testid="workspace-filter"
          aria-label="Find workspace or chat"
          autocomplete="off"
          spellcheck="false"
          placeholder="Find workspace or chat…"
          @keydown.esc="handleFilterEscape"
        />
      </div>

      <nav class="workspace-list" aria-label="Workspaces" data-testid="workspace-list" @keydown="handleWorkspaceListKeydown">
        <section
          v-for="item in filteredWorkspaceCatalog"
          :key="item.workspace.id"
          class="workspace-group"
          :class="{ active: isCurrentWorkspace(item.workspace) }"
          :data-workspace-id="item.workspace.id"
        >
          <div class="workspace-row">
            <button
              class="workspace-item"
              :class="{ active: isCurrentWorkspace(item.workspace) }"
              type="button"
              data-testid="workspace-item"
              :data-workspace-path="item.workspace.path"
              :title="item.workspace.path"
              :aria-current="isCurrentWorkspace(item.workspace) ? 'location' : undefined"
              @click="chooseWorkspace(item.workspace)"
            >
              <span class="workspace-indicator" aria-hidden="true"></span>
              <span class="workspace-copy">
                <span class="workspace-name truncate">{{ item.workspace.display_name }}</span>
                <span class="workspace-meta">
                  {{ chatCount(item.workspace.session_count) }}<template v-if="relativeTime(item.workspace.updated_at)"> · {{ relativeTime(item.workspace.updated_at) }}</template>
                </span>
              </span>
            </button>
            <button
              v-if="item.canToggleSessions"
              class="workspace-session-toggle"
              type="button"
              data-testid="workspace-session-toggle"
              :aria-expanded="item.expanded"
              :aria-label="workspaceSessionToggleLabel(item.workspace, item.expanded)"
              :title="workspaceSessionToggleLabel(item.workspace, item.expanded)"
              @click.stop="toggleWorkspaceSessions(item.workspace)"
            >
              <span class="workspace-session-chevron" aria-hidden="true">›</span>
            </button>
          </div>

          <div
            v-if="item.visibleSessions.length || (isCurrentWorkspace(item.workspace) && !item.sessions.length)"
            class="workspace-sessions"
            :data-testid="isCurrentWorkspace(item.workspace) ? 'active-workspace-sessions' : 'workspace-sessions'"
            :data-active="isCurrentWorkspace(item.workspace) ? 'true' : 'false'"
          >
            <button
              v-for="session in item.visibleSessions"
              :key="session.id"
              class="session-item"
              :class="{ active: isCurrentWorkspace(item.workspace) && session.id === remote.state.current_session_id }"
              data-testid="session-item"
              :data-session-id="session.id"
              :aria-current="isCurrentWorkspace(item.workspace) && session.id === remote.state.current_session_id ? 'page' : undefined"
              :disabled="isCurrentWorkspace(item.workspace) && remote.connection !== 'connected'"
              :title="session.title || 'Untitled session'"
              @click="chooseSession(item.workspace, session.id)"
            >
              <span class="session-title">{{ session.title || 'Untitled session' }}</span>
              <span class="session-meta">{{ relativeTime(session.updated_at) }}</span>
            </button>
            <div v-if="isCurrentWorkspace(item.workspace) && !item.sessions.length" class="empty-sidebar">No saved chats yet.</div>
          </div>
        </section>

        <div v-if="!workspaceCatalog.length" class="empty-sidebar workspace-empty">
          {{ remote.connection === 'connected' ? 'No known workspaces yet.' : 'Loading workspaces…' }}
        </div>
        <div v-else-if="normalizedFilter && !filteredWorkspaceCatalog.length" class="empty-sidebar workspace-empty" data-testid="workspace-filter-empty">
          No matching workspaces or chats.
        </div>
      </nav>
    </div>

    <div class="sidebar-footer">
      <button @click="openSettings"><span aria-hidden="true">⚙</span><span>Settings</span></button>
    </div>
  </aside>
</template>


<style scoped>
.workspace-filter-shell {
  margin: 0 2px 7px;
}

.workspace-filter-shell input {
  width: 100%;
  min-width: 0;
  min-height: 34px;
  height: 34px;
  padding: 0 9px;
  border-color: var(--border);
  background: rgba(255, 255, 255, .025);
  color: var(--soft);
  font-size: 11px;
}

.workspace-filter-shell input::placeholder {
  color: var(--dim);
}

.session-sidebar.is-drawer .workspace-filter-shell input {
  min-height: 44px;
  height: 44px;
  font-size: 16px;
}

@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .new-session-button,
  .workspace-item,
  .session-item,
  .sidebar-footer button {
    min-height: 44px;
  }

  .workspace-heading { min-height: 44px; }
  .workspace-add-button,
  .workspace-session-toggle {
    width: 44px;
    height: 44px;
  }
  .workspace-row { grid-template-columns: minmax(0, 1fr) 44px; }

  .workspace-open-form input,
  .workspace-open-form button,
  .workspace-filter-shell input {
    min-height: 44px;
    height: 44px;
  }
  .workspace-open-form input,
  .workspace-filter-shell input {
    font-size: 16px;
  }
}
</style>
