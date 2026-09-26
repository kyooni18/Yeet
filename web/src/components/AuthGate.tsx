import { useEffect, useRef, useState } from 'react'
import { Fingerprint, KeyRound, Lock } from '@/components/Icons'
import {
  fetchRemoteAuthStatus,
  loginWithAccessKey,
  loginWithPasskey,
  registerPasskey,
} from '@/remote/auth'
import { remoteStore } from '@/store/remoteStore'
import type { RemoteAuthStatus } from '@/remote/protocol'

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

function enrollmentTokenFromLocation(): string | null {
  if (location.pathname !== '/enroll') return null
  const search = new URLSearchParams(location.search)
  return search.get('token') || search.get('enroll')
}

function isAlreadyRegisteredPasskeyError(cause: unknown): boolean {
  return typeof cause === 'object'
    && cause !== null
    && 'name' in cause
    && (cause as { name?: unknown }).name === 'InvalidStateError'
}

export function AuthGate() {
  const enrollmentMode = location.pathname === '/enroll'
  const enrollmentToken = enrollmentTokenFromLocation()
  const [status, setStatus] = useState<RemoteAuthStatus | null>(null)
  const [statusLoaded, setStatusLoaded] = useState(false)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const dialog = useRef<HTMLDivElement>(null)
  const keyInput = useRef<HTMLInputElement>(null)
  const passkeySupported = typeof window.PublicKeyCredential !== 'undefined' && Boolean(navigator.credentials)

  const loadStatus = async () => {
    setStatusLoaded(false)
    setStatus(null)
    setStatusError(null)
    setError(null)
    try {
      setStatus(await fetchRemoteAuthStatus(enrollmentToken))
    } catch (cause) {
      setStatusError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setStatusLoaded(true)
    }
  }

  useEffect(() => {
    void loadStatus()
  }, [enrollmentToken])

  useEffect(() => {
    if (!statusLoaded) return
    const frame = requestAnimationFrame(() => {
      const target = keyInput.current
        ?? dialog.current?.querySelector<HTMLElement>(FOCUSABLE)
        ?? dialog.current
      target?.focus({ preventScroll: true })
    })
    return () => cancelAnimationFrame(frame)
  }, [statusLoaded, status?.key, status?.passkey, status?.enrollmentValid, enrollmentMode])

  const handleDialogKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab' || !dialog.current) return
    const focusables = [...dialog.current.querySelectorAll<HTMLElement>(FOCUSABLE)]
      .filter((element) => element.getClientRects().length > 0 && element.getAttribute('aria-hidden') !== 'true')
    if (!focusables.length) {
      event.preventDefault()
      dialog.current.focus({ preventScroll: true })
      return
    }

    const first = focusables[0]
    const last = focusables[focusables.length - 1]
    const active = document.activeElement
    if (event.shiftKey && (active === first || !active || !dialog.current.contains(active))) {
      event.preventDefault()
      last.focus({ preventScroll: true })
    } else if (!event.shiftKey && (active === last || !active || !dialog.current.contains(active))) {
      event.preventDefault()
      first.focus({ preventScroll: true })
    }
  }

  const signInWithKey = async () => {
    if (!key.trim() || busy) return
    setBusy(true)
    setError(null)
    try {
      await loginWithAccessKey(key.trim())
      setKey('')
      remoteStore.reconnectAfterAuth()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const signInWithPasskey = async () => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await loginWithPasskey()
      remoteStore.reconnectAfterAuth()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const enrollPasskey = async () => {
    if (!enrollmentToken || !status?.enrollmentValid || busy) return
    setBusy(true)
    setError(null)
    try {
      await registerPasskey(enrollmentToken)
      location.replace('/')
    } catch (cause) {
      if (status.passkey && isAlreadyRegisteredPasskeyError(cause)) {
        try {
          await loginWithPasskey(enrollmentToken)
          location.replace('/')
          return
        } catch (authCause) {
          const detail = authCause instanceof Error ? authCause.message : String(authCause)
          setError(`This passkey is already registered, but Yeet could not authenticate it: ${detail}`)
          return
        }
      }
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  const enrollmentValid = Boolean(enrollmentToken && status?.enrollmentValid)
  const combinedError = statusError || error

  return (
    <div
      ref={dialog}
      className="connection-screen auth-gate"
      role="dialog"
      aria-modal="true"
      aria-labelledby="auth-title"
      tabIndex={-1}
      onKeyDown={handleDialogKeyDown}
    >
      <section className="connection-card auth-card glass-panel">
        <div className="connection-card__mark"><Lock size={23} /></div>

        {enrollmentMode ? (
          <>
            <div className="connection-card__title">
              <h1 id="auth-title">Register a passkey</h1>
              {!statusLoaded && <span>Validating the one-time enrollment…</span>}
              {statusLoaded && statusError && (
                <span>Yeet Remote could not verify this enrollment link. Try again when the Remote service is reachable.</span>
              )}
              {statusLoaded && !statusError && enrollmentValid && (
                <span>This enrollment was authorized from the local Yeet terminal.</span>
              )}
              {statusLoaded && !statusError && !enrollmentValid && (
                <span>This passkey enrollment link is invalid or has expired.</span>
              )}
            </div>

            {statusLoaded && enrollmentValid && passkeySupported && (
              <button className="connection-primary" disabled={busy} onClick={() => void enrollPasskey()}>
                <Fingerprint size={17} />
                <span>{busy ? 'Registering…' : 'Register passkey'}</span>
              </button>
            )}
            {statusLoaded && enrollmentValid && !passkeySupported && (
              <p className="connection-muted">
                Passkeys are not available in this browser. Open this enrollment link in a WebAuthn-capable browser.
              </p>
            )}
            {statusLoaded && statusError && (
              <button className="connection-secondary" type="button" onClick={() => void loadStatus()}>Retry</button>
            )}
            {statusLoaded && (!enrollmentValid || !passkeySupported) && (
              <button className="connection-secondary" type="button" onClick={() => location.replace('/')}>Back to Yeet Remote</button>
            )}
          </>
        ) : (
          <>
            <div className="connection-card__title">
              <h1 id="auth-title">Authorization required</h1>
              <span>Authenticate to Yeet Remote, then select the workspace you want to use.</span>
            </div>

            {!statusLoaded && (
              <p
                className="connection-muted"
                role="status"
                aria-label="Loading authentication methods"
                aria-live="polite"
              >
                Loading authentication methods…
              </p>
            )}

            {statusLoaded && status?.key && (
              <form
                className="connection-key"
                onSubmit={(event) => {
                  event.preventDefault()
                  void signInWithKey()
                }}
              >
                <KeyRound size={16} />
                <input
                  ref={keyInput}
                  type="password"
                  value={key}
                  onChange={(event) => setKey(event.target.value)}
                  placeholder="yeet_…"
                  autoComplete="current-password"
                  aria-label="Access key"
                />
                <button type="submit" disabled={!key.trim() || busy}>{busy ? 'Authorizing…' : 'Authorize'}</button>
              </form>
            )}

            {statusLoaded && status?.passkey && passkeySupported && (
              <button className="connection-secondary" disabled={busy} onClick={() => void signInWithPasskey()}>
                <Fingerprint size={17} />
                <span>{busy ? 'Authenticating…' : 'Use a passkey'}</span>
              </button>
            )}
            {statusLoaded && status?.passkey && !passkeySupported && (
              <p className="connection-muted">
                This browser does not provide passkeys. Use the access key here or another WebAuthn-capable browser.
              </p>
            )}
            {statusLoaded && statusError && (
              <>
                <p className="connection-muted">
                  Yeet Remote could not load the available authentication methods. Try again when the service is reachable.
                </p>
                <button className="connection-secondary" type="button" onClick={() => void loadStatus()}>Retry</button>
              </>
            )}
            {statusLoaded && !statusError && status && !status.key && !status.passkey && (
              <p className="connection-muted">
                Passkey enrollment is pending. Open the one-time enrollment URL from the local Yeet terminal.
              </p>
            )}
          </>
        )}

        {combinedError && <div className="connection-error" role="alert">{combinedError}</div>}
      </section>
    </div>
  )
}
