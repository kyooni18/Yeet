import type { ToolbarAction } from '@/remote/protocol'
import { Layers3, Menu, Pause, SlidersHorizontal, SquarePen } from '@/components/Icons'
import { formatTokens } from '@/ui/format'
import { remoteStore, useRemote } from '@/store/remoteStore'

export function TopBar({
  sidebarOpen,
  controlsOpen,
  onToggleSidebar,
  onToggleControls,
  page,
}: {
  sidebarOpen: boolean
  controlsOpen: boolean
  onToggleSidebar: (action?: ToolbarAction) => void
  onToggleControls: (action?: ToolbarAction) => void
  page?: string
}) {
  const remote = useRemote()
  const groups = remote.ui?.toolbar?.groups ?? []
  const icons = { menu: Menu, new_session: SquarePen, controls: SlidersHorizontal, interrupt: Pause }
  const renderControls = (group: string) => groups.find(item => item.id === group)?.controls.filter(control => control.id !== 'interrupt').map(control => {
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
  const title = page ?? (remote.currentSession?.title || 'New Chat')
  const directory = remote.currentWorkspace?.path || remote.state.workspace_root || workspace
  const usage = remote.state.token_usage
  const tokens = (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0)

  // Only a connection problem is worth a label; a healthy session shows nothing.
  const problem = remote.connection === 'connected' ? null
    : remote.connection === 'failed' ? (remote.connectionError || 'Connection failed')
    : remote.connection === 'offline' ? 'Offline'
    : remote.connection === 'auth-required' ? 'Sign in required'
    : remote.connection === 'reconnecting' ? 'Reconnecting'
    : 'Connecting'
  const fatal = remote.connection === 'failed' || remote.connection === 'offline' || remote.connection === 'auth-required'

  return (
    <header className={`remote-topbar${page ? ' is-page' : ''}`}>
      <div className="topbar-leading">
        {renderControls('header_leading')}

        <div className="topbar-identity">
          <strong className="topbar-title">{title}</strong>
          <span className="topbar-workspace">{workspace}</span>
        </div>
      </div>

      {problem && <div className={`topbar-status ${fatal ? 'is-warning' : 'is-busy'}`}
        role={remote.connection === 'failed' ? 'alert' : 'status'}
        aria-live={remote.connection === 'failed' ? 'assertive' : 'polite'}
        aria-label={remote.connection === 'failed' ? 'Connection failed' : undefined} title={problem}>
        <span className="topbar-status__dot" aria-hidden="true" /><span>{problem}</span>
      </div>}

      <div className="topbar-actions">
        {renderControls('header_actions')}
      </div>

      {!page && <div className="session-meta" aria-label="Session details">
        <span className="session-meta__path" title={directory}>{directory}</span>
        <span className="session-meta__spacer" />
        {remote.state.is_streaming && <span className="session-meta__run"><span className="mini-spinner" aria-hidden="true" />Running</span>}
        {tokens > 0 && <span className="session-meta__tokens" title="Tokens used"><Layers3 size={12} strokeWidth={1.8} />{formatTokens(tokens)}</span>}
        <span className="session-meta__divider" aria-hidden="true" />
        {renderControls('session_actions')}
      </div>}
    </header>
  )
}
