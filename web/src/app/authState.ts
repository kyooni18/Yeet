import { State } from '@mun/core'
import { fetchRemoteAuthStatus, loginWithAccessKey, loginWithPasskey, registerPasskey } from '@/remote/auth'
import type { RemoteAuthStatus } from '@/remote/protocol'
import { remoteStore } from '@/store/remoteStore'
import { bindElement } from './dom'

export const key = State('')
export const error = State('')
export const status = State<RemoteAuthStatus | null>(null)
export const loading = State(true)
export const busy = State(false)
export const enrolling = location.pathname === '/enroll'
const params = new URLSearchParams(location.search)
const token = params.get('token') || params.get('enroll')
let generation = 0
let root: HTMLElement | null = null
const message = (failure: unknown) => failure instanceof Error ? failure.message : String(failure)

export async function loadStatus() {
  const attempt = ++generation
  loading.value = true
  error.value = ''
  status.value = null
  try {
    const result = await fetchRemoteAuthStatus(enrolling ? token : null)
    if (attempt !== generation) return
    status.value = result
  } catch (failure) {
    if (attempt === generation) error.value = message(failure)
  } finally {
    if (attempt === generation) {
      loading.value = false
      requestAnimationFrame(() => root?.querySelector<HTMLElement>('input, button:not(:disabled)')?.focus())
    }
  }
}

export async function authorize(method: 'key' | 'passkey' | 'enroll') {
  if (busy.value || loading.value || !status.value) return
  if (method === 'key' && (!status.value.key || !key.value.trim())) return
  if (method === 'enroll' && (!token || !status.value.enrollmentValid)) return
  busy.value = true
  error.value = ''
  try {
    if (method === 'key') await loginWithAccessKey(key.value.trim())
    else if (method === 'passkey') await loginWithPasskey()
    else {
      try { await registerPasskey(token!) }
      catch (failure) {
        if (!(failure instanceof DOMException) || failure.name !== 'InvalidStateError') throw failure
        await loginWithPasskey(token!)
      }
    }
    key.value = ''
    if (enrolling) location.replace('/')
    else remoteStore.reconnectAfterAuth()
  } catch (failure) { error.value = message(failure) }
  finally { busy.value = false }
}

export const authRef = bindElement(element => {
  root = element
  void loadStatus()
  const trap = (event: KeyboardEvent) => {
    if (event.key !== 'Tab') return
    const items = [...element.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled)')]
      .filter(item => item.getClientRects().length)
    const first = items[0], last = items.at(-1)
    if (!first) { event.preventDefault(); element.focus(); return }
    if (event.shiftKey && (document.activeElement === first || !element.contains(document.activeElement))) {
      event.preventDefault(); last?.focus()
    } else if (!event.shiftKey && (document.activeElement === last || !element.contains(document.activeElement))) {
      event.preventDefault(); first.focus()
    }
  }
  element.addEventListener('keydown', trap)
  return () => { ++generation; root = null; key.value = ''; element.removeEventListener('keydown', trap) }
})
