# Shared Rust Remote client

These modules contain the existing Remote v1 contract and semantic client state,
shared by the browser, Expo and desktop frontends. Rust remains authoritative for
sessions, tools, permissions, providers, settings and agent execution. UI layout
intent lives separately in `app/ui/`; these modules never interpret `.ui` files.

- `remote/protocol.ts`: state, command and event DTOs and protocol codecs.
- `remote/transport.ts`: WebSocket handshake, sequencing, reconnect and heartbeat.
- `state/remoteStore.ts`: semantic events, streaming reconciliation and subscription.

The transport receives `RemoteTransportAdapter` platform services. The adapter
resolves HTTP paths and WebSocket URLs against the selected endpoint, supplies
HTTP requests/socket construction, storage, connectivity, time and timers. Keep
browser cookies and WebAuthn in browser adapters; use native credential/session
handling in native adapters. Sharing these contracts does not bypass Rust Remote
authentication or its origin checks.

Identity storage must be scoped to a server and client surface. The browser uses
session storage to preserve independent tabs. Native apps can hydrate an in-memory
identity adapter from their platform storage before constructing the transport.
Only client/workspace/session identity is persisted; replay sequence/revision
cursors remain in memory because a restarted client needs a fresh snapshot.

`RemoteStore` receives a transport factory, frame scheduler and optional
connectivity subscription. `RemoteClientTransport` is a structural contract so a
local Tauri bridge can supply the same Rust state/events without a remote socket.
The store has no React dependency: frontend hooks should call their own React
`useSyncExternalStore(store.subscribe, store.getSnapshot, store.getServerSnapshot)`.
The browser wrapper also exports a configurable `useRemote(store)` hook.

The browser wrappers under `web/src/remote` and `web/src/store` retain the existing
singleton and same-origin behavior. Native adapters must handle reconnects,
credentials and platform lifecycle explicitly; never duplicate the semantic
streaming reducer in a platform frontend.
