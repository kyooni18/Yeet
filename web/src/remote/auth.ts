import type { RemoteAuthStatus } from './protocol'

const jsonRequest = async <T>(path: string, init?: RequestInit): Promise<T> => {
  const response = await fetch(path, {
    credentials: 'same-origin',
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(init?.headers ?? {}),
    },
  })
  if (!response.ok) {
    const body = await response.json().catch(() => ({})) as { error?: string }
    throw new Error(body.error || `${response.status} ${response.statusText}`)
  }
  return response.json() as Promise<T>
}

export const fetchRemoteAuthStatus = (enrollmentToken?: string | null) =>
  jsonRequest<RemoteAuthStatus>(
    `/api/auth/status${enrollmentToken ? `?enroll=${encodeURIComponent(enrollmentToken)}` : ''}`,
  )

export const loginWithAccessKey = (key: string) =>
  jsonRequest<{ ok: boolean }>('/api/auth/key', {
    method: 'POST',
    body: JSON.stringify({ key }),
  })

const base64UrlToBytes = (value: string): ArrayBuffer => {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '=')
  const binary = atob(padded)
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  return bytes.buffer as ArrayBuffer
}

const bytesToBase64Url = (value: ArrayBuffer): string => {
  const binary = String.fromCharCode(...new Uint8Array(value))
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '')
}

export async function loginWithPasskey(enrollmentToken?: string): Promise<void> {
  if (!navigator.credentials) throw new Error('Passkeys are not available in this browser.')
  const begin = await jsonRequest<Record<string, unknown>>('/api/auth/passkey/begin', { method: 'POST', body: '{}' })
  const transaction = String(begin.transaction ?? '')
  const options = (begin.options ?? {}) as Record<string, unknown>
  const publicKey = (options.publicKey ?? options.public_key) as Record<string, unknown> | undefined
  if (!transaction || !publicKey?.challenge) throw new Error('Yeet Remote returned an invalid passkey challenge.')
  const allowCredentials = Array.isArray(publicKey.allowCredentials)
    ? publicKey.allowCredentials.map((item) => {
        const credential = item as Record<string, unknown>
        return { ...credential, id: base64UrlToBytes(String(credential.id)) }
      })
    : undefined
  const request: PublicKeyCredentialRequestOptions = {
    ...(publicKey as unknown as PublicKeyCredentialRequestOptions),
    challenge: base64UrlToBytes(String(publicKey.challenge)),
    allowCredentials: allowCredentials as PublicKeyCredentialDescriptor[] | undefined,
  }
  const credential = await navigator.credentials.get({ publicKey: request }) as PublicKeyCredential | null
  if (!credential) throw new Error('Passkey authentication was cancelled.')
  const response = credential.response as AuthenticatorAssertionResponse
  await jsonRequest('/api/auth/passkey/finish', {
    method: 'POST',
    body: JSON.stringify({
      transaction,
      enrollment_token: enrollmentToken,
      credential: {
        id: credential.id,
        rawId: bytesToBase64Url(credential.rawId),
        type: credential.type,
        response: {
          authenticatorData: bytesToBase64Url(response.authenticatorData),
          clientDataJSON: bytesToBase64Url(response.clientDataJSON),
          signature: bytesToBase64Url(response.signature),
          userHandle: response.userHandle ? bytesToBase64Url(response.userHandle) : null,
        },
        extensions: credential.getClientExtensionResults(),
      },
    }),
  })
}

export async function registerPasskey(token: string): Promise<void> {
  if (!navigator.credentials) throw new Error('Passkeys are not available in this browser.')
  const begin = await jsonRequest<Record<string, unknown>>('/api/auth/passkey/register/begin', {
    method: 'POST',
    body: JSON.stringify({ token }),
  })
  const transaction = String(begin.transaction ?? '')
  const options = (begin.options ?? {}) as Record<string, unknown>
  const publicKey = (options.publicKey ?? options.public_key) as Record<string, unknown> | undefined
  const user = publicKey?.user as Record<string, unknown> | undefined
  if (!transaction || !publicKey?.challenge || !user?.id) {
    throw new Error('Yeet Remote returned an invalid passkey enrollment challenge.')
  }

  const excludeCredentials = Array.isArray(publicKey.excludeCredentials)
    ? publicKey.excludeCredentials.map((item) => {
        const credential = item as Record<string, unknown>
        return { ...credential, id: base64UrlToBytes(String(credential.id)) }
      })
    : undefined
  const request: PublicKeyCredentialCreationOptions = {
    ...(publicKey as unknown as PublicKeyCredentialCreationOptions),
    challenge: base64UrlToBytes(String(publicKey.challenge)),
    user: {
      ...(user as unknown as PublicKeyCredentialUserEntity),
      id: base64UrlToBytes(String(user.id)),
    },
    excludeCredentials: excludeCredentials as PublicKeyCredentialDescriptor[] | undefined,
  }
  const credential = await navigator.credentials.create({ publicKey: request }) as PublicKeyCredential | null
  if (!credential) throw new Error('Passkey enrollment was cancelled.')
  const response = credential.response as AuthenticatorAttestationResponse
  const transports = typeof response.getTransports === 'function' ? response.getTransports() : undefined
  await jsonRequest('/api/auth/passkey/register/finish', {
    method: 'POST',
    body: JSON.stringify({
      transaction,
      credential: {
        id: credential.id,
        rawId: bytesToBase64Url(credential.rawId),
        type: credential.type,
        response: {
          attestationObject: bytesToBase64Url(response.attestationObject),
          clientDataJSON: bytesToBase64Url(response.clientDataJSON),
          transports,
        },
        extensions: credential.getClientExtensionResults(),
      },
    }),
  })
}
