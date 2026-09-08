# Yeet runtime bridge protocol

The Rust host talks to one persistent TypeScript sidecar over newline-delimited JSON on stdin/stdout. stderr is diagnostics only. Provider HTTP attempts are emitted there as `api-call {json}` records with redacted endpoints; request bodies and credentials are never included.

Bridge protocol version: `1`.
Package version: `0.1.0`.

## Model calls

`complete` and `stream` requests contain a single `model` field in `provider/model` form. There is no separate routed provider field. The bridge parses the first slash, refreshes that provider's credential, and passes the remaining model name to the provider adapter.

## Commands

Core/auth:

- ping
- list-providers
- list-models
- list-model-info
- config-path
- auth-status
- auth-set-api-key
- auth-login-browser
- auth-logout
- register-provider
- unregister-provider
- complete
- stream
- cancel
- shutdown

Skills:

- skill-list
- skill-load
- skill-read

MCP:

- mcp-list-servers
- mcp-set-server
- mcp-remove-server
- mcp-list-tools
- mcp-call-tool
- mcp-list-resources
- mcp-read-resource
- mcp-list-prompts
- mcp-get-prompt
- mcp-disconnect

Every command contains v, id, and op. Streams emit event frames followed by done. Errors are terminal for the request id.

## Native-app approval

An MCP stdio server can request Computer Use access with an `elicitation/create`
request. Qualifying requests are forwarded as an unsolicited sidecar frame
without an ordinary response id:

```json
{"v":1,"type":"native_app_approval_request","requestId":"9001","server":"node_repl","tool":"js","bundleId":"org.blenderfoundation.blender","appName":"Blender","operation":"Open Blender","message":"Open Blender for this MCP operation"}
```

The Rust host must answer with a session-only decision frame. No persistent or
global approval marker is accepted or emitted:

```json
{"v":1,"type":"native_app_approval_decision","requestId":"9001","decision":"accept","scope":"session"}
```

`decision` is `accept` or `decline`; cancellation, shutdown, connection close,
and unsupported elicitation requests resolve to `decline`.

## Tool-call lifecycle data

The shared TypeScript types expose additive `ToolCallFeedback` and
`ToolCallResult` payloads. Hosts that execute tools can attach a result to a
`Message` with `toolResult` (and optional `toolFeedback`) or emit lifecycle
frames as `tool-call-feedback` and `tool-call-result` stream events. Structured
results are encoded as JSON when sent to providers; existing text-only tool
messages continue to use their original wire shape.

`list-models` takes a registered provider id and returns provider-local model identifiers in a `models` frame. The bridge refreshes the provider credentials before making the provider's model-list request.

`list-model-info` uses the same provider refresh and returns `ModelInfo` entries
in a `model-info` frame. Each entry includes `contextLength` when the remote
model catalog provides a context-window value.

## Storage

```text
~/.yeet/
├── config.json
├── credentials.json
├── mcp.json
└── skills/
```

`YEET_CONFIG_DIR` overrides the root.

## Cancellation

Host cancellation removes the local stream registration and sends cancel. The bridge aborts the model request's AbortController, including Fetch body/SSE consumption.

MCP stdio processes are held by the sidecar and closed by mcp-disconnect or shutdown.


### Embeddings

`embedding-models` returns `{ type: "embedding-models", models: string[] }` with
initial-selection candidates from configured providers. It never selects a new
model for an existing index.

`embed` accepts `model` (provider-qualified) and `input` (1–64 nonempty strings).
It returns `{ type: "embedding-result", result: { model, resolvedModel, source,
vectors } }`. `source` fingerprints the provider protocol and endpoint without
including credentials. `cancel` applies to embedding calls. The same CallCore
provider adapters and credentials are used as for generation. Native Rust memory
owns index binding, validation, locking and atomic full re-indexing; embeddings
never pass through the conversation compactor.
