<script setup lang="ts">
import { computed, ref, watch } from 'vue'
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
const pendingSession = ref<{ workspacePath: string; sessionId: string } | null>(null)

const sessionGroups = computed(() => new Map(
  remote.state.workspace_session_groups.map((group) => [group.workspace_id, group.sessions] as const),
))

const isCurrentWorkspace = (workspace: WorkspaceSummary) =>
  workspace.is_current || workspace.path === remote.state.workspace_root

const sortSessions = (sessions: SessionSummary[]) => [...sessions].sort((a, b) =>
  Date.parse(b.updated_at) - Date.parse(a.updated_at),
)

const workspaceCatalog = computed(() => remote.knownWorkspaces.map((workspace) => {
  const semanticSessions = sessionGroups.value.get(workspace.id)
  const sessions = semanticSessions ?? (isCurrentWorkspace(workspace) ? remote.state.saved_sessions : [])
  return { workspace, sessions: sortSessions(sessions) }
}))

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
    remote.loadSession(id)
    emit('close')
    return
  }

  pendingSession.value = { workspacePath: workspace.path, sessionId: id }
  remote.selectWorkspace(workspace)
}

function createSession() {
  pendingSession.value = null
  remote.newSession()
  emit('close')
}

function chooseWorkspace(workspace: WorkspaceSummary) {
  if (isCurrentWorkspace(workspace)) return
  pendingSession.value = null
  remote.selectWorkspace(workspace)
  if (props.drawer) emit('close')
}

function toggleWorkspacePath() {
  workspacePathOpen.value = !workspacePathOpen.value
  if (workspacePathOpen.value) workspacePath.value = ''
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
    pendingSession.value = null
    remote.loadSession(pending.sessionId)
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

    <button class="new-session-button" data-testid="new-session" @click="createSession">
      <span aria-hidden="true">＋</span><span>New chat</span>
    </button>

    <div class="sidebar-navigation">
      <div class="workspace-heading">
        <span>Workspaces</span>
        <button
          class="workspace-add-button"
          type="button"
          data-testid="open-workspace-path"
          aria-label="Open workspace by path"
          :aria-expanded="workspacePathOpen"
          @click="toggleWorkspacePath"
        >＋</button>
      </div>

      <form v-if="workspacePathOpen" class="workspace-open-form" data-testid="workspace-open-form" @submit.prevent="openWorkspacePath">
        <input
          v-model="workspacePath"
          data-testid="workspace-path-input"
          aria-label="Workspace path"
          autocomplete="off"
          spellcheck="false"
          placeholder="~/Code/project"
          autofocus
        />
        <button type="submit" :disabled="!workspacePath.trim()">Open</button>
      </form>

      <nav class="workspace-list" aria-label="Workspaces" data-testid="workspace-list">
        <section
          v-for="item in workspaceCatalog"
          :key="item.workspace.id"
          class="workspace-group"
          :class="{ active: isCurrentWorkspace(item.workspace) }"
          :data-workspace-id="item.workspace.id"
        >
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

          <div
            v-if="item.sessions.length || isCurrentWorkspace(item.workspace)"
            class="workspace-sessions"
            :data-testid="isCurrentWorkspace(item.workspace) ? 'active-workspace-sessions' : 'workspace-sessions'"
            :data-active="isCurrentWorkspace(item.workspace) ? 'true' : 'false'"
          >
            <button
              v-for="session in item.sessions"
              :key="session.id"
              class="session-item"
              :class="{ active: isCurrentWorkspace(item.workspace) && session.id === remote.state.current_session_id }"
              data-testid="session-item"
              :data-session-id="session.id"
              :aria-current="isCurrentWorkspace(item.workspace) && session.id === remote.state.current_session_id ? 'page' : undefined"
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
      </nav>
    </div>

    <div class="sidebar-footer">
      <button @click="openSettings"><span aria-hidden="true">⚙</span><span>Settings</span></button>
    </div>
  </aside>
</template>
