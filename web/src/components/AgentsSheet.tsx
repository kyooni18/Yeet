import { useEffect, useMemo, useRef, useState } from 'react'
import { Bot, CircleAlert, X } from '@/components/Icons'
import { formatTokens } from '@/ui/format'
import { remoteStore, useRemote } from '@/store/remoteStore'
import type { AgentGroupEventItem, AgentMemberItem } from '@/remote/protocol'

type FeedEntry = {
  id: string
  sequence?: number
  at: string
  kind: string
  detail: string
  memberId?: string | null
  actor: string
}

function titleCase(value: string): string {
  return value.replaceAll('_', ' ').replace(/\b\w/g, (letter) => letter.toUpperCase()) || 'Idle'
}

function memberStatus(member: AgentMemberItem): string {
  if (member.taskStatus === 'failed') return 'Failed'
  if (member.taskStatus === 'cancelled') return 'Cancelled'
  if (member.status === 'stopped') return 'Stopped'
  if (member.activityState === 'waiting_for_input') return 'Waiting for input'
  if (member.activityState === 'tool_call') return 'Using a tool'
  if (member.activityState === 'reasoning') return 'Reasoning'
  if (member.activityState === 'provider_activity') return 'Working'
  if (member.activityState === 'queued') return 'Queued'
  if (member.taskStatus === 'needs_verification') return 'Needs review'
  if (['verified', 'reported', 'done'].includes(member.taskStatus)) return 'Completed'
  return titleCase(member.status)
}

function statusTone(status: string): string {
  const normalized = status.toLowerCase()
  if (['running', 'reasoning', 'using a tool', 'working', 'queued'].includes(normalized)) return 'active'
  if (['failed', 'cancelled', 'stopped'].includes(normalized)) return 'error'
  if (['waiting for input', 'needs review', 'paused'].includes(normalized)) return 'waiting'
  if (['completed', 'verified', 'reported', 'done'].includes(normalized)) return 'complete'
  return 'idle'
}

function timestamp(value: string): string {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat(undefined, {
    hour: 'numeric', minute: '2-digit', month: 'short', day: 'numeric',
  }).format(date)
}

function progressPercent(used: number, limit: number): number {
  return limit > 0 ? Math.min(100, Math.max(0, used / limit * 100)) : 0
}

function memberActor(memberId: string | null | undefined, members: AgentMemberItem[]): string {
  if (!memberId) return 'Group coordinator'
  return members.find((member) => member.id === memberId)?.description || `Member ${memberId.slice(0, 8)}`
}

function eventEntries(events: AgentGroupEventItem[], groupId: string, members: AgentMemberItem[]): FeedEntry[] {
  return events
    .filter((event) => !groupId || event.groupId === groupId)
    .slice()
    .sort((a, b) => a.sequence - b.sequence)
    .slice(-60)
    .reverse()
    .map((event) => ({
      id: `${event.groupId}:${event.sequence}`,
      sequence: event.sequence,
      at: event.at,
      kind: event.kind,
      detail: event.detail,
      memberId: event.memberId,
      actor: memberActor(event.memberId, members),
    }))
}

export function AgentsSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const remote = useRemote()
  const group = remote.state.agent_group
  const members = group.members ?? []
  const events = group.events ?? []
  const activity = group.activity ?? []
  const budget = group.budget
  const dialogRef = useRef<HTMLElement>(null)
  const [objective, setObjective] = useState('')
  const [creating, setCreating] = useState(false)
  const [selectedMemberId, setSelectedMemberId] = useState<string | null>(null)
  const canMutate = remote.connection === 'connected'
  const selectedMember = members.find((member) => member.id === selectedMemberId) ?? null
  const activeCount = members.filter((member) => statusTone(memberStatus(member)) === 'active').length
  const waitingCount = members.filter((member) => statusTone(memberStatus(member)) === 'waiting').length
  const feed = useMemo<FeedEntry[]>(() => {
    if (events.length) return eventEntries(events, group.groupId, members)
    const byId = new Map(members.map((member) => [member.id, member.description]))
    return activity.slice().reverse().slice(0, 60).map((entry, index) => ({
      id: `${entry.at}:${index}`,
      at: entry.at,
      kind: entry.kind,
      detail: entry.text,
      memberId: entry.from ?? entry.to,
      actor: entry.from
        ? byId.get(entry.from) ?? `Member ${entry.from.slice(0, 8)}`
        : entry.kind === 'steer' ? 'You' : 'Group coordinator',
    }))
  }, [activity, events, group.groupId, members])
  const visibleFeed = selectedMemberId
    ? feed.filter((entry) => entry.memberId === selectedMemberId)
    : feed

  useEffect(() => {
    if (!open) return
    requestAnimationFrame(() => {
      dialogRef.current?.querySelector<HTMLElement>('button:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])')
        ?.focus({ preventScroll: true })
    })
  }, [open])

  useEffect(() => {
    if (!members.some((member) => member.id === selectedMemberId)) setSelectedMemberId(null)
  }, [members, selectedMemberId])

  const cycleFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab' || !dialogRef.current) return
    const focusables = [...dialogRef.current.querySelectorAll<HTMLElement>(
      'button:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    )].filter((element) => element.getClientRects().length > 0)
    if (!focusables.length) return
    const active = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const current = active ? focusables.indexOf(active) : -1
    const next = event.shiftKey
      ? (current <= 0 ? focusables.length - 1 : current - 1)
      : (current < 0 || current >= focusables.length - 1 ? 0 : current + 1)
    event.preventDefault()
    focusables[next]?.focus({ preventScroll: false })
  }

  if (!open) return null

  const create = () => {
    const nextObjective = objective.trim()
    if (!nextObjective || !canMutate) return
    if (remoteStore.createAgentGroup(nextObjective)) {
      setObjective('')
      setCreating(false)
      setSelectedMemberId(null)
    }
  }

  const runGroup = () => {
    if (!canMutate || !group.groupId) return
    if (group.status === 'paused') remoteStore.resumeAgentGroup(group.groupId)
    else remoteStore.startAgentGroup(group.groupId)
  }

  const canStart = !!group.objective && ['created', 'paused', 'completed', 'failed'].includes(group.status)
  const canCancel = !!group.groupId && group.status === 'running'
  const canStop = !!group.groupId && ['running', 'paused'].includes(group.status)
  const canCreateAnother = !['running', 'paused'].includes(group.status)
  const outputLimit = budget?.outputLimitTokens ?? 0
  const outputUsed = budget?.outputUsedTokens ?? group.outputTokens
  const costLimit = budget?.costLimitUsd ?? 0
  const costUsed = budget?.estimatedCostUsedUsd ?? group.estimatedCostUsd ?? null
  const taskAllocations = budget?.tasks ?? []

  return (
    <div className="sheet-layer agent-sheet-layer" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section
        ref={dialogRef}
        className="bottom-sheet agent-group-sheet"
        role="dialog"
        aria-modal="true"
        aria-label="Agent Group"
        onKeyDown={cycleFocus}
      >
        <div className="sheet-handle" />
        <div className="sheet-header agent-sheet-header">
          <div>
            <strong><Bot size={17} /> Agent Group</strong>
            <span>{group.objective || 'Coordinate member work around one shared objective.'}</span>
          </div>
          <span className={`agent-status agent-status--${statusTone(group.status)}`}>{titleCase(group.status)}</span>
          {canCreateAnother && !creating && (
            <button className="small-action" onClick={() => setCreating(true)}>
              {group.objective ? 'New group' : 'Create group'}
            </button>
          )}
          <button className="panel-icon" onClick={onClose} aria-label="Close Agents"><X size={15} /></button>
        </div>

        <div className="agent-sheet-scroll">
          {creating ? (
            <form
              className="agent-create inset-surface"
              onSubmit={(event) => { event.preventDefault(); create() }}
            >
              <label htmlFor="agent-group-objective">Shared objective</label>
              <textarea
                id="agent-group-objective"
                rows={3}
                value={objective}
                onChange={(event) => setObjective(event.target.value)}
                placeholder="What should the Group Agent accomplish?"
                disabled={!canMutate}
              />
              <div>
                {group.objective && <button type="button" className="agent-secondary" onClick={() => setCreating(false)}>Back</button>}
                <button type="submit" className="agent-primary" disabled={!canMutate || !objective.trim()}>
                  Create group
                </button>
              </div>
            </form>
          ) : !group.objective ? (
            <div className="agent-empty inset-surface">
              <Bot size={24} />
              <strong>No Agent Group yet</strong>
              <span>Create one shared objective. Yeet will coordinate any member work inside that group.</span>
              <button className="agent-primary" onClick={() => setCreating(true)} disabled={!canMutate}>Create group</button>
            </div>
          ) : (
            <>
              <section className="agent-objective inset-surface">
                <span className="agent-section-label">SHARED OBJECTIVE</span>
                <p>{group.objective}</p>
                <div className="agent-lifecycle">
                  {canStart && (
                    <button className="agent-primary" onClick={runGroup} disabled={!canMutate}>
                      {group.status === 'paused' ? 'Resume group' : 'Start group'}
                    </button>
                  )}
                  {canCancel && (
                    <button className="agent-danger" onClick={() => remoteStore.cancelAgentGroup(group.groupId)} disabled={!canMutate}>
                      Cancel group
                    </button>
                  )}
                  {canStop && (
                    <button className="agent-secondary" onClick={() => remoteStore.stopAgentGroup(group.groupId)} disabled={!canMutate}>
                      Stop group
                    </button>
                  )}
                  {!!group.groupId && (
                    <button className="agent-secondary" onClick={() => remoteStore.inspectAgentGroup(group.groupId)} disabled={!canMutate}>
                      Refresh state
                    </button>
                  )}
                </div>
              </section>

              {!!group.finalResult && (
                <section className="agent-result inset-surface">
                  <span className="agent-section-label">GROUP RESULT</span>
                  <p>{group.finalResult}</p>
                </section>
              )}
              {!group.finalResult && !!group.checkpointSummary && (
                <section className="agent-result inset-surface">
                  <span className="agent-section-label">LATEST CHECKPOINT</span>
                  <p>{group.checkpointSummary}</p>
                </section>
              )}

              <div className="agent-group-grid">
                <section className="agent-card">
                  <div className="agent-card-heading">
                    <strong>Members</strong>
                    <span>{activeCount} active · {waitingCount} waiting · {members.length} total</span>
                  </div>
                  {members.length ? (
                    <div className="agent-member-list" role="list" aria-label="Group members">
                      {members.map((member) => {
                        const state = memberStatus(member)
                        return (
                          <button
                            key={member.id}
                            className={`agent-member${selectedMemberId === member.id ? ' is-selected' : ''}`}
                            onClick={() => setSelectedMemberId((selected) => selected === member.id ? null : member.id)}
                            aria-pressed={selectedMemberId === member.id}
                          >
                            <span className={`agent-member-dot is-${statusTone(state)}`} />
                            <span className="agent-member-copy">
                              <strong>{member.description || member.role}</strong>
                              <small>{member.role} · {member.model || 'Default model'}</small>
                            </span>
                            <span className={`agent-status agent-status--${statusTone(state)}`}>{state}</span>
                          </button>
                        )
                      })}
                    </div>
                  ) : (
                    <div className="agent-card-empty">The coordinator has not delegated a member task yet.</div>
                  )}

                  {selectedMember && (
                    <div className="agent-member-detail">
                      <div className="agent-card-heading"><strong>{selectedMember.description || selectedMember.role}</strong><span>{memberStatus(selectedMember)}</span></div>
                      <p>{selectedMember.summary || 'No member summary yet.'}</p>
                      <div className="agent-usage-pair">
                        <span>{formatTokens(selectedMember.inputTokens)} input tokens</span>
                        <span>{formatTokens(selectedMember.outputTokens)} output tokens</span>
                      </div>
                    </div>
                  )}
                </section>

                <section className="agent-card">
                  <div className="agent-card-heading">
                    <strong>Group activity</strong>
                    <span>{selectedMember ? `Filtered to ${selectedMember.description}` : `Group ${group.groupId.slice(0, 8)}`}</span>
                  </div>
                  {visibleFeed.length ? (
                    <ol className="agent-event-list" aria-label="Group activity events">
                      {visibleFeed.map((entry) => (
                        <li key={entry.id}>
                          <span className="agent-event-marker" />
                          <div>
                            <div className="agent-event-heading">
                              <strong>{entry.actor}</strong>
                              <span>{entry.sequence ? `#${entry.sequence}` : titleCase(entry.kind)}</span>
                            </div>
                            <p>{entry.detail || titleCase(entry.kind)}</p>
                            <time dateTime={entry.at}>{timestamp(entry.at)}</time>
                          </div>
                        </li>
                      ))}
                    </ol>
                  ) : (
                    <div className="agent-card-empty">No activity recorded{selectedMember ? ' for this member' : ' yet'}.</div>
                  )}
                </section>
              </div>

              {!!group.sharedFindings.length && (
                <section className="agent-card agent-findings">
                  <div className="agent-card-heading"><strong>Shared findings</strong><span>Promoted to group context</span></div>
                  {group.sharedFindings.map((finding, index) => (
                    <div className="agent-finding" key={`${finding.taskId}:${finding.at}:${index}`}>
                      <strong>{memberActor(finding.memberId, members)}</strong>
                      <p>{finding.summary}</p>
                    </div>
                  ))}
                </section>
              )}

              {(outputLimit > 0 || costLimit > 0 || taskAllocations.length > 0) && (
                <section className="agent-card agent-budget">
                  <div className="agent-card-heading"><strong>Shared budget</strong><span>Usage and limits are shown separately</span></div>
                  <div className="agent-budget-grid">
                    {outputLimit > 0 && (
                      <div className="agent-budget-metric">
                        <div><strong>Output tokens</strong><span>{formatTokens(outputUsed)} / {formatTokens(outputLimit)}</span></div>
                        <progress value={progressPercent(outputUsed, outputLimit)} max={100} />
                        <small>{formatTokens(budget.coordinationReserveTokens)} coordination · {formatTokens(budget.synthesisReserveTokens)} synthesis reserved</small>
                      </div>
                    )}
                    {costLimit > 0 && (
                      <div className="agent-budget-metric">
                        <div><strong>Estimated cost</strong><span>{costUsed == null ? 'Not reported' : `$${costUsed.toFixed(2)}`} / ${costLimit.toFixed(2)}</span></div>
                        <progress value={costUsed == null ? 0 : progressPercent(costUsed, costLimit)} max={100} />
                        <small>${budget.coordinationReserveCostUsd.toFixed(2)} coordination · ${budget.synthesisReserveCostUsd.toFixed(2)} synthesis reserved</small>
                      </div>
                    )}
                  </div>
                  {!!taskAllocations.length && (
                    <div className="agent-task-contexts">
                      <span className="agent-section-label">TASK CONTEXT WINDOWS</span>
                      {taskAllocations.map((task) => {
                        const taskInfo = remote.state.agent_tasks.find((item) => item.id === task.taskId)
                        return (
                          <div key={task.taskId}>
                            <span>{taskInfo?.objective || `Task ${task.taskId.slice(0, 8)}`}</span>
                            <small>{formatTokens(task.contextWindowTokens)} context · {formatTokens(task.usedOutputTokens)} / {formatTokens(task.allocatedOutputTokens)} output</small>
                          </div>
                        )
                      })}
                    </div>
                  )}
                </section>
              )}
            </>
          )}
          {remote.state.error_message?.includes('Agent Group') && (
            <div className="agent-inline-error" role="alert"><CircleAlert size={14} />{remote.state.error_message}</div>
          )}
        </div>
      </section>
    </div>
  )
}
