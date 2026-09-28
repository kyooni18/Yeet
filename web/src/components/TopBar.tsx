import { Menu, SlidersHorizontal, SquarePen } from '@/components/Icons'
import { remoteStore, useRemote } from '@/store/remoteStore'

export function TopBar({
  sidebarOpen,
  controlsOpen,
  onToggleSidebar,
  onToggleControls,
}: {
  sidebarOpen: boolean
  controlsOpen: boolean
  onToggleSidebar: () => void
  onToggleControls: () => void
}) {
  const remote = useRemote()
  const workspace = remote.currentWorkspace?.display_name || remote.state.workspace_root || 'Workspace'
  const title = remote.currentSession?.title || 'New Chat'

  const connectionLabel = remote.state.is_streaming
    ? 'Running'
    : remote.connection === 'connected'
      ? 'Ready'
      : remote.connection === 'offline'
        ? 'Offline'
        : remote.connection === 'failed'
          ? 'Connection failed'
          : remote.connection === 'auth-required'
            ? 'Sign in required'
            : remote.connection === 'reconnecting'
              ? 'Reconnecting'
              : 'Connecting'
  const statusText = remote.connection === 'failed' && remote.connectionError
    ? remote.connectionError
    : connectionLabel


  const connectionTone = remote.state.is_streaming
    ? 'busy'
    : remote.connection === 'connected'
      ? 'ok'
      : remote.connection === 'failed' || remote.connection === 'offline' || remote.connection === 'auth-required'
        ? 'warning'
        : 'busy'

  return (
    <header className="remote-topbar">
      <div className="topbar-leading">
        <button
          className="topbar-icon"
          onClick={onToggleSidebar}
          aria-label="Open sidebar"
          aria-pressed={sidebarOpen}
        >
          <Menu size={18} />
        </button>

        <div className="topbar-identity">
          <span className="topbar-workspace">{workspace}</span>
          <strong className="topbar-title">{title}</strong>
        </div>
      </div>

      <div
        className={`topbar-status is-${connectionTone}`}
        role={remote.connection === 'failed' ? 'alert' : 'status'}
        aria-live={remote.connection === 'failed' ? 'assertive' : 'polite'}
        aria-label={remote.connection === 'failed' ? 'Connection failed' : undefined}
        title={statusText}
      >
        <span className="topbar-status__dot" aria-hidden="true" />
        <span>{statusText}</span>
      </div>

      <div className="topbar-actions">
        <button
          className="topbar-icon"
          onClick={() => remoteStore.newSession()}
          aria-label="New chat"
          disabled={remote.connection !== 'connected'}
        >
          <SquarePen size={16} />
        </button>
        <button
          className={`topbar-icon${controlsOpen ? ' is-active' : ''}`}
          onClick={onToggleControls}
          aria-label="Quick settings"
          aria-pressed={controlsOpen}
        >
          <SlidersHorizontal size={16} />
        </button>
      </div>
    </header>
  )
}
