import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import DOMPurify from 'dompurify'
import MarkdownIt from 'markdown-it'
import {
  BookOpen,
  Check,
  ChevronDown,
  CircleAlert,
  Copy,
  FileText,
  Folder,
  Globe,
  Info,
  Lightbulb,
  Lock,
  MacWindow,
  Pencil,
  Plug,
  RotateCw,
  Search,
  Terminal,
  TriangleAlert,
  Wrench,
} from '@/components/Icons'
import type { ConversationEntry as Entry, ConversationTrace, ConversationDetail, ConversationIcon, ConversationActivityGroup, ConversationControl } from '@/remote/protocol'
import { remoteStore, useRemote } from '@/store/remoteStore'

const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true, typographer: true })

markdown.renderer.rules.fence = (tokens, index) => {
  const token = tokens[index]
  const language = token.info.trim().split(/\s+/)[0] || ''
  const escapedLanguage = markdown.utils.escapeHtml(language)
  const label = markdown.utils.escapeHtml(language ? `${language} code block` : 'Code block')
  const codeClass = escapedLanguage ? ` class="language-${escapedLanguage}"` : ''
  return `<pre tabindex="0" role="region" aria-label="${label}"><code${codeClass}>${markdown.utils.escapeHtml(token.content)}</code></pre>\n`
}

markdown.renderer.rules.code_block = (tokens, index) =>
  `<pre tabindex="0" role="region" aria-label="Code block"><code>${markdown.utils.escapeHtml(tokens[index].content)}</code></pre>\n`

function Markdown({ content }: { content: string }) {
  const html = useMemo(() => DOMPurify.sanitize(markdown.render(content || '')), [content])
  return <div className="markdown" dangerouslySetInnerHTML={{ __html: html }} />
}

async function writeClipboardText(text: string): Promise<boolean> {
  const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null

  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // Fall through to the selection-based fallback.
  }

  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.opacity = '0'
  textarea.style.pointerEvents = 'none'
  textarea.style.inset = '-10000px auto auto -10000px'
  document.body.append(textarea)

  let copied = false
  try {
    textarea.focus({ preventScroll: true })
    textarea.select()
    copied = document.execCommand('copy')
  } catch {
    copied = false
  } finally {
    textarea.remove()
    previousFocus?.focus({ preventScroll: true })
  }

  return copied
}

const traceIcons: Record<ConversationIcon, typeof Terminal> = {
  terminal: Terminal, edit: Pencil, file: FileText, search: Search, folder: Folder,
  screen: MacWindow, web: Globe, mcp: Plug, skill: BookOpen, tool: Wrench,
  reasoning: Lightbulb, info: Info, error: CircleAlert, copy: Copy,
  refresh: RotateCw, activity: Info,
}
function TraceDetails({ details, title, summary }: { details: ConversationDetail[]; title: string; summary?: string | null }) {
  return <div className="trace-disclosure__details">{details.filter(detail => ![title, summary].some(text => text && text.trim() === detail.content.trim())).map((detail, index) => {
    const scrollable = detail.content.length > 600 || detail.content.split('\n').length > 12
    return <div className={`trace-detail${detail.has_background ? ' has-background' : ''}`} key={`${detail.title}-${index}`}>
      {detail.title && <div className="trace-detail__title">{detail.title}</div>}
      <div className={['trace-detail__content', detail.monospaced ? 'is-monospaced' : '', detail.is_error ? 'is-error' : '', scrollable ? 'is-scrollable' : ''].filter(Boolean).join(' ')}
        tabIndex={scrollable ? 0 : undefined} role={scrollable ? 'region' : undefined} aria-label={scrollable ? `${detail.title || title} details` : undefined}>{detail.content}</div>
    </div>
  })}</div>
}
/** Status slot: spinner, padlock or dot, never a checkmark; completed rows stay empty. */
function TraceStatus({ trace }: { trace: ConversationTrace }) {
  const label = trace.status_label
  const kind = trace.active ? 'running'
    : label && /permission|approv/i.test(label) ? 'waiting'
    : label && /fail|error|denied|timed|cancel|interrupt/i.test(label) ? 'failed'
    : null
  return <span className="trace-disclosure__status" data-status={kind ?? undefined}>
    {kind === 'running' && <span className="mini-spinner" role="img" aria-label={label || 'Running'} />}
    {kind === 'waiting' && <Lock size={12} strokeWidth={1.8} role="img" aria-label={label || 'Awaiting approval'} />}
    {kind === 'failed' && <span className="trace-disclosure__failed" role="img" aria-label={label || 'Failed'} />}
    {kind === null && label && <span className="visually-hidden">{label}</span>}
  </span>
}
function TraceDisclosure({ trace }: { trace: ConversationTrace }) {
  const Icon = traceIcons[trace.icon]
  const expandable = trace.details.length > 0
  return <div className="trace-disclosure">
    <button className={`trace-disclosure__row${expandable ? ' is-expandable' : ''}`}
      onClick={() => expandable && remoteStore.sendConversationUi({ type: 'toggle', value: trace.id })}
      aria-expanded={expandable ? trace.expanded : undefined}>
      {trace.icon !== 'reasoning' && <span className="trace-disclosure__icon"><Icon size={12} strokeWidth={1.8} /></span>}
      <span className={`trace-disclosure__title${trace.active ? ' is-active' : ''}`}>{trace.title}</span>
      {trace.summary && <><span className="trace-disclosure__dot">·</span><span className="trace-disclosure__summary">{trace.summary}</span></>}
      <span className="trace-disclosure__spacer" />
      {trace.metadata && <span className="trace-disclosure__metadata">{trace.metadata}</span>}
      <TraceStatus trace={trace} />
      {expandable ? <ChevronDown size={9} className={trace.expanded ? 'is-open' : ''} /> : <span className="trace-disclosure__chevron-gap" />}
    </button>
    {trace.expanded && expandable && <TraceDetails details={trace.details} title={trace.title} summary={trace.summary} />}
  </div>
}
function ActivityGroupView({ group }: { group: ConversationActivityGroup }) {
  const lead = group.events[group.events.length - 1]?.trace.icon
  const GroupIcon = lead && lead !== 'reasoning' && lead !== 'activity' && lead !== 'info' ? traceIcons[lead] : null
  const groupIcon = GroupIcon ? <GroupIcon size={12} strokeWidth={1.8} /> : null
  return <div className="activity-group">
    <button className="activity-group__header" onClick={() => remoteStore.sendConversationUi({ type: 'toggle', value: group.id })} aria-expanded={group.expanded}>
      {groupIcon && <span className="activity-group__icon">{groupIcon}</span>}
      <span className={`activity-group__summary${group.active ? ' is-active' : ''}`}>{group.summary}</span>
      {group.awaits_permission && <Lock size={12} strokeWidth={1.8} role="img" aria-label="Awaiting approval" className="activity-group__lock" />}
      {group.active && <span className="mini-spinner" role="img" aria-label="Running" />}
      {group.failed && !group.active && <span className="trace-disclosure__failed" role="img" aria-label="Failed" />}
      <ChevronDown size={10} className={group.expanded ? 'is-open' : ''} />
    </button>
    {group.expanded && <div className="activity-group__events">{group.events.map(event => <div className="activity-group__event" key={event.key}><TraceDisclosure trace={event.trace} /></div>)}</div>}
  </div>
}

function InfoCard({
  icon,
  title,
  detail,
}: {
  icon: ReactNode
  title: string
  detail?: string | null
}) {
  return (
    <div className="info-card-wrap">
      <div className="info-card glass-panel">
        <span className="info-card__icon">{icon}</span>
        <span className="info-card__copy">
          <strong>{title}</strong>
          {detail && <small>{detail}</small>}
        </span>
      </div>
    </div>
  )
}

function MessageActionRow({ controls, copied, copyFailed }: { controls: ConversationControl[]; copied: boolean; copyFailed: boolean }) {
  return <div className="message-actions">{controls.map(control => {
    const copying = control.action.type === 'copy'
    const Icon = copying && copyFailed ? TriangleAlert : copying && copied ? Check : traceIcons[control.icon]
    const label = copying && copyFailed ? 'Copy failed' : copying && copied ? 'Copied' : control.label
    return <button key={control.action.type} disabled={!control.enabled} onClick={() => remoteStore.sendConversationUi(control.action)} aria-label={label} title={label}><Icon size={12} /></button>
  })}</div>
}
function ConversationEntry({ entry, controls, copied, copyFailed }: { entry: Entry; controls: ConversationControl[]; copied: boolean; copyFailed: boolean }) {
  switch (entry.kind.type) {
    case 'user':
    case 'assistant': {
      const user = entry.kind.type === 'user'
      return <div className={`message-block message-block--${user ? 'user' : 'assistant'}`}>
        <div className={`message-row message-row--${user ? 'user' : 'assistant'}`}><div className={user ? 'user-bubble' : 'assistant-message'}><Markdown content={entry.kind.content} /></div></div>
        <MessageActionRow controls={controls} copied={copied} copyFailed={copyFailed} />
      </div>
    }
    case 'error': return <InfoCard icon={<TriangleAlert size={14} />} title="Error" detail={entry.kind.content} />
    case 'system': return <InfoCard icon={<Info size={14} />} title="System" detail={entry.kind.content} />
    default: return null
  }
}

function StreamingAssistant({ text: content }: { text: string }) {
  const lines = content.split('\n')
  const last = lines.pop() ?? ''
  const rest = lines.join('\n')

  return (
    <div className="streaming-assistant">
      {rest && <div className="streaming-assistant__rest"><Markdown content={rest} /></div>}
      {last && <div className="streaming-assistant__last">{last}</div>}
    </div>
  )
}

interface ConversationScrollState {
  top: number
  following: boolean
}

const conversationScrollPositions = new Map<string, ConversationScrollState>()

export function Conversation({ onEditLast }: { onEditLast: (content: string) => void }) {
  const remote = useRemote()
  const scroller = useRef<HTMLDivElement>(null)
  const nearBottom = useRef(true)
  const [following, setFollowing] = useState(true)
  const viewKey = useMemo(() => JSON.stringify([
    remote.state.workspace_root ?? '',
    remote.state.current_session_id ?? null,
    remote.state.current_session_id == null ? remote.sessionResetRevision : 0,
  ]), [
    remote.state.workspace_root,
    remote.state.current_session_id,
    remote.sessionResetRevision,
  ])
  const [copiedEntryID, setCopiedEntryID] = useState<string | null>(null)
  const [copyFailedEntryID, setCopyFailedEntryID] = useState<string | null>(null)
  const copiedTimer = useRef<number | null>(null)

  const view = remote.conversation
  const displayItems = view?.items ?? []

  const copyMessage = async (content: string, id: string) => {
    const copied = await writeClipboardText(content)
    setCopiedEntryID(copied ? id : null)
    setCopyFailedEntryID(copied ? null : id)
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current)
    copiedTimer.current = window.setTimeout(() => {
      setCopiedEntryID(null)
      setCopyFailedEntryID(null)
    }, 1200)
  }

  const handledEffect = useRef<unknown>(null)
  useEffect(() => {
    const effect = remote.conversationEffect
    if (!effect || handledEffect.current === effect) return
    handledEffect.current = effect
    const delivered = remoteStore.consumeConversationEffect(effect.revision)
    if (delivered?.copy) void copyMessage(delivered.copy.text, delivered.copy.entry_id)
    if (delivered?.edit_draft != null) onEditLast(delivered.edit_draft)
  }, [remote.conversationEffect, onEditLast])

  const scrollLatest = (focus = false) => {
    const node = scroller.current
    if (!node) return
    nearBottom.current = true
    setFollowing(true)
    conversationScrollPositions.set(viewKey, { top: node.scrollHeight, following: true })
    node.scrollTo({ top: remote.entries.length ? node.scrollHeight : 0, behavior: 'auto' })
    if (focus) node.focus({ preventScroll: true })
  }

  useEffect(() => {
    const node = scroller.current
    if (!node || !nearBottom.current) return
    node.scrollTo({ top: node.scrollHeight, behavior: 'auto' })
  }, [
    remote.revision,
    remote.state.is_streaming,
    remote.state.active_assistant_text,
    remote.state.active_reasoning_text,
    remote.state.active_reasoning_summary,
  ])

  useEffect(() => {
    const node = scroller.current
    if (!node) return

    const saved = conversationScrollPositions.get(viewKey)
    const shouldFollow = saved?.following ?? true
    nearBottom.current = shouldFollow
    setFollowing(shouldFollow)

    const frame = requestAnimationFrame(() => {
      if (shouldFollow) {
        node.scrollTo({ top: node.scrollHeight, behavior: 'auto' })
        return
      }

      const maxTop = Math.max(0, node.scrollHeight - node.clientHeight)
      node.scrollTo({ top: Math.min(saved?.top ?? 0, maxTop), behavior: 'auto' })
    })
    return () => cancelAnimationFrame(frame)
  }, [viewKey])

  useEffect(() => {
    const node = scroller.current
    if (!node) return

    let frame = 0
    let followViewportChange = false

    const captureFollowPosition = () => {
      if (frame) return
      const distanceFromBottom = node.scrollHeight - node.scrollTop - node.clientHeight
      followViewportChange = distanceFromBottom < 160
    }

    const restoreFollowPosition = () => {
      if (frame) return
      frame = requestAnimationFrame(() => {
        frame = 0
        const shouldFollow = followViewportChange
        followViewportChange = false
        if (!shouldFollow) return
        nearBottom.current = true
        setFollowing(true)
        node.scrollTop = node.scrollHeight
      })
    }

    window.addEventListener('yeet:visual-viewport-will-change', captureFollowPosition)
    window.addEventListener('yeet:visual-viewport-change', restoreFollowPosition)
    return () => {
      if (frame) cancelAnimationFrame(frame)
      window.removeEventListener('yeet:visual-viewport-will-change', captureFollowPosition)
      window.removeEventListener('yeet:visual-viewport-change', restoreFollowPosition)
    }
  }, [])

  useEffect(() => {
    const node = scroller.current
    if (!node || typeof ResizeObserver === 'undefined') return

    let previousHeight = node.clientHeight
    const observer = new ResizeObserver(() => {
      const nextHeight = node.clientHeight
      if (nextHeight === previousHeight) return
      previousHeight = nextHeight
      if (nearBottom.current) node.scrollTop = node.scrollHeight
    })
    observer.observe(node)
    return () => observer.disconnect()
  }, [])

  useEffect(() => () => {
    if (copiedTimer.current) window.clearTimeout(copiedTimer.current)
  }, [])

  return (
    <div
      className="conversation-scroll"
      id="conversation-transcript"
      role="main"
      ref={scroller}
      data-testid="transcript"
      tabIndex={0}
      aria-label="Conversation transcript"
      onScroll={(event) => {
        const node = event.currentTarget
        const nextFollowing = node.scrollHeight - node.scrollTop - node.clientHeight < 160
        nearBottom.current = nextFollowing
        setFollowing(nextFollowing)
        conversationScrollPositions.set(viewKey, { top: node.scrollTop, following: nextFollowing })
      }}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget || event.altKey || event.ctrlKey || event.metaKey) return
        const node = event.currentTarget
        const pageDistance = Math.max(120, node.clientHeight - 80)
        if (event.key === 'PageUp') {
          event.preventDefault()
          node.scrollBy({ top: -pageDistance, behavior: 'auto' })
        } else if (event.key === 'PageDown') {
          event.preventDefault()
          node.scrollBy({ top: pageDistance, behavior: 'auto' })
        } else if (event.key === 'Home') {
          event.preventDefault()
          nearBottom.current = false
          setFollowing(false)
          node.scrollTo({ top: 0, behavior: 'auto' })
        } else if (event.key === 'End') {
          event.preventDefault()
          scrollLatest(false)
        }
      }}
    >
      <main className="conversation-content" aria-live="polite">
        {displayItems.map((item) => (
          item.type === 'activity'
            ? <ActivityGroupView key={item.id} group={item.group} />
            : item.type === 'reasoning'
              ? <div className="reasoning-trace" key={item.id}><TraceDisclosure trace={item.trace} /></div>
              : (
                <ConversationEntry
                  key={item.id}
                  entry={item.entry}
                  copied={copiedEntryID === item.entry.id}
                  copyFailed={copyFailedEntryID === item.entry.id}
                  controls={item.controls}
                />
              )
        ))}

        {view?.streaming && <div className="streaming-block">
          {view.streaming_text ? <StreamingAssistant text={view.streaming_text} /> : view.preparing && <div className="preparing-response"><span className="mini-spinner" /><span>{view.preparing_label}</span></div>}
        </div>}
        {view?.error && <InfoCard icon={<TriangleAlert size={14} />} title="Error" detail={view.error} />}

      </main>
      {!following && (
        <button
          className="conversation-latest"
          type="button"
          onClick={() => scrollLatest(true)}
        >
          ↓ Latest
        </button>
      )}
    </div>
  )
}
