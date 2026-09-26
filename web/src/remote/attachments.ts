export interface UploadedAttachment {
  id: string
  name?: string | null
  media_type: string
  size: number
}

export async function uploadAttachment(file: Blob, name: string, mediaType: string): Promise<UploadedAttachment> {
  const response = await fetch(`/api/attachments?name=${encodeURIComponent(name)}`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': mediaType || 'application/octet-stream' },
    body: file,
  })

  if (!response.ok) {
    const body = await response.json().catch(() => ({})) as { error?: string }
    throw new Error(body.error || `${response.status} ${response.statusText}`)
  }

  return response.json() as Promise<UploadedAttachment>
}

export async function deleteAttachment(id: string): Promise<void> {
  const response = await fetch(`/api/attachments/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    credentials: 'same-origin',
  })

  if (!response.ok && response.status !== 404) {
    const body = await response.json().catch(() => ({})) as { error?: string }
    throw new Error(body.error || `${response.status} ${response.statusText}`)
  }
}

export async function remoteFeatureSet(): Promise<Set<string>> {
  const response = await fetch('/api/protocol', {
    credentials: 'same-origin',
    cache: 'no-store',
  })
  if (!response.ok) return new Set()
  const value = await response.json().catch(() => ({})) as { features?: string[] }
  return new Set(Array.isArray(value.features) ? value.features : [])
}
