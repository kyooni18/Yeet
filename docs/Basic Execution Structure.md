# Basic Execution Structure

Yeet keeps the interactive surface small and moves execution behind explicit
boundaries:

```text
Session (presentation + slash commands)
  -> AgentCoordinator (one history + finite direct-tool loop)
      -> DirectToolRegistry (metadata + lazy Skill/MCP activation)
          -> LiveHarnessRuntime (one bridge + one edit daemon)
```

`ToolCallAssembly` is the shared protocol boundary for the lead stream. It
reconstructs calls from delta-only providers without coupling orchestration to
provider-specific event details.

The lead always sees the base tools `find_capabilities`,
`activate_capability`, `read_file`, and `apply_file_edits`. Activation adds only
the selected capability's tools:

- `workspace`: snapshot-safe `read_file` and `apply_file_edits`.
- `skill:<name>`: Skill instructions and supporting files, loaded on demand.
- `mcp:<server>`: only the selected server's tool schemas and calls.

Skill instructions and MCP schemas are loaded only after explicit activation;
activation is published atomically after loading succeeds. One model history,
one bounded tool loop, and one runtime are the only execution state.

The runtime is lazy and persistent for the lifetime of an interactive session.
It refreshes capability metadata at turn boundaries, reuses the same bridge and
edit daemon, and can discard a dead bridge for clean recovery on the next turn.
