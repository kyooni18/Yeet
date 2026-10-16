import { State } from '@mun/core'
import type { ComposerSuggestion, EditorSnapshot } from '@/remote/protocol'
import { deleteAttachment, remoteFeatureSet, uploadAttachment } from '@/remote/attachments'
import { remoteStore } from '@/store/remoteStore'
import { connected, openSheet, remote } from './state'

export interface Attachment { id: string; name: string; remoteId: string | null; error: string | null }

const MAX_ATTACHMENTS = 8
const MAX_BYTES = 20 * 1024 * 1024

export const text = State('')
export const attachments = State<Attachment[]>([])
export const editingLast = State(false)
export const canAttach = State(false)

let revision = 0
let lastSignature = ''
let stashedDraft = ''

/**
 * The Rust composer projection owns submit/edit/slash-command rules; the
 * browser only reports its editor snapshot and applies returned effects.
 */
function editor(): EditorSnapshot {
  const snapshot = remote.value
  const context = snapshot.composer?.context ?? {
    workspace: snapshot.state.workspace_root ?? '',
    session_id: snapshot.state.current_session_id ?? null,
    unsaved_generation: snapshot.sessionResetRevision,
  }
  const content = {
    context,
    text: text.value,
    mode: editingLast.value
      ? { type: 'edit_last' as const, has_attachments: attachments.value.length > 0 }
      : { type: 'draft' as const },
    attachments: attachments.value.map(item => ({
      id: item.id, name: item.name, attachment_id: item.remoteId,
      ready: !!item.remoteId && !item.error, error: item.error,
    })),
  }
  const signature = JSON.stringify(content)
  if (signature !== lastSignature) { lastSignature = signature; revision += 1 }
  return { ...content, revision }
}

const same = (a: EditorSnapshot, b: EditorSnapshot) =>
  a.revision === b.revision && a.text === b.text && JSON.stringify(a.context) === JSON.stringify(b.context)

function sync() {
  if (connected() && remote.value.composer) remoteStore.sendComposerUi({ type: 'update_editor', value: editor() })
}

export function setText(value: string) {
  text.value = value
  sync()
}

export function submit() {
  if (!remote.value.composer?.can_submit && !text.value.trim()) return
  remoteStore.sendComposerUi({ type: 'submit', value: editor() })
}

export function primary() {
  const control = remote.value.composer?.primary_control
  if (control?.action.type === 'interrupt') remoteStore.sendComposerUi(control.action)
  else submit()
}

export function cancelEdit() {
  remoteStore.sendComposerUi({ type: 'cancel_edit', value: editor() })
}

export function chooseSuggestion(item: ComposerSuggestion) {
  remoteStore.sendComposerUi({ type: 'select_suggestion', value: { editor: editor(), command: item.command } })
}

export function suggestions(): ComposerSuggestion[] {
  return text.value.startsWith('/') ? (remote.value.composer?.suggestions ?? []).slice(0, 6) : []
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

function update(id: string, patch: Partial<Attachment>) {
  attachments.value = attachments.value.map(item => item.id === id ? { ...item, ...patch } : item)
  sync()
}

export function addFiles(files: FileList | File[] | null) {
  const list = Array.from(files ?? []).slice(0, Math.max(0, MAX_ATTACHMENTS - attachments.value.length))
  for (const file of list) {
    const id = crypto.randomUUID()
    const tooLarge = file.size > MAX_BYTES
    attachments.value = [...attachments.value, {
      id, name: file.name || 'attachment', remoteId: null, error: tooLarge ? 'Files must be 20 MiB or smaller.' : null,
    }]
    if (tooLarge) continue
    uploadAttachment(file, file.name || 'attachment', file.type || 'application/octet-stream')
      .then(uploaded => update(id, { remoteId: uploaded.id }))
      .catch(error => update(id, { error: error instanceof Error ? error.message : String(error) }))
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
  if (!editingLast.value) stashedDraft = text.value
  editingLast.value = true
  setText(draft)
}

let handledComposer = 0
remoteStore.subscribe(() => {
  const pending = remoteStore.getSnapshot().composerEffect
  if (!pending || pending.revision === handledComposer) return
  handledComposer = pending.revision
  const effect = remoteStore.consumeComposerEffect(pending.revision)
  if (!effect) return
  const current = editor()
  const accepted = effect.accepted_editor ?? effect.cancel_edit
  if (accepted && same(accepted, current)) {
    if (accepted.mode.type === 'edit_last') {
      editingLast.value = false
      text.value = stashedDraft
      stashedDraft = ''
    } else {
      text.value = ''
      attachments.value = []
    }
  }
  if (effect.replace_editor && same(effect.replace_editor.editor, current)) text.value = effect.replace_editor.text
  if (effect.destination === 'models') openSheet('model')
  else if (effect.destination) openSheet('settings')
})

void remoteFeatureSet().then(features => {
  canAttach.value = features.has('attachments-v1')
}).catch(() => {})
