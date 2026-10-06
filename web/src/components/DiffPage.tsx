import { useEffect, useMemo } from 'react'
import { parsePatch } from '@/ui/format'
import { remoteStore, useRemote } from '@/store/remoteStore'

const REFRESH_MS = 5000

function Counts({ added, removed }: { added: number; removed: number }) {
  return <>{added > 0 && <span className="is-add">+{added}</span>}{added > 0 && removed > 0 ? ' ' : ''}{removed > 0 && <span className="is-del">−{removed}</span>}</>
}

function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf('/')
  return slash < 0 ? { name: path, dir: '' } : { name: path.slice(slash + 1), dir: path.slice(0, slash) }
}

/** Uncommitted changes: files on the left, the selected file's patch on the right. */
export function DiffPage({ initialFile }: { initialFile: string | null }) {
  const remote = useRemote()
  const view = remote.workspaceChanges
  const connected = remote.connection === 'connected'
  const selected = view?.selected ?? null
  const full = view?.full ?? false

  useEffect(() => {
    if (!connected) return
    remoteStore.requestChanges(initialFile, false)
    // Agents change files while the page is open, so keep the list current.
    const timer = window.setInterval(() => {
      const current = remoteStore.getSnapshot().workspaceChanges
      remoteStore.requestChanges(current?.selected ?? null, current?.full ?? false)
    }, REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [connected, initialFile])

  const rows = useMemo(() => parsePatch(view?.patch ?? []), [view?.patch])
  const totals = useMemo(() => (view?.files ?? []).reduce(
    (sum, file) => ({ added: sum.added + (file.added ?? 0), removed: sum.removed + (file.removed ?? 0) }),
    { added: 0, removed: 0 }), [view?.files])

  if (!view) return <section className="page-view" aria-label="Diff"><p className="page-empty page-pad">Loading changes…</p></section>

  return (
    <section className="page-view" aria-label="Diff">
      <header className="page-header">
        <h1>Uncommitted changes</h1>
        {view.files.length > 0 && <span className="page-meta">
          {view.files.length} {view.files.length === 1 ? 'file' : 'files'} · <Counts added={totals.added} removed={totals.removed} />
        </span>}
        {view.branch && <span className="page-meta">{view.branch}</span>}
        {view.files.length > 0 && <div className="page-segment" role="group" aria-label="Diff scope">
          <button aria-pressed={!full} onClick={() => remoteStore.requestChanges(selected, false)}>Changes</button>
          <button aria-pressed={full} onClick={() => remoteStore.requestChanges(selected, true)}>Full file</button>
        </div>}
      </header>

      {view.files.length === 0 ? (
        <div className="page-blank">
          <h2>{view.message ?? 'No uncommitted changes'}</h2>
          {!view.message && <p>Edits made in this workspace appear here.</p>}
        </div>
      ) : (
        <div className="page-body">
          <nav className="page-list" aria-label="Changed files">
            {view.files.map(file => {
              const { name, dir } = splitPath(file.path)
              return (
                <button key={file.path} className={`page-row${file.path === selected ? ' is-selected' : ''}`}
                  aria-pressed={file.path === selected} title={file.path}
                  onClick={() => remoteStore.requestChanges(file.path, full)}>
                  <span className="page-row__name page-row__mono">{name}{dir && <small>{dir}</small>}</span>
                  <span className="page-row__meta">
                    {file.added != null ? <Counts added={file.added} removed={file.removed ?? 0} /> : 'binary'}
                  </span>
                </button>
              )
            })}
          </nav>
          <div className="page-patch" role="region" aria-label={selected ? `Changes in ${selected}` : 'Changes'} tabIndex={0}>
            {selected && <div className="page-patch__path">{selected}</div>}
            {view.message && <p className="page-error">{view.message}</p>}
            {rows.length === 0 && !view.message && <p className="page-empty page-pad">No textual changes to show.</p>}
            {rows.map((row, index) => row.kind === 'hunk'
              ? <div key={index} className="patch-hunk">{row.text || ' '}</div>
              : <div key={index} className={`patch-line is-${row.kind}`}>
                  <i aria-hidden="true">{row.newNumber ?? row.oldNumber ?? ''}</i>
                  <b aria-hidden="true">{row.kind === 'add' ? '+' : row.kind === 'del' ? '−' : ''}</b>
                  <code>{row.text}</code>
                </div>)}
          </div>
        </div>
      )}
    </section>
  )
}
