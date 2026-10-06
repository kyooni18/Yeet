import { Layers3, Menu, Pause, SlidersHorizontal, SquarePen } from '@/components/Icons'
import { formatTokens } from '@/ui/format'
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
  const directory = remote.currentWorkspace?.path || remote.state.workspace_root || workspace
  const usage = remote.state.token_usage
  const tokens = (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0)

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
          onClick={(event) => {
            // Keep the touch invoker focused while UI crosses transport.
            event.currentTarget.focus({ preventScroll: true })
            onToggleSidebar()
          }}
          aria-label="Open sidebar"
          aria-pressed={sidebarOpen}
        >
          <Menu size={18} />
        </button>

        <div className="topbar-identity">
          <strong className="topbar-title">{title}</strong>
          <span className="topbar-workspace">{workspace}</span>
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

      <div className="session-meta" aria-label="Session details">
        <span className="session-meta__path" title={directory}>{directory}</span>
        <span className="session-meta__spacer" />
        {remote.state.is_streaming && <span className="session-meta__run"><span className="mini-spinner" aria-hidden="true" />Running</span>}
        {tokens > 0 && <span className="session-meta__tokens" title="Tokens used"><Layers3 size={12} strokeWidth={1.8} />{formatTokens(tokens)}</span>}
        <span className="session-meta__divider" aria-hidden="true" />
        <button
          className="session-meta__button"
          onClick={() => remoteStore.interrupt()}
          disabled={!remote.state.is_streaming}
          aria-label="Interrupt run"
          title="Interrupt run"
        >
          <Pause size={14} strokeWidth={1.9} />
        </button>
      </div>
    </header>
  )
}
