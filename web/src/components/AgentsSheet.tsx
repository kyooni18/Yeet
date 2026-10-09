import { useEffect, useRef, useState } from 'react'
import { Bot, Check, CircleAlert, Plus, RotateCw, Send, Settings, Square, X } from '@/components/Icons'
import { remoteStore, useRemote } from '@/store/remoteStore'
import type { AgentControl, AgentIcon } from '@/remote/protocol'

const glyphs = { add: Plus, play: Send, stop: Square, remove: X, settings: Settings,
  refresh: RotateCw, message: Send, tool: Bot, complete: Check, error: CircleAlert } satisfies Record<AgentIcon, typeof Bot>

function timestamp(value: string): string {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat(undefined, {
    hour: 'numeric', minute: '2-digit', month: 'short', day: 'numeric',
  }).format(date)
}

/** Native browser components for the shared Rust Agent Group view. */
export function AgentsSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const remote = useRemote()
  const view = remote.agents
  const dialogRef = useRef<HTMLElement>(null)
  // A native editor buffer, committed through the shared semantic draft action.
  const [draft, setDraft] = useState('')
  const canDeliver = remote.connection === 'connected'

  useEffect(() => {
    if (!open) return
    remoteStore.sendAgentUi({ type: 'open' })
    requestAnimationFrame(() => {
      dialogRef.current?.querySelector<HTMLElement>('button:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])')
        ?.focus({ preventScroll: true })
    })
  }, [open])
  useEffect(() => {
    const submitted = remote.agentEffect?.value.submitted_text
    if (submitted != null) setDraft(current => current.trim() === submitted ? '' : current)
  }, [remote.agentEffect])

  const cycleFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab' || !dialogRef.current) return
    const focusables = [...dialogRef.current.querySelectorAll<HTMLElement>(
      'button:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    )].filter(element => element.getClientRects().length > 0)
    if (!focusables.length) return
    const current = document.activeElement instanceof HTMLElement ? focusables.indexOf(document.activeElement) : -1
    const next = event.shiftKey ? (current <= 0 ? focusables.length - 1 : current - 1)
      : (current < 0 || current >= focusables.length - 1 ? 0 : current + 1)
    event.preventDefault()
    focusables[next]?.focus({ preventScroll: false })
  }

  if (!open || !view) return null
  const group = view.group
  const selectedMember = view.members.find(member => member.selected)
  const submit = () => remoteStore.sendAgentUi({ type: 'submit_draft', value: draft })
  const control = (item: AgentControl, className = 'agent-secondary') => {
    if (!item.visible) return null
    const Glyph = glyphs[item.icon]
    return <button key={item.action.type} className={className} disabled={!canDeliver || !item.enabled}
      onClick={() => remoteStore.sendAgentUi(item.action)}><Glyph size={13} />{item.label}</button>
  }

  return (
    <div className="sheet-layer agent-sheet-layer" onMouseDown={event => event.target === event.currentTarget && onClose()}>
      <section ref={dialogRef} className="bottom-sheet agent-group-sheet" role="dialog" aria-modal="true" aria-label={view.accessible_label} onKeyDown={cycleFocus}>
        <div className="sheet-handle" />
        <div className="sheet-header agent-sheet-header">
          <div><strong><Bot size={17} />{view.title}</strong><span>{view.headline}</span></div>
          <span className={`agent-status agent-status--${view.group_status.tone}`}>{view.group_status.label}</span>
          {control(view.create_control, 'small-action')}
          <button className="panel-icon" onClick={onClose} aria-label="Close Agents"><X size={15} /></button>
        </div>
        <div className="agent-sheet-scroll">
          {view.state.creating_group ? (
            <form className="agent-create inset-surface" onSubmit={event => { event.preventDefault(); submit() }}>
              <label htmlFor="agent-group-objective">Shared objective</label>
              <textarea id="agent-group-objective" rows={3} value={draft} onChange={event => setDraft(event.target.value)}
                placeholder={view.create_hint} disabled={!canDeliver} />
              <div>
                <button type="button" className="agent-secondary" onClick={() => remoteStore.sendAgentUi({ type: 'cancel_draft' })}>Back</button>
                <button type="submit" className="agent-primary" disabled={!canDeliver || !draft.trim()}>Create group</button>
              </div>
            </form>
          ) : (
            <>
              <div className="agent-group-grid">
              {view.sections.filter(section => section.visible).map(section => {
                const style = { gridColumn: section.layout === 'full_width' ? '1 / -1' : undefined }
                switch (section.kind) {
                  case 'objective': return (
                    <section key={section.kind} style={style} className="agent-objective inset-surface">
                      <span className="agent-section-label">{section.label}</span>
                      {group.objective ? <p>{group.objective}</p> : <div className="agent-empty">
                        <Bot size={24} /><strong>{view.empty_title}</strong><span>{view.empty_message}</span>
                        {control(view.create_control, 'agent-primary')}
                      </div>}
                      <div className="agent-lifecycle">{view.group_controls.map(item => control(item,
                        item.action.type === 'cancel_group' ? 'agent-danger' : item.action.type === 'run_group' ? 'agent-primary' : 'agent-secondary'))}</div>
                    </section>
                  )
                  case 'result': return <section key={section.kind} style={style} className="agent-result inset-surface">
                    <span className="agent-section-label">{section.label}</span><p>{view.result}</p>
                  </section>
                  case 'members': return (
                    <section key={section.kind} style={style} className="agent-card">
                      <div className="agent-card-heading"><strong>{section.label}</strong>
                        <span>{view.active_count} active · {view.waiting_count} waiting · {view.members.length} total</span></div>
                      {view.members.length ? <div className="agent-member-list" role="list" aria-label="Group members">
                        {view.members.map(item => <button key={item.member.id} className={`agent-member${item.selected ? ' is-selected' : ''}`}
                          onClick={() => remoteStore.sendAgentUi(item.select_action)} aria-pressed={item.selected}>
                          <span className={`agent-member-dot is-${item.status.tone}`} />
                          <span className="agent-member-copy"><strong>{item.member.description || item.member.role}</strong>
                            <small>{item.member.role} · {item.member.model || 'Default model'}</small></span>
                          <span className={`agent-status agent-status--${item.status.tone}`}>{item.status.label}</span>
                        </button>)}
                      </div> : <div className="agent-card-empty">The coordinator has not delegated a member task yet.</div>}
                      {selectedMember && <div className="agent-member-detail">
                        <div className="agent-card-heading"><strong>{selectedMember.member.description || selectedMember.member.role}</strong><span>{selectedMember.status.label}</span></div>
                        <p>{selectedMember.member.summary || 'No member summary yet.'}</p>
                      </div>}
                      <div className="agent-lifecycle">{view.selection_controls.map(item => control(item))}</div>
                    </section>
                  )
                  case 'activity': return <section key={section.kind} style={style} className="agent-card">
                    <div className="agent-card-heading"><strong>{section.label}</strong><span>{selectedMember ? `Filtered to ${selectedMember.member.description}` : `Group ${group.groupId.slice(0, 8)}`}</span></div>
                    {view.feed.length ? <ol className="agent-event-list" aria-label="Group activity events">
                      {view.feed.map(entry => <li key={entry.id}><span className="agent-event-marker" /><div>
                        <div className="agent-event-heading"><strong>{entry.actor}</strong><span>{entry.sequence ? `#${entry.sequence}` : entry.kind}</span></div>
                        <p>{entry.detail || entry.kind}</p><time dateTime={entry.at}>{timestamp(entry.at)}</time>
                      </div></li>)}
                    </ol> : <div className="agent-card-empty">No activity recorded{selectedMember ? ' for this member' : ' yet'}.</div>}
                  </section>
                  case 'findings': return <section key={section.kind} style={style} className="agent-card agent-findings">
                    <div className="agent-card-heading"><strong>{section.label}</strong><span>Promoted to group context</span></div>
                    {view.findings.map((finding, index) => <div key={`${finding.task_id}:${finding.at}:${index}`} className="agent-finding">
                      <strong>{finding.actor}</strong><p>{finding.summary}</p>
                    </div>)}
                  </section>

                }
              })}
              </div>
              {view.state.input_focused && <form className="agent-create inset-surface" onSubmit={event => { event.preventDefault(); submit() }}>
                <label htmlFor="agent-steering">{view.composer_hint}</label>
                <textarea id="agent-steering" value={draft} onChange={event => setDraft(event.target.value)} disabled={!canDeliver} />
                <button type="submit" className="agent-primary" disabled={!canDeliver || !draft.trim()}>Send</button>
              </form>}
            </>
          )}
          {remote.state.error_message?.includes('Agent Group') && <div className="agent-inline-error" role="alert"><CircleAlert size={14} />{remote.state.error_message}</div>}
        </div>
      </section>
    </div>
  )
}
