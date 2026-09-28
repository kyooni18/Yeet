import { useEffect, useRef, useState } from 'react'
import { Check, ChevronsUpDown, Folder, Plus, Settings, X } from '@/components/Icons'
import { remoteStore, useRemote } from '@/store/remoteStore'

export function Sidebar({
  open,
  desktopDocked,
  onClose,
  onSettings,
}: {
  open: boolean
  desktopDocked: boolean
  onClose: () => void
  onSettings: () => void
}) {
  const remote = useRemote()
  const [workspaceMenu, setWorkspaceMenu] = useState(false)
  const panelRef = useRef<HTMLElement>(null)
  const returnFocus = useRef<HTMLElement | null>(null)
  const wasOpen = useRef(false)

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
            <button className="panel-icon" onClick={onSettings} aria-label="Settings"><Settings size={15} /></button>
            <button className="panel-icon" onClick={onClose} aria-label="Close sidebar"><X size={15} /></button>
          </div>
        </div>

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
              {remote.state.known_workspaces.map((workspace) => (
                <button
                  key={workspace.id}
                  onClick={() => {
                    setWorkspaceMenu(false)
                    if (!workspace.is_current) remoteStore.switchWorkspace(workspace.path)
                  }}
                >
                  <span>{workspace.display_name}</span>
                  {workspace.is_current && <Check size={13} />}
                </button>
              ))}
              {!remote.state.known_workspaces.length && <div className="menu-empty">No workspaces</div>}
            </div>
          )}
        </div>

        <div className="sidebar-section-heading">
          <span>Sessions</span>
          <button
            className="panel-icon"
            onClick={() => {
              remoteStore.newSession()
              if (!desktopDocked) onClose()
            }}
            aria-label="New session"
            disabled={remote.connection !== 'connected'}
          >
            <Plus size={15} strokeWidth={1.9} />
          </button>
        </div>

        <div className="sidebar-session-list" aria-label="Saved sessions">
          {remote.currentWorkspaceSessions.map((session) => {
            const current = session.id === remote.state.current_session_id
            return (
              <button
                key={session.id}
                className={`sidebar-session${current ? ' is-current' : ''}`}
                data-session-id={session.id}
                aria-current={current ? 'page' : undefined}
                disabled={!current && remote.connection !== 'connected'}
                onClick={() => {
                  if (!current) remoteStore.loadSession(session.id)
                  if (!desktopDocked) onClose()
                }}
              >
                <span className="sidebar-session__activity">
                  {current && remote.state.is_streaming ? <span className="mini-spinner" /> : null}
                </span>
                <span className="sidebar-session__copy">
                  <strong>{session.title || 'Untitled'}</strong>
                  <span>{session.model || 'Model'}</span>
                </span>
              </button>
            )
          })}
          {!remote.currentWorkspaceSessions.length && (
            <div className="sidebar-empty">No saved sessions in this workspace.</div>
          )}
        </div>
      </aside>
    </>
  )
}
