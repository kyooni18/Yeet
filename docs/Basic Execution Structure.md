# Basic Execution Structure

## Runtime topology

Interactive frontends do not own session state. They attach to one shared
background runtime per workspace:

```text
Frontend / host (Ratatui TUI, Remote web client, desktop host, CLI)
  -> Harness                         src/harness.rs (Shared or Embedded mode)
      -> BackgroundConnection        src/background/connection.rs
          -> workspace daemon        src/background.rs (one per workspace + scope)
              -> RuntimeProcess      src/background/runtime_process.rs
                                     (one dedicated thread per session runtime)
                  -> BackendService  src/backend.rs (command routing)
                      -> AgentCoordinator  src/agent.rs (turn loop)
                          -> ToolRegistry  src/tools.rs
                          -> ProviderBridge (BridgeClient, src/core/provider_bridge.rs)
                              -> Node sidecar RuntimeSource/dist/bridge.js
```

`Harness::embedded` skips the daemon and owns a `BackendService` in-process for
native hosts, the one-shot `yeet agent` command and tests.

Two transports are deliberately named differently:

- **Harness state transport**: `HarnessCommand` in, `HarnessEvent` /
  `HarnessState` out (historically `FrontendCommand`, `BridgeEnvelope`,
  `BridgeState`; compatibility names refer to the same types and keep the
  same serialized representation). Frontends, Remote and the daemon exchange
  this transport.
- **ProviderBridge**: the Rust <-> Node line protocol (`BRIDGE_PROTOCOL_VERSION`)
  to the provider/auth/Skill/MCP sidecar. It is a protocol service, never an
  application-state owner.

## Background daemon contract

- **Single owner.** A workspace daemon holds an owner lease; startup and stale
  recovery are serialized by a lifecycle lock, so two terminals can never
  produce two live daemons for one workspace.
- **Compatibility.** The first frame on every socket is a `heartbeat` carrying
  a handshake (`src/background/protocol.rs`): transport `protocolVersion`,
  semantic `stateSchemaVersion` and additive `features`. Clients require the
  same protocol version and a state schema they support. A heartbeat without a
  handshake is a legacy daemon and is accepted, so a daemon started by an
  older binary keeps its sessions across a local install. A reachable but
  incompatible daemon is reported and left running; clients never retire it.
- **One live runtime per session.** Clients loading the same session join one
  runtime (pending isolations coalesce concurrent startups). Each runtime runs
  on its own thread so one busy or wedged session cannot stall the daemon,
  heartbeats or other sessions.
- **Reconnect.** A client that loses its socket re-establishes it and replays
  `LoadSession` for its session, then ignores frames until that session's
  state arrives. Losing every client is not an interrupt: runs continue.
- **Lifecycle ownership.** `background/session_runtime.rs` owns idle and
  interrupt deadlines. Runtime teardown runs on a separate thread so backend
  work cannot block the daemon loop. Connection lifetime and reconnect policy
  live separately in `background/connection.rs`.
- **Recovery.** An interrupted run whose worker does not settle is abandoned
  (`BackendService::abandon_stuck_run`) and the runtime replaced; idle
  runtimes retire after a minute and an idle daemon exits after ten minutes.

## Backend service and runs

`BackendService` routes `FrontendCommand`s to focused modules and owns the
wiring between them (see `docs/CODEBASE.md` for the module map). A service
executes **at most one top-level run at a time**: its single coordinator is
held for the run, and live state carries one `current_turn`, one streaming
assistant/reasoning buffer and one pending tool-call projection. Admission is
gated on `is_streaming`; every projection is fenced by
`current_turn == run.id`, so a replaced or abandoned run cannot write into its
successor. The run lifecycle (`backend/run.rs`: admit, drive, settle, release)
is independent of storage: persistence is prepared under the live-state lock
and committed after it is released (`backend/persistence.rs`).

## Agent coordinator

`AgentCoordinator` keeps one provider-neutral history and isolated execution
lanes (coding, research, general). It reconstructs delta-only tool calls,
keeps tool call/result pairs together, retracts provisional assistant prose
when a response turns into a tool round, detects semantically repeated
inspection, and forces a decision after repeated no-progress rounds.

Prompt-cache continuity is a protocol invariant: within one context window,
provider-visible history is append-only. A request that would rewrite an
already-submitted prefix is rejected before dispatch; rollover, rewind
(regenerate/edit-last) and history replacement open a new window instead.
`src/agent/continuity_tests.rs` checks this end to end against a scripted
bridge.

## Tools

`ToolRegistry` routes calls; `ToolCatalog` owns schemas, activation and
capability toggles; `ToolExecutionContext` owns workspace/session identity,
context roots, protected session-state paths and approvals. Tool selection
hides document/data built-ins from coding turns and coding-only mutation/shell
built-ins from general turns. Skills, MCP servers and Workers add schemas only
after activation; only MCP tools annotated read-only may run as a parallel
batch or inside research.

`ToolEvidence` owns observation state with three lifetimes. Visibility reset
(context rollover or a direct MCP call boundary) forgets duplicate coverage,
while keeping task edit snapshots/read coverage and web source grants. Task
completion clears that provenance and mutation/auxiliary usage but preserves
workspace generations. Workspace mutations invalidate affected source evidence
(or all source evidence when scope is unknown) and advance its generation;
web provenance remains valid. Shell grants, artifacts and workers keep their
separate owners. `ToolServiceConfiguration` groups Foundation/web routing
choices independently of task evidence; web route changes clear only web
coverage and grants, and Foundation route changes deactivate prior aliases.

Workspace reads go through the transactional edit daemon and return a
snapshot plus `line:hash|text` anchors; writes require the snapshot from a
preceding read. `run_shell` executes under the platform sandbox; commands
classified as mutating need a one-time exact-command permit. Active session
state is write-protected even in unlimited mode.

## Agent groups and the peer invariant

Yeet agents are peers. Agent identity grants no permanent authority over
another agent, and no model is a built-in lead, manager, supervisor, or root
decision-maker. Coordination authority belongs to deterministic runtime
state: task claims, budgets, dependencies, validation records, and conflict
checks. Execution lineage is recorded as `spawned_by` and grants nothing.

Roles such as researcher, implementer, reviewer, verifier, or synthesizer are
scoped to one task and may move between agents. Owning a task means owning its
bounded work scope, not owning other agents. Final synthesis is a task stage,
not a privileged agent class. When peers disagree, resolution comes from
stronger evidence, successful validation, explicit user direction, or an
isolated review task.

Each delegated member has its own coordinator and history. The group runtime
enforces the group budget, admits at most one busy implementer at a time
(single-writer workspace mutation), and stops or retires members only on
explicit request, group replacement or shutdown. Background members persist
across primary turns and report through notifications; interrupting the
primary cancels only foreground members.

## Storage

Session state lives under `~/.yeet/sessions` (`SessionStore`): semantic
components are content-addressed objects and a manifest replaced last is the
only commit point; per-session and store-wide file locks serialize writers;
the event log is append-only evidence; workspaces are identified by a stable
id in `<root>/.yeet/workspace.json`. Project preferences live in
`./.yeet/settings.json`; sandbox policy in `./.yeet/sandbox.json`.
