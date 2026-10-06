import type { ToolbarAction } from '@/remote/protocol'
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
  onToggleSidebar: (action?: ToolbarAction) => void
  onToggleControls: (action?: ToolbarAction) => void
}) {
  const remote = useRemote()
  const groups = remote.ui?.toolbar?.groups ?? []
  const icons = { menu: Menu, new_session: SquarePen, controls: SlidersHorizontal, interrupt: Pause }
  const renderControls = (group: string) => groups.find(item => item.id === group)?.controls.map(control => {
    const Icon = icons[control.icon as keyof typeof icons]
    return <button key={control.id} className={`${group === 'session_actions' ? 'session-meta__button' : 'topbar-icon'}${control.pressed ? ' is-active' : ''}`}
      aria-label={control.label} title={control.label} aria-pressed={control.pressed ?? undefined}
      disabled={!control.enabled || (['new_session', 'interrupt'].includes(control.id) && remote.connection !== 'connected')}
      onClick={event => {
        event.currentTarget.focus({preventScroll:true})
        const action:ToolbarAction={type:'activate',value:control.id}
        if(control.id==='navigation')onToggleSidebar(action)
        else if(control.id==='quick_controls')onToggleControls(action)
        else remoteStore.sendToolbarUi(action)
      }}>{Icon && <Icon size={group === 'header_leading' ? 18 : 16} />}</button>
  })
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
        {renderControls('header_leading')}

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
        {renderControls('header_actions')}
      </div>

      <div className="session-meta" aria-label="Session details">
        <span className="session-meta__path" title={directory}>{directory}</span>
        <span className="session-meta__spacer" />
        {remote.state.is_streaming && <span className="session-meta__run"><span className="mini-spinner" aria-hidden="true" />Running</span>}
        {tokens > 0 && <span className="session-meta__tokens" title="Tokens used"><Layers3 size={12} strokeWidth={1.8} />{formatTokens(tokens)}</span>}
        <span className="session-meta__divider" aria-hidden="true" />
        {renderControls('session_actions')}
      </div>
    </header>
  )
}
