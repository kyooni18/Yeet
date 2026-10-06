import { useEffect, useState } from 'react'
import { ChevronRight, Folder } from '@/components/Icons'
import { formatBytes, formatModified } from '@/ui/format'
import { remoteStore, useRemote } from '@/store/remoteStore'

function FileGlyph() {
  return <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true">
    <path d="M4 2h5l3 3v9H4z" /><path d="M9 2v3h3" />
  </svg>
}

/** Read-only browser for the workspace. Directories open on click; files show their details. */
export function FilesPage({ onShowChanges }: { onShowChanges: (path: string) => void }) {
  const remote = useRemote()
  const view = remote.workspaceFiles
  const connected = remote.connection === 'connected'

  const [unsupported, setUnsupported] = useState(false)

  useEffect(() => {
    if (connected) setUnsupported(!remoteStore.requestFiles(view?.path ?? '', null))
    // Only the first load and reconnects; navigation is driven by clicks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected])

  if (!view) return <section className="page-view" aria-label="Files">
    <p className="page-empty page-pad">{unsupported ? 'Files are not available in this client.' : connected ? 'Loading files…' : 'Reconnecting…'}</p>
  </section>
  const parts = view.path ? view.path.split('/') : []
  const crumbs = [{ label: view.root, path: '' }, ...parts.map((label, index) => ({ label, path: parts.slice(0, index + 1).join('/') }))]
  const info = view.info

  return (
    <section className="page-view" aria-label="Files">
      <header className="page-header">
        <nav className="page-crumbs" aria-label="Location">
          {crumbs.map((crumb, index) => index === crumbs.length - 1
            ? <b key={crumb.path} aria-current="location">{crumb.label}</b>
            : <span key={crumb.path}>
                <button onClick={() => remoteStore.requestFiles(crumb.path)}>{crumb.label}</button>
                <ChevronRight size={11} />
              </span>)}
        </nav>
        {view.branch && <span className="page-meta">{view.branch}</span>}
      </header>
      <div className="page-body">
        <div className="page-table" role="group" aria-label="Files">
          <div className="page-table__head" aria-hidden="true"><span /><span>Name</span><span>Size</span><span>Modified</span></div>
          {view.entries.map(entry => {
            const selected = entry.path === view.selected
            return (
              <button key={entry.path} aria-pressed={selected} className={`page-table__row${selected ? ' is-selected' : ''}`}
                onClick={() => entry.directory ? remoteStore.requestFiles(entry.path) : remoteStore.requestFiles(view.path, entry.path)}>
                <span className="view-slot">{entry.directory ? <Folder size={14} strokeWidth={1.5} /> : <FileGlyph />}</span>
                <span className="page-table__name">
                  {entry.name}
                  {entry.status && <span className="view-dot is-changed" role="img" aria-label="Changed" />}
                </span>
                <span className="page-table__meta">{entry.directory ? `${entry.items ?? 0} items` : formatBytes(entry.size)}</span>
                <span className="page-table__meta">{formatModified(entry.modified)}</span>
              </button>
            )
          })}
          {!view.entries.length && <p className="page-empty page-pad">This folder is empty.</p>}
        </div>
        {info && <aside className="page-info" aria-label="File details">
          <h2>{info.name}</h2>
          <dl>
            <dt>Size</dt><dd>{formatBytes(info.size)}</dd>
            {info.lines != null && <><dt>Lines</dt><dd>{info.lines.toLocaleString()}</dd></>}
            {info.modified != null && <><dt>Modified</dt><dd>{formatModified(info.modified)}</dd></>}
            {info.permissions && <><dt>Permissions</dt><dd>{info.permissions}</dd></>}
            <dt>Where</dt><dd>{info.path}</dd>
            {info.status && <><dt>Git</dt><dd>Changed{info.added != null && (info.added > 0 || (info.removed ?? 0) > 0) ? <> · {info.added > 0 && <span className="is-add">+{info.added}</span>} {(info.removed ?? 0) > 0 && <span className="is-del">−{info.removed}</span>}</> : null}</dd></>}
          </dl>
          {info.status && <button className="page-button" onClick={() => onShowChanges(info.path)}>View changes</button>}
        </aside>}
      </div>
    </section>
  )
}
