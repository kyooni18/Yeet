import { useEffect, useRef, useState } from 'react'
import { Bot, Check, ChevronsUpDown, DiffIcon, Folder, Lock, Plus, Search, Settings, SessionList, X } from '@/components/Icons'
import type { Destination } from '@/components/RemoteShell'
import { remoteStore, useRemote } from '@/store/remoteStore'
import type { ResourceTarget } from '@/remote/protocol'

function sessionAge(value: string): string {
  const time = new Date(value).getTime()
  if (Number.isNaN(time)) return ''
  const minutes = Math.max(0, Math.round((Date.now() - time) / 60_000))
  if (minutes < 1) return 'now'
  if (minutes < 60) return `${minutes}m`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours}h`
  const days = Math.round(hours / 24)
  return days === 1 ? 'yesterday' : `${days}d`
}

export function Sidebar({
  open,
  desktopDocked,
  onClose,
  onSettings,
  page,
  onSelect,
}: {
  open: boolean
  desktopDocked: boolean
  onClose: () => void
  onSettings: () => void
  page: Destination
  onSelect: (destination: Destination) => void
}) {
  const remote = useRemote()
  const toolbar = remote.ui?.toolbar
  const workspaceChoices = toolbar?.workspace_choices?.map(workspace => ({
    ...workspace,
    is_current: workspace.selected,
  })) ?? remote.state.known_workspaces.map(workspace => ({
    id: workspace.id,
    path: workspace.path,
    label: workspace.display_name,
    is_current: workspace.is_current,
  }))
  const [workspaceMenu, setWorkspaceMenu] = useState(false)
  const [filter, setFilter] = useState('')
  const panelRef = useRef<HTMLElement>(null)
  const returnFocus = useRef<HTMLElement | null>(null)
  const wasOpen = useRef(false)
  const awaitingPermission = Boolean(remote.state.pending_shell_permission || remote.state.pending_native_app_permission)
  const group = remote.state.agent_group
  const members = group?.members ?? []
  const activeMembers = members.filter((member) =>
    member.status === 'running' || ['reasoning', 'tool_call', 'provider_activity', 'queued'].includes(member.activityState),
  ).length
  const homeRows = remote.home?.activity ?? []
  const sessionEnd = homeRows.findIndex(row => row.type === 'gap')
  const sessionRows: { id: string; title: string; detail: string; target: ResourceTarget | null }[] = remote.home
    ? homeRows
      .slice(0, sessionEnd < 0 ? homeRows.length : sessionEnd)
      .flatMap(row => row.type === 'item' && row.value.target.type === 'session'
        ? [{ id: row.value.target.value, title: row.value.title, detail: [row.value.context, row.value.age].filter(Boolean).join(' · '), target: row.value.target }]
        : [])
    : remote.currentWorkspaceSessions.map(session => ({
      id: session.id,
      title: session.title || 'Untitled',
      detail: [remote.currentWorkspace?.display_name, sessionAge(session.updated_at)].filter(Boolean).join(' · ') || session.model,
      target: null,
    }))

  useEffect(() => {
    if (open && !wasOpen.current) {
      const active = document.activeElement
      returnFocus.current = active instanceof HTMLElement && active !== document.body ? active : null
      if (!desktopDocked) {
        requestAnimationFrame(() => {
          panelRef.current?.querySelector<HTMLElement>('button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])')
            ?.focus({ preventScroll: true })
        })
      }
    } else if (!open && wasOpen.current) {
      setWorkspaceMenu(false)
      const target = returnFocus.current
      returnFocus.current = null
      if (!desktopDocked && target?.isConnected) {
        requestAnimationFrame(() => target.focus({ preventScroll: true }))
      }
    } else if (!open) {
      setWorkspaceMenu(false)
    }
    wasOpen.current = open
  }, [open, desktopDocked])

  const cycleFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab' || !panelRef.current) return
    const focusables = [...panelRef.current.querySelectorAll<HTMLElement>(
      'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    )].filter((element) => element.getClientRects().length > 0)
    if (!focusables.length) return

    const active = document.activeElement
    const current = active instanceof HTMLElement ? focusables.indexOf(active) : -1
    const next = event.shiftKey
      ? (current <= 0 ? focusables.length - 1 : current - 1)
      : (current < 0 || current >= focusables.length - 1 ? 0 : current + 1)
    event.preventDefault()
    focusables[next]?.focus({ preventScroll: false })
  }

  return (
    <>
      <div
        className={`panel-backdrop sidebar-backdrop${open ? ' is-open' : ''}${desktopDocked ? ' is-desktop-docked' : ''}`}
        onClick={onClose}
        aria-hidden={!open}
      />
      <aside
        ref={panelRef}
        className={`remote-sidebar${open ? ' is-open' : ''}${desktopDocked ? ' is-desktop-docked' : ''}`}
        aria-hidden={!open}
        role={desktopDocked ? 'complementary' : 'dialog'}
        aria-modal={desktopDocked ? undefined : true}
        aria-label="Sessions"
        onKeyDown={desktopDocked ? undefined : cycleFocus}
      >
        <div className="sidebar-header">
          <strong>Yeet</strong>
          <div className="sidebar-header-actions">
            <button className="panel-icon" onClick={() => onSettings()} aria-label="Settings"><Settings size={15} /></button>
            <button className="panel-icon" onClick={onClose} aria-label="Close sidebar"><X size={15} /></button>
          </div>
        </div>

        <nav className="sidebar-nav" aria-label="Navigation">
          {([
            ['session', 'Sessions', <SessionList key="s" size={15} strokeWidth={1.6} />],
            ['files', 'Files', <Folder key="f" size={15} strokeWidth={1.6} />],
            ['diff', 'Diff', <DiffIcon key="d" size={15} strokeWidth={1.6} />],
            ['agents', 'Agents', <Bot key="a" size={15} strokeWidth={1.6} />],
          ] as const).map(([destination, label, icon]) => (
            <button key={destination} className={`sidebar-nav__row${page === destination ? ' is-current' : ''}`}
              aria-current={page === destination ? 'page' : undefined}
              aria-label={destination === 'agents' && activeMembers ? `Agents, ${activeMembers} running` : label}
              onClick={() => { onSelect(destination); if (!desktopDocked) onClose() }}>
              {icon}<span>{label}</span>
              {destination === 'agents' && activeMembers > 0 && <span className="sidebar-nav__count">{activeMembers}</span>}
            </button>
          ))}
        </nav>

        <div className="workspace-picker">
          <button
            className="workspace-picker__button inset-surface"
            onClick={() => setWorkspaceMenu((value) => !value)}
            aria-expanded={workspaceMenu}
          >
            <Folder size={15} strokeWidth={1.7} />
            <span>{remote.currentWorkspace?.display_name || remote.state.workspace_root || 'Workspace'}</span>
            <ChevronsUpDown size={11} strokeWidth={1.8} />
          </button>
          {workspaceMenu && (
            <div className="floating-menu workspace-menu">
              {workspaceChoices.map((workspace) => (
                <button
                  key={workspace.id}
                  onClick={() => {
                    setWorkspaceMenu(false)
                    if (workspace.is_current) return
                    if (toolbar?.workspace_choices && toolbar.workspace_source) {
                      remoteStore.sendToolbarUi({
                        type: 'choose_workspace',
                        value: {
                          id: workspace.id,
                          source_workspace: toolbar.workspace_source ?? remote.state.workspace_root ?? '',
                        },
                      })
                    } else {
                      remoteStore.switchWorkspace(workspace.path)
                    }
                  }}
                >
                  <span>{workspace.label}</span>
                  {workspace.is_current && <Check size={13} />}
                </button>
              ))}
              {!workspaceChoices.length && <div className="menu-empty">No workspaces</div>}
            </div>
          )}
        </div>

        <div className="sidebar-search inset-surface">
          <Search size={13} strokeWidth={1.8} />
          <input
            type="search"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder="Search"
            aria-label="Search sessions"
          />
        </div>

        <div className="sidebar-session-list" aria-label="Saved sessions">
          {sessionRows.filter((session) => !filter.trim() || session.title.toLowerCase().includes(filter.trim().toLowerCase())).map((session) => {
            const current = session.id === remote.state.current_session_id
            return (
              <button
                key={session.id}
                className={`sidebar-session${current ? ' is-current' : ''}`}
                data-session-id={session.id}
                aria-current={current ? 'page' : undefined}
                disabled={!current && remote.connection !== 'connected'}
                onClick={() => {
                  onSelect('session')
                  if (!current) {
                    if (session.target) remoteStore.sendHomeUi({ type: 'open', value: session.target })
                    else remoteStore.loadSession(session.id)
                  }
                  if (!desktopDocked) onClose()
                }}
              >
                <span className="sidebar-session__copy">
                  <strong>{session.title}</strong>
                  <span>{session.detail}</span>
                </span>
                <span className="sidebar-session__activity">
                  {current && awaitingPermission ? <Lock size={12} strokeWidth={1.8} className="sidebar-session__lock" aria-label="Needs approval" role="img" /> : null}
                  {current && remote.state.is_streaming ? <span className="mini-spinner" role="img" aria-label="Running" /> : null}
                </span>
              </button>
            )
          })}
          {!sessionRows.length && (
            <div className="sidebar-empty">No saved sessions in this workspace.</div>
          )}
        </div>

        <div className="sidebar-footer">
          <button
            className="sidebar-new-session"
            onClick={() => {
              onSelect('session')
              if (remote.home) remoteStore.sendHomeUi({ type: 'new_session' })
              else remoteStore.newSession()
              if (!desktopDocked) onClose()
            }}
            disabled={remote.connection !== 'connected'}
          >
            <Plus size={14} strokeWidth={1.9} />
            <span>New session</span>
            <kbd aria-hidden="true">⌘N</kbd>
          </button>
        </div>
      </aside>
    </>
  )
}
