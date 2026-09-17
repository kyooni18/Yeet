# Yeet extensions

Yeet extensions are out-of-process add-ons discovered from either the user configuration directory or the current project. They are deliberately separate from Yeet skills and MCP servers: an extension is a companion process that observes Yeet lifecycle/state events and can provide UI or other sidecar behavior without being placed in the model's tool surface.

## Locations

Global extensions live under the Yeet config directory at `extensions/<id>/extension.json`. Project extensions live under `<workspace>/.yeet/extensions/<id>/extension.json`. If both scopes contain the same extension ID, the project extension wins.

Use `yeet extension path` to print the global extension directory and `yeet extension list` to show the effective extension set for the current workspace.

## Manifest

The v1 manifest is JSON:

```json
{
  "schemaVersion": 1,
  "id": "example",
  "name": "Example extension",
  "version": "1.0.0",
  "description": "Example sidecar",
  "autoStart": true,
  "platforms": ["macos"],
  "events": ["state"],
  "commands": [
    { "name": "example", "description": "Open the example extension" }
  ],
  "entry": {
    "command": "./bin/example",
    "args": [],
    "env": {}
  }
}
```

`entry.command` is resolved relative to the extension directory when it contains a path. Bare executable names use `PATH`. `${HOME}` and `${workspace}` are expanded in command, args, and environment values.

## Protocol

Auto-start extensions are owned by the workspace background daemon, not individual session runtimes. This keeps one extension process attached while Yeet creates, switches, or retires multiple live sessions. The daemon launches each extension with stdin piped and stdout detached. Yeet sets `YEET_EXTENSION_PROTOCOL=yeet.extension.v1`, `YEET_EXTENSION_ID`, `YEET_EXTENSION_DIR`, and `YEET_WORKSPACE`.

Events are newline-delimited JSON objects on stdin. The first record is `hello`; subsequent subscribed events use the same envelope:

```json
{
  "protocol": "yeet.extension.v1",
  "sequence": 2,
  "timestamp": "2026-09-14T00:00:00Z",
  "event": "state",
  "workspace": "/path/to/project",
  "payload": {
    "isStreaming": true,
    "model": "provider/model",
    "reasoningLevel": "medium",
    "sessionId": "...",
    "runId": "...",
    "pendingPermission": null,
    "errorMessage": null,
    "activity": {
      "phase": "tool",
      "title": "Using computer",
      "detail": "computer_use"
    },
    "activeTool": "computer_use"
  }
}
```

State events are deduplicated on the stable extension projection, so token-level model deltas do not flood sidecar processes.

Extensions can declare slash commands in `commands`. Command names omit the leading slash in the manifest. Yeet advertises them to frontends for autocomplete; when invoked, the owning extension receives a `command` event on stdin:

```json
{
  "protocol": "yeet.extension.v1",
  "event": "command",
  "payload": {
    "name": "example",
    "args": ["menu"]
  }
}
```

Built-in Yeet commands take precedence over extension commands with the same name.

## CLI

```sh
yeet extension list
yeet extension path
yeet extension validate /path/to/extension
yeet extension install /path/to/extension
yeet extension install /path/to/extension --project
yeet extension install /path/to/extension --force
yeet extension remove extension-id
yeet extension remove extension-id --project
```

The v1 host is intentionally observation-first. It provides process lifecycle and state events, while model tools remain owned by Yeet's existing tool/MCP/capability layers.
