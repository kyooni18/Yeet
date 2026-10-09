import { useEffect, useState } from 'react'
import { Lock } from '@/components/Icons'
import { remoteStore, useRemote } from '@/store/remoteStore'
import type { AgentControl, AgentTone } from '@/remote/protocol'

function clock(value: string): string {
  const date = new Date(value)
  return !value || Number.isNaN(date.getTime())
    ? ''
    : new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).format(date)
}

/** One slot for member state: motion for work, a lock for waiting, a dot only for errors. */
function MemberState({ tone, label }: { tone: AgentTone; label: string }) {
  return <span className="view-slot" role="img" aria-label={label}>
    {tone === 'active' && <span className="mini-spinner" />}
    {tone === 'waiting' && <Lock size={12} strokeWidth={1.8} />}
    {tone === 'error' && <span className="view-dot is-error" />}
  </span>
}

/** Agent Group as a page: members on the left, the shared objective and its work on the right. */
export function AgentsPage() {
  const remote = useRemote()
  const view = remote.agents
  const [draft, setDraft] = useState('')
  const canDeliver = remote.connection === 'connected'

  useEffect(() => { remoteStore.sendAgentUi({ type: 'open' }) }, [])
  useEffect(() => {
    const submitted = remote.agentEffect?.value.submitted_text
    if (submitted != null) setDraft(current => current.trim() === submitted ? '' : current)
  }, [remote.agentEffect])

  if (!view) return <section className="page-view" aria-label="Agents" />
  const group = view.group
  const selected = view.members.find(member => member.selected)
  const submit = () => remoteStore.sendAgentUi({ type: 'submit_draft', value: draft })
  const control = (item: AgentControl, kind: 'primary' | 'secondary' | 'danger' = 'secondary') => item.visible
    ? <button key={item.action.type} className={`page-button is-${kind}`} disabled={!canDeliver || !item.enabled}
        onClick={() => remoteStore.sendAgentUi(item.action)}>{item.label}</button>
    : null
  const counts = [`${view.members.length} agents`, view.active_count ? `${view.active_count} running` : '',
    view.waiting_count ? `${view.waiting_count} waiting` : ''].filter(Boolean).join(' · ')

  return (
    <section className="page-view" aria-label={view.accessible_label}>
      <header className="page-header">
        <h1>{view.title}</h1>
        <span className="page-meta">{counts}</span>
        <div className="page-actions">
          {view.group_controls.map(item => control(item, item.action.type === 'cancel_group' || item.action.type === 'stop_group' ? 'danger' : item.action.type === 'run_group' ? 'primary' : 'secondary'))}
          {group.objective ? control(view.create_control) : null}
        </div>
      </header>

      {!view.state.creating_group && !group.objective && !view.members.length ? (
        <div className="page-blank">
          <h2>{view.empty_title}</h2>
          <p>{view.empty_message}</p>
          {control(view.create_control, 'primary')}
        </div>
      ) : view.state.creating_group ? (
        <form className="page-form" onSubmit={event => { event.preventDefault(); submit() }}>
          <label htmlFor="agent-group-objective">Objective</label>
          <textarea id="agent-group-objective" rows={4} value={draft} onChange={event => setDraft(event.target.value)}
            placeholder={view.create_hint} disabled={!canDeliver} autoFocus />
          <div className="page-actions">
            <button type="button" className="page-button" onClick={() => remoteStore.sendAgentUi({ type: 'cancel_draft' })}>Cancel</button>
            <button type="submit" className="page-button is-primary" disabled={!canDeliver || !draft.trim()}>Create group</button>
          </div>
        </form>
      ) : (
        <div className="page-body">
          <nav className="page-list" aria-label="Group members">
            {view.members.map(item => (
              <button key={item.member.id} className={`page-row${item.selected ? ' is-selected' : ''}`}
                aria-pressed={item.selected} onClick={() => remoteStore.sendAgentUi(item.select_action)}>
                <MemberState tone={item.status.tone} label={item.status.label} />
                <span className="page-row__name">{item.member.description || item.member.role}</span>
                {item.status.tone === 'waiting' || item.status.tone === 'error'
                  ? <span className="page-row__meta">{item.status.label}</span> : null}
              </button>
            ))}
            {!view.members.length && <p className="page-empty">No agents have been delegated yet.</p>}
          </nav>

          <div className="page-detail">
            {view.sections.filter(section => section.visible).map(section => {
              switch (section.kind) {
                case 'objective': return <section key={section.kind}>
                  <h2>{section.label}</h2>
                  {group.objective ? <p>{group.objective}</p> : <p className="page-empty">{view.empty_title}. {view.empty_message}</p>}
                </section>
                case 'result': return <section key={section.kind}><h2>{section.label}</h2><p>{view.result}</p></section>
                case 'members': return selected ? <section key={section.kind}>
                  <h2>{selected.member.description || selected.member.role}</h2>
                  <p>{selected.member.summary || 'No summary yet.'}</p>
                  <div className="page-actions">{view.selection_controls.map(item => control(item, item.action.type === 'stop' || item.action.type === 'remove' ? 'danger' : 'secondary'))}</div>
                </section> : null
                case 'activity': return <section key={section.kind}>
                  <h2>{section.label}{selected ? ` · ${selected.member.description || selected.member.role}` : ''}</h2>
                  {view.feed.length ? <ol className="page-feed" aria-label="Group activity events">
                    {view.feed.map(entry => <li key={entry.id}>
                      <time dateTime={entry.at}>{clock(entry.at)}</time>
                      <strong>{entry.actor}</strong>
                      <span>{entry.detail || entry.kind}</span>
                    </li>)}
                  </ol> : <p className="page-empty">No activity yet.</p>}
                </section>
                case 'findings': return view.findings.length ? <section key={section.kind}>
                  <h2>{section.label}</h2>
                  {view.findings.map((finding, index) => <p key={`${finding.task_id}:${index}`}><strong>{finding.actor}</strong> {finding.summary}</p>)}
                </section> : null

              }
            })}
            {view.state.input_focused && <form className="page-form" onSubmit={event => { event.preventDefault(); submit() }}>
              <label htmlFor="agent-steering">{view.composer_hint}</label>
              <textarea id="agent-steering" value={draft} onChange={event => setDraft(event.target.value)} disabled={!canDeliver} />
              <div className="page-actions"><button type="submit" className="page-button is-primary" disabled={!canDeliver || !draft.trim()}>Send</button></div>
            </form>}
          </div>
        </div>
      )}
      {remote.state.error_message?.includes('Agent Group') && <p className="page-error" role="alert">{remote.state.error_message}</p>}
    </section>
  )
}
