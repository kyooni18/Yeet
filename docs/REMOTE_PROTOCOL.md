# Yeet Remote semantic protocol

Protocol version: `1`

This document is the wire contract between the Yeet Remote backend and a first-class browser frontend. It is intentionally frontend-neutral: no message contains Ratatui cells, terminal dimensions, fake keyboard events, rendered HTML, or `App` state.

The semantic backend is exposed at `GET /api/ws` over a persistent WebSocket. The legacy browser-TUI endpoints `/api/frame` and `/api/input` remain available during migration, but they are not part of this protocol and a semantic WebUI must not use them.

## Discovery and authentication

`GET /api/protocol` returns the currently supported protocol range without exposing workspace filesystem metadata before authentication:

```json
{
  "version": 1,
  "minVersion": 1,
  "maxVersion": 1,
  "websocket": "/api/ws"
}
```

Browser authentication remains HTTP-based and precedes the WebSocket upgrade:

| Endpoint | Purpose |
| --- | --- |
| `GET /api/auth/status` | Reports whether auth is required and whether access-key/passkey login is available. `?enroll=TOKEN` also reports `enrollmentValid`. |
| `POST /api/auth/key` | Access-key login: `{ "key": "..." }`. |
| `POST /api/auth/passkey/begin` | Starts passkey authentication. |
| `POST /api/auth/passkey/finish` | Finishes passkey authentication. |
| `POST /api/auth/passkey/register/begin` | Starts one-time enrollment with `{ "token": "..." }`. |
| `POST /api/auth/passkey/register/finish` | Finishes passkey enrollment. |

Successful access-key or passkey authentication issues a Remote-wide session cookie. Authentication is deliberately independent of the workspace selected after login, so one access key or synced WebAuthn passkey can be reused by multiple clients and Remote instances that use the same Remote credential store. The cookie is `HttpOnly`, `SameSite=Strict`, has a 12-hour max age, and gains `Secure` whenever the configured public origin is HTTPS. Access keys remain stored as Argon2 password hashes; passkeys remain backed by the WebAuthn credential store and enrollment flow. WebAuthn still scopes credentials to the configured relying-party ID/origin as required by the standard; workspace paths do not participate in passkey identity. Browser auth POSTs validate `Origin` whenever the browser supplies it; non-browser clients without an `Origin` remain supported.

`/api/ws` requires a valid authenticated session when Remote auth is enabled. The server rejects an unauthenticated upgrade with HTTP `401`, and rejects a browser `Origin` that does not match the configured public origin (or, without an explicit public origin, the request `Host`) with HTTP `403`. Browser clients must send an `Origin`; a missing origin is rejected. The preferred WebSocket subprotocol is `yeet.remote.v1`.

## Framing and naming

All application messages are UTF-8 JSON text WebSocket messages. Binary application messages are invalid. Rust enum tags and protocol fields use `snake_case` unless an existing Yeet model type already defines compatibility casing, such as `ConversationKind.toolCalls`, `ConversationToolCall.callID`, `NativeAppPermission.bundleId`, and `NativeAppPermission.appName`.

The gateway limits both WebSocket messages and individual frames to 256 KiB. Clients should send text incrementally (especially user attachments/large metadata) rather than attempting oversized single protocol messages.

Every connection starts with exactly one `hello` message. No command may precede it.

### Client hello

```json
{
  "type": "hello",
  "min_version": 1,
  "max_version": 1,
  "client_id": "browser-tab-uuid",
  "workspace": "/absolute/workspace/path",
  "session_id": "optional-session-id",
  "last_sequence": 381,
  "last_revision": 97
}
```

`client_id`, `workspace`, `session_id`, `last_sequence`, and `last_revision` are optional. A client that wants reconnect/resume must persist and resend the `client_id` returned by `welcome`. `client_id` is limited to 1-128 ASCII letters, digits, `.`, `_`, or `-`.

When `workspace` is omitted, the daemon uses its startup workspace as the default. When it is present, Yeet resolves it to a local directory (`~` and paths relative to the startup workspace are accepted) and creates or reuses a semantic runtime scoped to `(workspace, client_id)`. This lets independent authenticated clients use different projects through one Remote service. An invalid or missing directory is rejected with `workspace_invalid`. `session_id`, when present, is an affinity request within that selected workspace: Yeet restores that session before completing the handshake rather than silently attaching the browser to another session.

`last_sequence` is the highest semantic event sequence the client has fully applied. `last_revision` is the latest conversation revision the client has applied. Send both on reconnect.

### Server welcome

```json
{
  "type": "welcome",
  "version": 1,
  "client_id": "browser-tab-uuid",
  "workspace": "/absolute/workspace/path",
  "session_id": "current-session-id",
  "sequence": 381,
  "revision": 97,
  "resumed": true
}
```

`version` is the negotiated protocol version. Version 1 currently supports only the range `1..=1`; the range form in `hello` is deliberate so future clients and servers can overlap cleanly.

If `resumed` is `false`, a full `snapshot` immediately follows `welcome`. If `resumed` is `true`, the server replays every retained sequenced event after `last_sequence`; there is no redundant snapshot. A client must therefore handle either `welcome -> snapshot` or `welcome -> replayed events`.

## Sequencing, revisions, and reconnect

Semantic state events carry a monotonically increasing `sequence` within one retained `client_id` runtime. `revision` is Yeet's existing `BridgeState.conversation_revision`; it is not a second state model and is not incremented by the Remote gateway itself.

The server retains the most recent 1024 semantic events per client runtime. If the requested sequence is still covered, an exact retained/current sequence must also report the revision associated with that sequence. A sequence/revision mismatch, replay gap, future cursor, absent cursor, or new client identity falls back to a fresh full snapshot instead of risking a stale partial resume.

The semantic runtime is independent of the browser socket. Closing the WebSocket does not interrupt an active Yeet run. A streaming runtime is retained across disconnects; an inactive disconnected client runtime becomes eligible for eviction after 15 minutes. Reusing the same `client_id` and `session_id` is therefore the normal reload/Wi-Fi-loss/phone-lock recovery path.

Use a distinct `client_id` for independent tabs. Reusing a client identity intentionally means sharing its semantic frontend connection and selected-session affinity.

## Server state messages

### Full snapshot

```json
{
  "type": "snapshot",
  "version": 1,
  "sequence": 0,
  "revision": 12,
  "state": { "...BridgeState": "..." }
}
```

`state` is a complete serialized `BridgeState`, including `conversation`. It is the authoritative replacement state after initial connect or resynchronization.

The current top-level `BridgeState` fields are:

```text
debate
conversation_revision
conversation
active_assistant_entry_id
active_assistant_text
active_activity_entry_id
active_reasoning_entry_id
active_reasoning_text
active_reasoning_summary
is_streaming
error_message
active_model
active_reasoning_level
active_model_context_length
current_context_tokens
token_usage
credit_usage
pending_shell_permission
pending_native_app_permission
available_models
is_loading_models
saved_sessions
current_session_id
active_run_id
available_capabilities
is_loading_capabilities
auth_providers
auth_notice
auth_working
provider_configurations
providers_notice
providers_working
openai_flex
foundation_memory_enabled
foundation_memory_server
foundation_memory_connected
settings_notice
settings_working
sandbox_settings
sandbox_notice
sandbox_working
```

The Rust definitions in `src/model.rs` remain authoritative for nested state types.

### Compact state update

```json
{
  "type": "state_update",
  "version": 1,
  "sequence": 382,
  "revision": 98,
  "patch": {
    "is_streaming": true,
    "active_run_id": "run-id",
    "current_context_tokens": 12450
  }
}
```

`patch` is a shallow top-level `BridgeState` patch. Apply only the provided keys. Conversation data, `conversation_revision`, `active_assistant_text`, `active_reasoning_text`, and `active_reasoning_summary` are deliberately excluded because they have dedicated semantic events. Optional values may be JSON `null`.

### Assistant stream

```json
{
  "type": "assistant_delta",
  "version": 1,
  "sequence": 383,
  "revision": 99,
  "entry_id": "conversation-entry-id",
  "delta": "next chunk",
  "content": "",
  "reset": false
}
```

For the common append case, append `delta`; `content` is deliberately the empty string so long streams remain O(n) on the wire rather than resending the accumulated answer on every chunk. When `reset` is `true`, `delta` is empty and the full replacement text is in `content`; replace the live content for `entry_id` with it. A reset covers a non-prefix correction or a new active entry. When the backend seals a stream, the completed `conversation_entry` is authoritative and the Remote gateway does not emit a meaningless empty reset.

### Reasoning stream

```json
{
  "type": "reasoning_delta",
  "version": 1,
  "sequence": 384,
  "revision": 100,
  "entry_id": "reasoning-entry-id",
  "delta": "next chunk",
  "content": "",
  "summary": false,
  "reset": false
}
```

`summary=false` targets live reasoning text. `summary=true` targets the live reasoning summary. `delta`/`content`/`reset` have the same compact semantics as assistant streaming.

### Conversation entry

```json
{
  "type": "conversation_entry",
  "version": 1,
  "sequence": 385,
  "revision": 101,
  "entry": {
    "id": "entry-id",
    "kind": { "type": "assistant", "content": "done", "toolCalls": [] }
  }
}
```

If `entry.id` is new, append it. If an entry with that id already exists, replace it in place. This supports sealing streamed assistant/reasoning content without replacing the whole transcript.

Current `ConversationKind.type` values are `user`, `assistant`, `reasoning`, `activity`, `toolCall`, `skill`, `mcp`, and `system`.

### Tool update

```json
{
  "type": "tool_update",
  "version": 1,
  "sequence": 386,
  "revision": 102,
  "entry": {
    "id": "entry-id",
    "kind": {
      "type": "toolCall",
      "toolCall": {
        "id": "tool-id",
        "index": 0,
        "callID": "provider-call-id",
        "name": "read_file",
        "arguments": "{\"path\":\"src/lib.rs\"}",
        "status": "streaming"
      }
    }
  },
  "tool_call": {
    "id": "tool-id",
    "index": 0,
    "callID": "provider-call-id",
    "name": "read_file",
    "arguments": "{\"path\":\"src/lib.rs\"}",
    "status": "streaming"
  }
}
```

`entry` is the canonical transcript row; `tool_call` is duplicated as a convenience for tool-card stores. Tool statuses are `streaming`, `completed`, or `failed`.

### Activity update

```json
{
  "type": "activity_update",
  "version": 1,
  "sequence": 387,
  "revision": 103,
  "entry": {
    "id": "activity-entry-id",
    "kind": {
      "type": "activity",
      "activity": {
        "phase": "running",
        "title": "Reading",
        "detail": "src/lib.rs",
        "run_id": "run-id"
      }
    }
  },
  "activity": {
    "phase": "running",
    "title": "Reading",
    "detail": "src/lib.rs",
    "run_id": "run-id"
  }
}
```

Treat `entry` as the canonical conversation representation and `activity` as a convenience view.

### Conversation reset

```json
{
  "type": "conversation_reset",
  "version": 1,
  "sequence": 388,
  "revision": 104,
  "conversation": []
}
```

This is uncommon. Replace the conversation array when its identity/order can no longer be represented as entry additions/updates. A session change normally produces a full `snapshot` instead.

## Client commands

Commands wrap the existing `FrontendCommand` directly. This is the key compatibility rule: Remote does not invent a second command vocabulary.

```json
{
  "type": "command",
  "version": 1,
  "request_id": "optional-client-request-id",
  "command": { "type": "submit", "text": "hello" }
}
```

The server answers a successfully queued command with:

```json
{
  "type": "ack",
  "version": 1,
  "request_id": "optional-client-request-id"
}
```

`ack` means the command was accepted into the existing Yeet frontend/backend channel. It does not mean an asynchronous operation has completed. Observe subsequent semantic state events for completion.

Supported `FrontendCommand` payloads are:

```jsonc
{ "type": "submit", "text": "..." }
{ "type": "interrupt" }
{ "type": "allow_shell" }
{ "type": "deny_shell" }
{ "type": "allow_native_app" }
{ "type": "deny_native_app" }

{ "type": "request_models" }
{ "type": "select_model", "model": "..." }
{ "type": "select_reasoning", "level": "auto|low|medium|high" }

{ "type": "request_sessions" }
{ "type": "load_session", "session_id": "..." }
{ "type": "new_session" }

{ "type": "request_capabilities" }
{ "type": "toggle_capability", "id": "..." }

{ "type": "request_auth" }
{ "type": "auth_login", "provider": "..." }
{ "type": "auth_logout", "provider": "..." }
{ "type": "auth_set_api_key", "provider": "...", "key": "..." }

{ "type": "request_providers" }
{ "type": "save_provider", "id": "...", "base_url": "...", "require_api_key": true }
{ "type": "remove_provider", "id": "..." }

{ "type": "request_settings" }
{ "type": "set_open_ai_flex", "enabled": true }
{ "type": "set_foundation_memory", "enabled": true }

{ "type": "request_sandbox" }
{ "type": "update_sandbox", "action": { "type": "apply_preset", "preset": "..." } }

{ "type": "start_debate", "topic": "...", "models": null }
```

`shutdown` exists internally in `FrontendCommand` for local lifecycle management but is intentionally forbidden over Remote.

`update_sandbox.action` uses the existing `SandboxAction` variants:

```text
apply_preset { preset }
set_execution_mode { mode }
set_auto_approve { enabled }
set_workspace_mode { mode }
add_workspace_path { path }
remove_workspace_path { path }
set_scratch_writable { enabled }
add_network { host, port? }
remove_network { host, port? }
set_environment { key, value }
remove_environment { key }
add_secret { id }
remove_secret { id }
set_limit { name, value }
reset
```

Permission allow/deny commands intentionally operate on the currently pending permission already represented by `BridgeState.pending_shell_permission` or `pending_native_app_permission`; the browser does not synthesize permission IDs or fake key presses.

## Errors and liveness

Protocol errors use:

```json
{
  "type": "error",
  "version": 1,
  "code": "protocol_mismatch",
  "message": "human-readable detail",
  "fatal": true,
  "request_id": null,
  "supported_min_version": 1,
  "supported_max_version": 1
}
```

Current error codes include `hello_required`, `malformed_message`, `protocol_mismatch`, `workspace_mismatch`, `invalid_client_id`, `backend_unavailable`, `session_unavailable`, `already_initialized`, `command_forbidden`, `backend_command_failed`, and `backend_error`.

When `fatal=true`, reconnect with corrected handshake/auth/version data rather than continuing to use the socket. Malformed JSON after a successful handshake and forbidden commands are nonfatal; malformed/missing initial `hello` is fatal.

Application-level liveness is available with:

```json
{ "type": "ping", "version": 1, "nonce": "optional" }
```

and:

```json
{ "type": "pong", "version": 1, "nonce": "optional" }
```

Normal WebSocket Ping/Pong control frames are also supported.

## WebUI implementation rules

The browser should maintain one local semantic store seeded by `snapshot`, then update it from `state_update`, streaming events, and conversation/tool/activity events. That store is a client representation of `BridgeState`, not a separately invented Yeet application-state model.

Do not poll `/api/frame`. Do not infer state from HTML. Do not send `/api/input` key/resize/wheel messages. Do not depend on terminal rows/columns. Do not instantiate or reproduce Ratatui behavior in the browser.

On reconnect, reuse `client_id`, send the currently selected `session_id`, and send the last fully applied `sequence` and `revision` only when the browser still has the semantic state corresponding to that cursor (for example, a live SPA reconnect or a deliberately persisted store). A hard page reload that did not persist its semantic store should omit `last_sequence`/`last_revision`, forcing a full snapshot. Only advance a retained cursor after an event has been successfully applied. If the server responds with `resumed=false`, discard speculative local semantic state and replace it with the following snapshot.

For commands that can be clicked repeatedly, use a unique `request_id` for UI correlation, but do not treat `ack` as operation completion. In particular, a submitted prompt is represented as successful only by subsequent Yeet state/run events; the transport does not create a second request/run lifecycle.
