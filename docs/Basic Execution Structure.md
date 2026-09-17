# Basic Execution Structure

```text
Ratatui TUI / Rust CLI
  -> Rust session controller
      -> AgentCoordinator (one history + isolated execution lanes)
          -> Coding lane (existing coding prompt/tool/recovery behavior)
          -> Research lane (explicit live-web queries)
          -> General lane (documents, data, typed artifacts, lazy capabilities)
          -> ToolRegistry
              -> workspace edit daemon client
              -> sandboxed shell executor
              -> document readers + structured data analysis
              -> typed artifact store
              -> lazy Skill / MCP / Worker activation
          -> BridgeClient
              -> persistent TypeScript provider/auth/MCP sidecar
```

The interactive process is a single Rust executable. It no longer launches a
second Yeet backend process. Provider and edit JavaScript sidecars are protocol
services rather than application-state owners.


## Peer agent invariant

Yeet agents are peers. Agent identity grants no permanent authority over another
agent, and no model is a built-in lead, manager, supervisor, or root decision-maker.
Coordination authority belongs to deterministic runtime state: task claims, leases,
dependencies, validation records, and conflict checks.

Roles such as researcher, implementer, reviewer, verifier, or synthesizer are
scoped to one task and may move between agents. Owning a task means owning its
bounded work scope, not owning other agents. Final synthesis is likewise a task
stage, not a privileged agent class.

When peers disagree, resolution should come from stronger evidence, successful
validation, explicit user direction, or an isolated review task. File ownership and
exclusive test resources use revocable leases so stale agents cannot block useful
work indefinitely.

`AgentCoordinator` reconstructs delta-only tool calls, keeps tool call/result
pairs in one provider-neutral history, retracts provisional assistant prose when
a response turns into a tool round, detects semantically repeated inspection,
and forces a decision after repeated no-progress rounds. Coding requests keep
the established coding lane unchanged; general requests do not inherit its
source-edit/shell tool surface.

`ToolRegistry` starts with workspace discovery/read/search/edit, shell,
document/data tools, typed artifacts, and lazy capability discovery. Tool
selection hides document/data built-ins from coding turns and hides coding-only
workspace mutation/shell built-ins from general turns. Skills, MCP servers, and
Workers add schemas only after explicit activation.

Workspace reads use the transactional edit daemon and return a snapshot plus
`line:hash|text` anchors. Read/search/list coverage is cached for the task so
the agent can reuse established source instead of growing context with duplicates.

`run_shell` executes under a macOS Seatbelt profile. Read-oriented commands can
run directly with bounded output. Noisy build/test commands can be evaluated by
a separate tool-less provider call. Commands classified as mutating require a
one-time exact-command permit from the user.

Session state and model history are persisted under `~/.yeet/sessions`.
Project-wide Yeet preferences, including capability toggles, live at
`./.yeet/settings.json`; sandbox requests remain separate at
`./.yeet/sandbox.json`.
