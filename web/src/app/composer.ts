import { State } from '@mun/core'
import type { ComposerContext, ComposerSuggestion, EditorSnapshot } from '@/remote/protocol'
import { deleteAttachment, remoteFeatureSet, uploadAttachment } from '@/remote/attachments'
import { remoteStore } from '@/store/remoteStore'
import { connected, navigateDestination, remote } from './state'

export interface Attachment { id: string; name: string; remoteId: string | null; error: string | null }

const MAX_ATTACHMENTS = 8
const MAX_BYTES = 20 * 1024 * 1024

export const text = State('')
export const attachments = State<Attachment[]>([])
export const editingLast = State(false)
export const canAttach = State(false)

let revision = 0
interface Draft {
  context: ComposerContext
  text: string
  attachments: Attachment[]
  editingLast: boolean
  stashedDraft: string
  revision: number
  signature: string
}
const drafts = new Map<string, Draft>()
const contextKey = (context: ComposerContext) => JSON.stringify([
  context.workspace, context.session_id, context.session_id == null ? context.unsaved_generation : 0,
])
function currentContext(): ComposerContext {
  const snapshot = remoteStore.getSnapshot()
  const workspace = snapshot.state.workspace_root ?? ''
  const session_id = snapshot.state.current_session_id ?? null
  const projected = snapshot.composer?.context
  return { workspace, session_id, unsaved_generation:
    projected?.workspace === workspace && projected.session_id === session_id
      ? Math.max(projected.unsaved_generation, session_id == null ? snapshot.sessionResetRevision : 0) : snapshot.sessionResetRevision }
}
function newDraft(context: ComposerContext): Draft {
  return { context, text: '', attachments: [], editingLast: false, stashedDraft: '', revision: 0, signature: '' }
}
let active = newDraft(currentContext())
drafts.set(contextKey(active.context), active)
function saveActive() {
  active.text = text.value
  active.attachments = attachments.value
  active.editingLast = editingLast.value
}
function showActive() {
  text.value = active.text
  attachments.value = active.attachments
  editingLast.value = active.editingLast
}
function reconcileContext() {
  const context = currentContext()
  const key = contextKey(context)
  if (contextKey(active.context) !== key) {
    cancelSync()
    saveActive()
    active = drafts.get(key) ?? newDraft(context)
    drafts.set(key, active)
    showActive()
  }
  active.context = context
}
let editorFrame = 0
const cancelSync = () => { cancelAnimationFrame(editorFrame); editorFrame = 0 }

/**
 * The Rust composer projection owns submit/edit/slash-command rules; the
 * browser only reports its editor snapshot and applies returned effects.
 */
function editor(draft = active): EditorSnapshot {
  if (draft === active) saveActive()
  const context = draft.context
  const content = {
    context,
    text: draft.text,
    mode: draft.editingLast
      ? { type: 'edit_last' as const, has_attachments: draft.attachments.length > 0 }
      : { type: 'draft' as const },
    attachments: draft.attachments.map(item => ({
      id: item.id, name: item.name, attachment_id: item.remoteId,
      ready: !!item.remoteId && !item.error, error: item.error,
    })),
  }
  const signature = JSON.stringify(content)
  if (signature !== draft.signature) { draft.signature = signature; draft.revision = ++revision }
  return { ...content, revision: draft.revision }
}

const same = (a: EditorSnapshot, b: EditorSnapshot) =>
  a.revision === b.revision && a.text === b.text && a.context.workspace === b.context.workspace && a.context.session_id === b.context.session_id
    && a.context.unsaved_generation === b.context.unsaved_generation

function contextReady(): boolean {
  const projected = remoteStore.getSnapshot().composer?.context
  return !!projected && projected.workspace === active.context.workspace
    && projected.session_id === active.context.session_id
    && projected.unsaved_generation === active.context.unsaved_generation
}

function sync() {
  cancelSync()
  if (connected() && contextReady()) remoteStore.sendComposerUi({ type: 'update_editor', value: editor() })
}

export function setText(value: string) {
  reconcileContext()
  text.value = value
  // Coalesce same-frame input events without delaying local input or submit.
  if (!editorFrame) editorFrame = requestAnimationFrame(sync)
}

export function submit() {
  reconcileContext()
  cancelSync()
  if (!connected() || !contextReady()) return
  if (!remote.value.composer?.can_submit && !text.value.trim()) return
  remoteStore.sendComposerUi({ type: 'submit', value: editor() })
}

/** Allow a freshly typed draft while its projection is in flight; Rust still validates submission. */
export function primaryEnabled(): boolean {
  const view = remote.value.composer
  if (!connected() || !view || !contextReady()) return false
  if (remote.value.state.is_streaming) return view.primary_control.enabled
  if (same(view.editor, editor())) return view.primary_control.enabled
  return !!text.value.trim() && attachments.value.every(item => !!item.remoteId && !item.error)
}

export function primary() {
  const control = remote.value.composer?.primary_control
  if (control?.action.type === 'interrupt') remoteStore.sendComposerUi(control.action)
  else submit()
}

export function cancelEdit() {
  cancelSync()
  remoteStore.sendComposerUi({ type: 'cancel_edit', value: editor() })
}

export function chooseSuggestion(item: ComposerSuggestion) {
  cancelSync()
  remoteStore.sendComposerUi({ type: 'select_suggestion', value: { editor: editor(), command: item.command } })
}

export function suggestions(): ComposerSuggestion[] {
  return text.value.startsWith('/') ? (remote.value.composer?.suggestions ?? []).slice(0, 6) : []
}

/** Mobile: the whole bubble focuses the field; controls keep their own behavior. */
export function focusComposer(event: MouseEvent) {
  const target = event.target as HTMLElement | null
  if (target?.closest('button, select, textarea, input, a, [role="button"]')) return
  ;(event.currentTarget as HTMLElement | null)?.querySelector('textarea')?.focus()
}

export function onKeyDown(event: KeyboardEvent) {
  if (event.key !== 'Enter' || event.shiftKey || event.isComposing) return
  const touch = window.matchMedia('(hover: none) and (pointer: coarse)').matches
  if (!event.metaKey && !event.ctrlKey && touch) return
  event.preventDefault()
  const first = suggestions()[0]
  if (first && !text.value.includes(' ')) chooseSuggestion(first)
  else submit()
}

function update(owner: Draft, id: string, patch: Partial<Attachment>) {
  if (owner === active) saveActive()
  if (!owner.attachments.some(item => item.id === id)) {
    // An upload may finish after its row was removed.
    if (patch.remoteId) void deleteAttachment(patch.remoteId).catch(() => {})
    return
  }
  owner.attachments = owner.attachments.map(item => item.id === id ? { ...item, ...patch } : item)
  if (owner === active) { showActive(); sync() }
}

export function addFiles(files: FileList | File[] | null) {
  reconcileContext()
  const owner = active
  const list = Array.from(files ?? []).slice(0, Math.max(0, MAX_ATTACHMENTS - attachments.value.length))
  for (const file of list) {
    const id = crypto.randomUUID()
    const tooLarge = file.size > MAX_BYTES
    attachments.value = [...attachments.value, {
      id, name: file.name || 'attachment', remoteId: null, error: tooLarge ? 'Files must be 20 MiB or smaller.' : null,
    }]
    saveActive()
    if (tooLarge) continue
    uploadAttachment(file, file.name || 'attachment', file.type || 'application/octet-stream')
      .then(uploaded => update(owner, id, { remoteId: uploaded.id }))
      .catch(error => update(owner, id, { error: error instanceof Error ? error.message : String(error) }))
  }
  sync()
}

export function removeAttachment(id: string) {
  const item = attachments.value.find(candidate => candidate.id === id)
  attachments.value = attachments.value.filter(candidate => candidate.id !== id)
  if (item?.remoteId) void deleteAttachment(item.remoteId).catch(() => {})
  sync()
}

export function pickFiles() {
  const input = document.createElement('input')
  input.type = 'file'
  input.multiple = true
  input.onchange = () => addFiles(input.files)
  input.click()
}

/** Conversation "edit" control: load the last prompt into the composer. */
export function beginEdit(draft: string) {
  reconcileContext()
  if (!editingLast.value) active.stashedDraft = text.value
  editingLast.value = true
  setText(draft)
}

remoteStore.subscribe(() => {
  saveActive()
  const pending = remoteStore.getSnapshot().composerEffect
  const effect = pending ? remoteStore.consumeComposerEffect(pending.revision) : null
  const accepted = effect?.accepted_editor ?? effect?.cancel_edit
  const owner = accepted ? drafts.get(contextKey(accepted.context)) : undefined
  if (owner && accepted && same(accepted, editor(owner))) {
    if (accepted.mode.type === 'edit_last') {
      owner.editingLast = false
      owner.text = owner.stashedDraft
      owner.stashedDraft = ''
    } else {
      owner.text = ''
      owner.attachments = []
    }
    if (owner === active) showActive()
  }
  if (effect?.replace_editor) {
    const replacement = effect.replace_editor
    const owner = drafts.get(contextKey(replacement.editor.context))
    if (owner && same(replacement.editor, editor(owner))) {
      owner.text = replacement.text
      if (owner === active) showActive()
    }
  }
  reconcileContext()
  if (effect?.destination) navigateDestination(effect.destination)
})

void remoteFeatureSet().then(features => {
  canAttach.value = features.has('attachments-v1')
}).catch(() => {})
