# Shared actions and runtime agents

- `agents/actions`: agent-owned intents and existing backend commands (wire format unchanged).
  This module is independent of Skyline; `actions` is only a compatibility re-export.
- `agents/state`: serializable runtime identity, public decisions and relationships.
- `agents/registry`: synchronized process-wide state and run lifecycle; no provider/UI dependencies.
- `agent`: execution orchestrator; registers lazily, updates model/workspace each run,
  records public goal verdicts/outcomes and unregisters on teardown.
- `tui/app/actions`: adapter interpreting navigation locally and sending commands
  through the existing Harness pipeline. `WorkbenchTab` and `model::FrontendCommand`
  are compatibility re-exports, not duplicate state or command definitions.

Canonical action imports come from `yeet::agents::actions::{Action, FrontendCommand, NavigationAction}`.
Skyline remains a separate deployment/service capability, not the owner of agent actions.

Hosts can use `App::dispatch_action(Action::Command(FrontendCommand::Submit { ... }), backend)`
or `Action::Navigate(NavigationAction::Session)` without knowing the input device.
Existing keyboard/mouse tab activation uses the same navigation interpreter.
Local input editing and widget-specific actions remain in the terminal layer.

Runtime identity lives across runs on one coordinator, not across process restarts.
`agents::global().snapshots()` returns owned snapshots; no caller holds registry locks
while invoking tools. `RunGuard` restores idle on every exit path. A spawned agent can
call `AgentCoordinator::register_runtime_agent(name, model, Some(spawned_by))` before
running, then `AgentRegistry::connect` to declare coworkers. `spawned_by` records
execution lineage only and grants no authority; it is validated, and unregistering
removes stale relationships. Decisions are explicit
public summaries, never private model reasoning.

The global registry is process-local. Detached Skyline children currently travel
through the background-session protocol; cross-process lineage and registry snapshots
are not automatically propagated by that protocol. This module does not replace
session persistence, authorization, or transport routing.

Adaptive in-process workers automatically register their role as their name, record
the primary coordinator as `spawned_by`, and connect to it as coworkers before running.
