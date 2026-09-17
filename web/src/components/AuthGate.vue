<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { fetchRemoteAuthStatus, loginWithAccessKey, loginWithPasskey, registerPasskey } from '@/remote/auth'
import { useRemoteStore } from '@/stores/remote'
import YeetMark from './YeetMark.vue'

const remote = useRemoteStore()
const route = useRoute()
const router = useRouter()
const key = ref('')
const error = ref('')
const statusError = ref('')
const working = ref(false)
const statusLoaded = ref(false)
const hasKey = ref(false)
const hasPasskey = ref(false)
const enrollmentValid = ref(false)
const passkeySupported = typeof window !== 'undefined'
  && typeof window.PublicKeyCredential !== 'undefined'
  && Boolean(navigator.credentials)
const dialog = ref<HTMLElement | null>(null)
const keyInput = ref<HTMLInputElement | null>(null)
let previouslyFocused: HTMLElement | null = null
const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

function focusableElements(): HTMLElement[] {
  const root = dialog.value
  if (!root) return []
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)]
    .filter((element) => element.getClientRects().length > 0 && element.getAttribute('aria-hidden') !== 'true')
}

function focusFirstAction() {
  const root = dialog.value
  if (!root) return
  const target = keyInput.value && keyInput.value.getClientRects().length > 0
    ? keyInput.value
    : focusableElements()[0]
  ;(target ?? root).focus({ preventScroll: true })
}

function handleDialogKeydown(event: KeyboardEvent) {
  if (event.key !== 'Tab') return
  const root = dialog.value
  if (!root) return
  const focusables = focusableElements()
  if (!focusables.length) {
    event.preventDefault()
    root.focus({ preventScroll: true })
    return
  }

  const first = focusables[0]
  const last = focusables[focusables.length - 1]
  const active = document.activeElement
  if (event.shiftKey && (active === first || !active || !root.contains(active))) {
    event.preventDefault()
    last.focus({ preventScroll: true })
  } else if (!event.shiftKey && (active === last || !active || !root.contains(active))) {
    event.preventDefault()
    first.focus({ preventScroll: true })
  }
}

onMounted(() => {
  const active = document.activeElement
  if (active instanceof HTMLElement && active !== document.body && active !== document.documentElement) {
    previouslyFocused = active
  }
  void nextTick(focusFirstAction)
})

watch([statusLoaded, hasKey, hasPasskey, enrollmentValid, () => route.name], () => {
  if (!statusLoaded.value) return
  void nextTick(() => {
    const root = dialog.value
    if (!root) return
    const active = document.activeElement
    if (!active || active === root || !root.contains(active)) focusFirstAction()
  })
})

onBeforeUnmount(() => {
  const target = previouslyFocused
  previouslyFocused = null
  if (target?.isConnected && !target.hasAttribute('disabled')) {
    void nextTick(() => target.focus({ preventScroll: true }))
  }
})
const enrollmentToken = computed(() => {
  if (route.name !== 'enroll') return null
  const value = route.query.token
  return typeof value === 'string' && value ? value : null
})
const enrollmentMode = computed(() => route.name === 'enroll')

async function loadAuthStatus(token = enrollmentToken.value) {
  statusLoaded.value = false
  error.value = ''
  statusError.value = ''
  hasKey.value = false
  hasPasskey.value = false
  enrollmentValid.value = false
  try {
    const status = await fetchRemoteAuthStatus(token)
    hasKey.value = status.key
    hasPasskey.value = status.passkey
    enrollmentValid.value = Boolean(token && status.enrollmentValid)
  } catch (cause) {
    statusError.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    statusLoaded.value = true
  }
}

watch(enrollmentToken, (token) => void loadAuthStatus(token), { immediate: true })

async function signInWithKey() {
  if (!key.value.trim() || working.value) return
  working.value = true
  error.value = ''
  try {
    await loginWithAccessKey(key.value.trim())
    key.value = ''
    remote.reconnectAfterAuth()
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    working.value = false
  }
}

async function signInWithPasskey() {
  if (working.value) return
  working.value = true
  error.value = ''
  try {
    await loginWithPasskey()
    remote.reconnectAfterAuth()
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    working.value = false
  }
}

function isAlreadyRegisteredPasskeyError(cause: unknown): boolean {
  return typeof cause === 'object'
    && cause !== null
    && 'name' in cause
    && (cause as { name?: unknown }).name === 'InvalidStateError'
}

async function enrollPasskey() {
  const token = enrollmentToken.value
  if (!token || !enrollmentValid.value || working.value) return
  working.value = true
  error.value = ''
  try {
    await registerPasskey(token)
    await router.replace('/')
    remote.reconnectAfterAuth()
  } catch (cause) {
    if (hasPasskey.value && isAlreadyRegisteredPasskeyError(cause)) {
      try {
        await loginWithPasskey(token)
        await router.replace('/')
        remote.reconnectAfterAuth()
        return
      } catch (authCause) {
        const detail = authCause instanceof Error ? authCause.message : String(authCause)
        error.value = `This passkey is already registered, but Yeet could not authenticate it: ${detail}`
        return
      }
    }
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    working.value = false
  }
}
</script>

<template>
  <div ref="dialog" class="auth-gate" role="dialog" aria-modal="true" aria-labelledby="auth-title" tabindex="-1" @keydown="handleDialogKeydown">
    <div class="auth-card surface-card">
      <div class="auth-mark"><YeetMark :size="30" /></div>
      <p class="eyebrow">Yeet Remote</p>
      <template v-if="enrollmentMode">
        <h1 id="auth-title">Register a passkey</h1>
        <p v-if="!statusLoaded" class="muted">Validating the one-time enrollment…</p>
        <p v-else-if="statusError" class="muted">Yeet Remote could not verify this enrollment link. Try again when the Remote service is reachable.</p>
        <p v-else-if="enrollmentValid" class="muted">This enrollment was authorized from the local Yeet terminal.</p>
        <p v-else class="muted">This passkey enrollment link is invalid or has expired.</p>

        <button
          v-if="statusLoaded && enrollmentValid && passkeySupported"
          class="primary-button auth-passkey"
          :disabled="working"
          @click="enrollPasskey"
        >{{ working ? 'Registering…' : 'Register passkey' }}</button>
        <p v-else-if="statusLoaded && enrollmentValid && !passkeySupported" class="muted">Passkeys are not available in this browser. Open this enrollment link in a WebAuthn-capable browser on a device that can sync the credential.</p>
        <button v-if="statusLoaded && statusError" class="secondary-button auth-passkey" type="button" @click="loadAuthStatus()">Retry</button>
        <button v-if="statusLoaded && (!enrollmentValid || !passkeySupported)" class="secondary-button auth-passkey" @click="router.replace('/')">Back to Yeet Remote</button>
      </template>

      <template v-else>
        <h1 id="auth-title">Authorization required</h1>
        <p class="muted">Authenticate to Yeet Remote, then select the workspace you want to use.</p>

        <p
          v-if="!statusLoaded"
          class="muted"
          role="status"
          aria-label="Loading authentication methods"
          aria-live="polite"
        >Loading authentication methods…</p>

        <form v-if="hasKey" class="auth-form" @submit.prevent="signInWithKey">
          <label for="remote-key">Access key</label>
          <input id="remote-key" ref="keyInput" v-model="key" type="password" autocomplete="current-password" placeholder="yeet_…" />
          <button class="primary-button" :disabled="working || !key.trim()">{{ working ? 'Authorizing…' : 'Authorize' }}</button>
        </form>

        <button v-if="hasPasskey && passkeySupported" class="secondary-button auth-passkey" :disabled="working" @click="signInWithPasskey">
          Use a passkey
        </button>
        <p v-else-if="hasPasskey && !passkeySupported" class="muted">This browser does not provide passkeys. Use the access key here, or use the same synced passkey from a WebAuthn-capable browser.</p>
        <p v-if="statusLoaded && statusError" class="muted">Yeet Remote could not load the available authentication methods. Try again when the service is reachable.</p>
        <button v-if="statusLoaded && statusError" class="secondary-button auth-passkey" type="button" @click="loadAuthStatus()">Retry</button>
        <p v-else-if="statusLoaded && !hasKey && !hasPasskey" class="muted">Passkey enrollment is pending. Open the one-time enrollment URL from the local Yeet terminal.</p>
      </template>
      <p v-if="statusError || error" class="error-text" role="alert">{{ statusError || error }}</p>
    </div>
  </div>
</template>


<style scoped>
@media (hover: none) and (pointer: coarse) and (min-width: 900px) {
  .auth-card button { min-height: 44px; }
  .auth-card input { min-height: 44px; font-size: 16px; }
}
</style>
