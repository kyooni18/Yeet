import { expect, test } from '../../../web/node_modules/@playwright/test/index.js'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import ts from '../node_modules/typescript/lib/typescript.js'
import { emptyBridgeState, type RemoteServerMessage } from '../../shared/remote/protocol'
import type { RemoteTransportEvents } from '../../shared/remote/transport'

// Execute the platform adapter in isolation, without a Tauri installation or Rust
// process. Type-only protocol imports disappear; the bridge is its sole dependency.
function loadTransport() {
  const source = readFileSync(resolve(__dirname, '../src/platform/desktop/transport.ts'), 'utf8')
  const javascript = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText
  const exports: { DesktopTransport?: new (events: RemoteTransportEvents) => { connect(): Promise<void>; close(): void; setWorkspace(path: string): void } } = {}
  new Function('exports', javascript)(exports)
  return exports.DesktopTransport!
}

test('desktop bridge preserves omitted conversation, orders buffered events and ignores a closed pending connection', async () => {
  const Transport = loadTransport()
  const host = globalThis as typeof globalThis & { __TAURI__?: unknown }
  const original = host.__TAURI__
  const messages: RemoteServerMessage[] = []
  let opened = 0
  let callback: ((event: { payload: unknown }) => void) | undefined
  let connectedState = emptyBridgeState()
  connectedState.conversation_revision = 1
  connectedState.conversation = [{ id: 'first', kind: { type: 'assistant', content: 'Authoritative transcript' } }]
  let resolveConnect: ((value: { workspace: string; state: typeof connectedState }) => void) | undefined
  let unsubscribeCalls = 0
  host.__TAURI__ = {
    event: { listen: async (_name: string, listener: typeof callback) => { callback = listener; return () => unsubscribeCalls++ } },
    core: { invoke: (command: string) => command === 'connect_core' ? new Promise(resolve => { resolveConnect = resolve }) : Promise.resolve() },
  }
  const events: RemoteTransportEvents = { onOpen: () => opened++, onMessage: message => messages.push(message), onStatus: () => {}, onError: error => { throw new Error(error) } }
  const transport = new Transport(events)
  transport.setWorkspace('/workspace')
  try {
    const connecting = transport.connect()
    await expect.poll(() => !!resolveConnect).toBe(true)
    callback!({ payload: { type: 'state', state: { ...emptyBridgeState(), conversation: null, conversation_revision: 2, active_model: 'updated-model' }, message: null } })
    resolveConnect!({ workspace: '/workspace', state: connectedState })
    await connecting
    expect(opened).toBe(1)
    expect(messages.map(message => message.type)).toEqual(['welcome', 'snapshot', 'snapshot'])
    const final = messages.at(-1)
    expect(final?.type).toBe('snapshot')
    if (final?.type !== 'snapshot') throw new Error('Expected authoritative snapshot')
    expect(final.state.conversation?.[0].kind).toEqual({ type: 'assistant', content: 'Authoritative transcript' })
    expect(final.state.active_model).toBe('updated-model')
    transport.close()
    messages.length = 0
    resolveConnect = undefined
    const pending = transport.connect()
    await expect.poll(() => !!resolveConnect).toBe(true)
    transport.close()
    resolveConnect!({ workspace: '/workspace', state: connectedState })
    await pending
    expect(messages).toEqual([])
    expect(opened).toBe(1)
    expect(unsubscribeCalls).toBe(2)
  } finally { transport.close(); host.__TAURI__ = original }
})
