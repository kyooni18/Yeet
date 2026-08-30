# Swift bridge protocol

The Swift compatibility layer talks to one persistent TypeScript sidecar over newline-delimited JSON on stdin/stdout. stderr is diagnostics only.

Bridge protocol version: `1`.
Package version: `0.1.0`.

## Model calls

`complete` and `stream` requests contain a single `model` field in `provider/model` form. There is no separate routed provider field. The bridge parses the first slash, refreshes that provider's credential, and passes the remaining model name to the provider adapter.

## Commands

Core/auth:

- ping
- list-providers
- list-models
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

`list-models` takes a registered provider id and returns provider-local model identifiers in a `models` frame. The bridge refreshes the provider credentials before making the provider's model-list request.

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

Swift cancellation removes its local continuation immediately and sends cancel. The bridge aborts the model request's AbortController, including Fetch body/SSE consumption.

MCP stdio processes are held by the sidecar and closed by mcp-disconnect or shutdown.
