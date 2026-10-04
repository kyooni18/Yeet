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

## Agent Group ownership

The Main Agent owns the user conversation. A Group Agent is the top-level
delegated execution unit for one objective, and its member agents are
subordinate participants:

```text
Main Agent
  └── Group Agent (objective, lifecycle, shared state, budget, final result)
        ├── Member Agent (bounded role-specific task)
        └── Member Agent (bounded role-specific task)
```

The Group Agent owns task decomposition, the member registry, coordination,
shared findings and artifacts, progress aggregation, cancellation, and final
synthesis. Members receive only the objective, their assignment, relevant
shared findings, and the permissions needed for their role. Important member
results are promoted into group state before the coordinator or Main Agent
consumes them. Member completions do not start unrelated Main Agent runs.

Member execution remains concurrent and independently attributable. Every
member event is associated with a stable group, member, and task identity;
group progress is derived from those events. The group-level budget covers
coordination, member work, and final synthesis. Input context capacity,
per-request output limits, and observed token/cost usage are tracked as
separate quantities. Allocations can be rebalanced as work changes, while a
reserved share remains available for coordination and synthesis.

The Rust harness exposes `CreateAgentGroup`, `StartAgentGroup`,
`ResumeAgentGroup`, `CancelAgentGroup`, `StopAgentGroup`, and
`InspectAgentGroup` commands. The Main Agent has the corresponding lifecycle
tools. `AgentGroupSupervisor` owns one active runtime and shares its coordinator
slot with both entry points. Member delegation is available only on that
Group Agent coordinator's tool registry. The older `SpawnAgent` frontend
command now creates and starts a group objective; it no longer launches a
standalone member. Skyline no longer opens detached child Yeet sessions for
delegation.

`BudgetLedger` uses the group's output and estimated-cost ceilings across that
group's lifecycle. Ten percent of each is initially reserved for coordination
and another ten percent for synthesis; unused task capacity rolls into final
integration after member work settles. Active task allocations are weighted by
role and requested effort, rebalanced on new assignments, usage reports, and
completion. Context-window capacity is model metadata for input fit; it does
not consume output-token allocation. Each provider output cap is refreshed from
the task's remaining allocation. Near the cap, members receive a checkpoint
instruction and retain a small completion allowance so findings can be
returned instead of abruptly cancelling the member. Provider usage reports
are deduplicated before group, task, and cost totals are updated. Missing cost
telemetry remains unknown and stops further cost-based delegation.

Group changes publish the `AgentGroupItem` in `HarnessState`. Each event has a
monotonic group sequence plus `group_id`, optional `member_id`, and optional
`task_id`; member phase (`reasoning`, `tool_call`, `waiting_for_input`,
`completed`, and failure/cancellation states) is separate from member lifecycle
status. The backend appends these events to the session event log and saves a
versioned `group-checkpoint.json` sidecar under the session lock. Loading a
session restores its group identity, findings, usage, budget ledger, bounded
event history, and task summaries. Work that had a live thread at shutdown is
marked interrupted and the group resumes from saved findings/checkpoints with
a fresh coordinator; in-process provider/member histories are not persisted.
Remote reconnect in a live process uses the current `HarnessState` projection,
while session reload uses the durable checkpoint and event journal.

Workspace-write admission remains deterministic: at most one busy
implementer may mutate the shared workspace at a time unless the group uses
isolated workspaces. A member owns only its bounded task. When members
disagree, the group coordinator resolves the result using evidence, validation,
and explicit user direction.

## Storage

Session state lives under `~/.yeet/sessions` (`SessionStore`): semantic
components are content-addressed objects and a manifest replaced last is the
only commit point; per-session and store-wide file locks serialize writers;
the event log is append-only evidence; workspaces are identified by a stable
id in `<root>/.yeet/workspace.json`. Project preferences live in
`./.yeet/settings.json`; sandbox policy in `./.yeet/sandbox.json`.
