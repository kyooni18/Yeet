import { Menu, SlidersHorizontal, SquarePen } from '@/components/Icons'
import { remoteStore, useRemote } from '@/store/remoteStore'

export function TopBar({
  onOpenSidebar,
  onToggleControls,
}: {
  onOpenSidebar: () => void
  onToggleControls: () => void
}) {
  const remote = useRemote()
  return (
    <header className="remote-topbar">
      <button
        className="glass-circle topbar-icon"
        onClick={(event) => {
          event.currentTarget.focus({ preventScroll: true })
          onOpenSidebar()
        }}
        aria-label="Open sidebar"
      >
        <Menu size={18} strokeWidth={1.8} />
      </button>
      <div className="topbar-title">{remote.currentSession?.title || 'New Chat'}</div>
      <div className="glass-capsule topbar-actions">
        <button
          className="topbar-icon"
          onClick={() => remoteStore.newSession()}
          aria-label="New chat"
          disabled={remote.connection !== 'connected'}
        >
          <SquarePen size={17} strokeWidth={1.8} />
        </button>
        <button
          className="topbar-icon"
          onClick={(event) => {
            event.currentTarget.focus({ preventScroll: true })
            onToggleControls()
          }}
          aria-label="Quick settings"
        >
          <SlidersHorizontal size={18} strokeWidth={1.75} />
        </button>
      </div>
    </header>
  )
}
