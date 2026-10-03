import { Platform } from 'react-native'
import * as DocumentPicker from 'expo-document-picker'
import { File } from 'expo-file-system'
import { remoteRequest } from './remote'
import { isDesktopHost } from './desktop/transport'

export interface UploadedAttachment { id: string; name?: string | null; media_type: string; size: number }
export async function pickAttachment(): Promise<UploadedAttachment | null> {
  if (isDesktopHost()) throw new Error('Local desktop attachments are not exposed by the Harness. Use the Remote client for uploads.')
  const result = await DocumentPicker.getDocumentAsync({ copyToCacheDirectory: true, multiple: false, base64: false })
  if (result.canceled) return null
  const asset = result.assets[0]
  const body = Platform.OS === 'web' && asset.file ? asset.file : await new File(asset.uri).arrayBuffer()
  return remoteRequest(`/api/attachments?name=${encodeURIComponent(asset.name)}`, {
    method: 'POST', headers: { 'content-type': asset.mimeType ?? 'application/octet-stream' }, body,
  })
}
export async function deleteRemoteAttachment(id: string): Promise<void> {
  await remoteRequest(`/api/attachments/${encodeURIComponent(id)}`, { method: 'DELETE' })
}
