# Shared application UI migration

The target dependency direction is `Platforms -> UI -> Harness`.

- `Harness/` owns agent execution, runtime commands/events, sessions, models,
  tools, persistence and filesystem access. Runtime business logic remains Rust.
- `UI/` owns application views, navigation, menus, actions, interaction state,
  layout semantics and semantic icon identities. Its representation must not
  contain Ratatui, Crossterm, SwiftUI, AppKit, DOM or React Native types.
- `Platforms/` will own visual translation, input-device translation, geometry,
  focus, clipboard, lifecycle, authentication capabilities and host transports.
  Adapters consume shared application structure rather than define their own.

This is an incremental migration, not a completed separation. A directory move
alone does not establish the dependency boundary.

## Current checkpoint

`Harness/mod.rs` owns the public Shared/Embedded attachment API and the renamed
service implementation. `src/backend.rs` remains a compatibility facade for
existing callers. Much non-UI Rust code still lives in `src/`; subsequent moves
must follow real ownership and preserve relative asset paths and module APIs.

`UI/actions.rs` owns the existing navigation intents and UI/runtime action
envelope. Runtime commands remain Harness-owned. Existing action paths and JSON
forms stay compatible. Index-based file/diff navigation is a legacy wire form;
retained interactions use stable view identities.

`UI/surfaces.rs` owns stable view identities, tab lifecycle and selection.
`UI/workbench.rs` declares workbench controls, labels, launcher items, semantic
icons and close actions. Terminal facades preserve existing callers while the
TUI begins consuming those shared declarations.

The terminal application now holds shared navigation state and consumes
`ApplicationView` for primary content, auxiliary surfaces and launcher overlays.
Its Session layout and painting follow the canonical workspace child order used
by the graphical shell. Component-specific state, menus, actions and conversation
presentation still need extraction; migrating those remains required.

The working browser client remains in `web/`. `RemoteShell.tsx` renders ordered
semantic views supplied by `UI/shell.rs`, reports viewport capabilities and sends
shared shell actions. Rust owns panel visibility, dismissal priority and compact
navigation interactions. Remote v1 adds per-client `ui_action`/`ui_state` messages;
UI revisions do not advance Harness sequence/replay cursors. Tauri exposes the
same shell state/actions/projection through its local adapter. Browser focus
handles, DOM accessibility and native bridge lifecycle stay in adapters.

`frontend/shared` shares Remote protocol, transport and streaming reconciliation.
Sharing runtime DTOs alone does not share application UI. Expo/React Native is
currently blank; Tauri hosts its web export and attaches an embedded Harness.
`Sources/` contains empty Swift directories, not another active GUI frontend.

`app/ui/ContentView.ui` reserves the user's OpenPencil design. Do not invent a
new design during structural migration. Preserve existing application behavior
while extracting its semantics. `.ui` documents remain design specifications;
this work does not introduce a parser or executable design-file runtime.

## Remaining sequence

1. Extract a shared application state and reducer from existing terminal and
   browser behavior. Include active view, overlays, sidebar/inspector visibility,
   draft ownership, action availability and navigation transitions. Leave input
   keys, mouse coordinates, focus handles and rendering caches in adapters.
2. Extend the shared declarative projection from workbench controls into actual
   application views and layout semantics. Use domain concepts such as
   conversation, composer, resource browser and permission prompt. Shared UI
   decides their composition and actions; each renderer supplies native visuals.
3. Make TUI navigation and view orchestration consume that state and projection.
   Replace terminal-owned application decisions incrementally, retaining its
   keyboard, scrolling, selection and geometry behavior. Extract filesystem and
   Git operations currently performed by UI helpers into Harness services.
4. Expose the same Rust UI state/actions/projection through additive Remote and
   Tauri adapter surfaces. Remote UI state belongs to each client, independently
   of shared runtime sessions. `RemoteHub` already maintains client/workspace
   runtimes and provides a useful integration boundary. Preserve existing v1
   streaming and reconnect behavior during rollout.
5. Migrate browser shell composition and interactions to the shared projection;
   retain DOM accessibility, focus restoration, viewport handling and browser
   authentication in its renderer. Connect native renderers to the same model
   when their user-directed design is implemented. Do not create a second
   TypeScript application reducer that independently determines app structure.
6. Move active adapters under `Platforms/`, resolve compatibility imports and
   remove obsolete facades after consumers migrate. Audit build scripts, asset
   embedding, install/release workflows and Tauri resources when paths move.
7. Verify the complete dependency direction and parity across actual consumers.
   Shared-state behavior tests, terminal rendering/interaction checks, browser
   integration checks and host builds must cover the migrated behavior. Keep
   tests focused on meaningful regressions, not redundant compile-time checks.

## Evidence and continuity

The shared action compatibility test covers every existing navigation JSON form
and a runtime command envelope. Stable tab lifecycle and declarative control
tests cover identities surviving another view's closure. These checks establish
the initial seams; they do not prove full UI migration or platform parity.

Foundation recall and remember calls returned MCP internal errors during this
survey. No durable memory write was confirmed. Retry Foundation when available;
use this document and coherent Git checkpoints to recover the migration state
until its service is restored. Jev favored renaming the runtime implementation
first while preserving compatibility and moving navigation ownership into UI.

Checkpoint verification: the Harness library/binary check, 18 Harness tests and
six existing source-layout checks passed. The three shared UI tests and three
TUI navigation tests passed. The broader terminal suite passed 100 of 102 tests.
Both failures also reproduce in an isolated snapshot with all migrated terminal
adapters reverted: `git_review_is_wired_to_launcher_files_tabs_and_mouse`
expects one diff instance while the existing worktree creates two;
`home_overview_layout_tracks_mockup_regions_at_desktop_preview_size` expects
older Home colors/layout. Those unrelated worktree changes are preserved.

The next checkpoint moves actual navigation state and transitions into
`UI/navigation.rs`: screen selection, launcher origin/cursor, tab order and
cycling, accepted activation and diff closure fallback. The TUI holds this shared
state; temporary field forwarding keeps legacy panel access working during
migration. Terminal key mappings, frame caches and hit geometry remain adapters.
`Harness/resources` now owns directory metadata, file inspection, Git snapshots,
repository discovery and review patches consumed by the TUI. Existing workbench
resource reexports remain compatible. Ten navigation tests and five workbench
checks passed, including tab identity, launcher behavior and Git index/worktree,
rename, binary and unborn repository handling.

The shared composition checkpoint introduces `UI/application.rs` and
`UI/shell.rs`. Terminal orchestration consumes the ApplicationView; Web shell
maps ordered Rust views and workspace children into native components. Tauri and
Remote adapters carry the same UI shell projections. Reconnect retains mounted
browser state and treats UI revision independently from Harness cursors. Browser
tests use generated Rust reducer fixtures (`examples/ui_shell_fixture.rs`), not
a second TypeScript implementation of application interaction policy.

Verification: 330 library tests passed, excluding only the two independently
attributed pre-existing TUI assertions documented above. The desktop UI
wire/isolation test and RN typecheck/lint passed. Web typecheck and 23 targeted
desktop/mobile shell, navigation, modal and protocol checks passed (five checks
skip inapplicable layouts). Startup layout and touch focus regressions found by
those tests were corrected before commit.

This checkpoint still does not establish the complete requested architecture.
Platform buttons/menus, drafts, settings forms and conversation grouping remain
partly independent; Home/Files/Diff graphical parity and native visual rendering
are not established. The shared terminal and browser state need convergence into
one application controller, and active renderer paths still need relocation
under `Platforms/`. Much runtime implementation is still in `src/`. Subsequent
work must remove those ownership gaps, not count shared projection types alone
as completion.

Five further reconnect/outage/client-identity and draft-preservation checks passed
across desktop/mobile (three inapplicable cases skipped). The next cross-platform
seam is Agent Group view state and controls: move semantic actions, eligibility,
member/feed projection and control labels/icons/order from the TUI and Web into
UI, then render the same controls in both. Also merge shell and navigation into
one application session so graphical clients can consume Home/Files/Diff as well
as Session. The blank native design is a remaining parity constraint; native
transport support alone does not prove native rendering completion.

The Agent Group checkpoint moves lifecycle/selection/draft actions, availability,
status precedence, attributed event filtering, findings, budgets and ordered
section layout into `UI/agents/`. TUI and Web render this projection. Native
editor buffers, focus traps, glyphs, scrolling and hit geometry remain adapters.
Remote and Tauri use `AgentSession` to prepare effects, deliver Harness commands,
then commit UI intent; failed delivery does not acknowledge or clear a draft.
Only safe editor/settings effects cross the UI wire. `ui_agents` uses its own
revision and does not advance Harness replay cursors. Browser tests invoke the
production Rust reducer through `examples/ui_agents_fixture.rs`.

Verification: 31 Rust agent checks, six desktop/mobile Agent Group browser checks,
two desktop host wire checks, Web typecheck and RN typecheck/lint passed. The
browser checks caught and corrected a changing accessible dialog name and hidden
empty state in the shared projection. Group settings forms remain native to the
TUI, and shell/navigation/agent session ownership still needs consolidation.
This is another incremental seam, not complete platform parity.

`UI/application_session.rs` now owns navigation, shell and agent interaction state
for the TUI, Remote client runtimes and Tauri's worker. Opening/dismissing Agents
coordinates the same semantic state, and accepted resource navigation validates
current surface identities. Adapters publish compatible shell/agent messages from
one aggregate snapshot; Tauri startup now fetches `application_projection` once.
Independent adapter reducers `RuntimeUi` and `DesktopUi` have been removed.
Temporary native field accessors reconcile through the controller while panels
migrate. The TUI's prepared agent effects commit after successful Harness delivery
for both keyboard and pointer input; failure preserves creation intent and text.
A browser batching regression also ensures accepted editor effects survive a
runtime refresh before the next paint and do not replay across reconnect.

Verification: 337 library tests passed with only the two previously attributed
worktree assertions excluded; 13 shared UI tests cover controller coordination,
stable identity, rollback and refresh. Three Remote integration/wire checks, two
desktop host checks, native navigation/rendering/delivery checks, Web typecheck
and RN typecheck/lint passed. Twenty-six targeted desktop/mobile browser checks
passed, with four skips for inapplicable layouts. Unrelated checkout edits remain
unstaged. Foundation recall/remember still return internal errors.

Controller ownership is consolidated, but graphical adapters still consume its
compatibility shell projection rather than rendering all ApplicationView primary
content. Graphical Home/Files/Diff parity, conversation/composer semantics, settings
forms, renderer path ownership under Platforms and remaining runtime source moves
are still required. The blank native OpenPencil specification remains in force.

The provider-neutral runtime types and persistent provider bridge now live under
`Harness/core.rs` and `Harness/core/`. The root `core` import is a compatibility
reexport of that implementation, not a second owner. Runtime asset lookup retains
its configured/executable/workspace behavior. An isolated checkpoint build passed
all targets and three existing core tests. Two stale Agent Group preview literals
now use defaults for newer runtime fields. Five source-layout checks passed; the
sixth found unchanged committed RuntimeSource auth.ts at 1205 lines against a
1200-line ceiling. The unrelated auth worktree edits are preserved.

Conversation structure and interaction now live in `UI/conversation/` and
`UI/conversation_session.rs`, owned by the aggregate ApplicationSession. Shared
projections define message controls, activity grouping, attention and disclosure,
reasoning traces, detail sections, stable selection identities and semantic icons.
TUI and Web render those projections; native hit geometry, scroll, clipboard and
editor focus remain adapters. The old terminal grouping implementation, including
its test-only duplicate, has been removed. An explicit inspect-work preference
preserves terminal attention behavior and browser collapsed defaults through the
same shared reducer.

Remote and Tauri prepare conversation actions, deliver any Harness command, then
commit shared intent. Only safe copy/edit effects cross the UI wire. Conversation
revisions remain separate from Harness replay cursors. Hosts project accumulated
history after sparse updates and streaming deltas; a sparse absent transcript
does not erase visible history. Consumed editor/clipboard effects cannot replay
on remount. Browser fixtures invoke the production Rust projection and reducer.

Verification: 343 library tests passed, excluding the same two previously
attributed worktree assertions. Shared model, terminal renderer, Remote sparse
streaming and desktop wire checks passed. Thirty-two existing desktop/mobile
conversation cases, two new edit/regenerate cases, and eight focused clipboard
and control cases passed across targeted runs. Web and RN typechecks passed.
An isolated staged Rust snapshot passed 342 library tests with the runtime asset
directory configured explicitly; five focused browser checks also passed against
the staged renderer and shared TypeScript sources. Foundation is available again
and architecture findings have been recovered and
persisted. Unrelated provider, layout, stylesheet and test edits are preserved.

TUI selection/disclosure consumes shared state; shared message controls still need
native input bindings. Composer semantics, settings, graphical Home/Files/Diff
rendering parity, thin adapter ownership under Platforms and remaining runtime
moves are still required.
The blank native OpenPencil design constraint remains in force.

Session persistence now lives in `Harness/session_store.rs` and its six
submodules: manifest/object commits, append-only events, locking, legacy readers,
export and workspace identity. Root `session_store` remains a compatibility
reexport. No storage format, filesystem behavior or locking policy changed.
Twelve existing persistence tests and all-target compilation passed.

The Harness adds `ResolvePermission { request_id, granted }` for shared UI
responses. PermissionBroker matches the pending request ID atomically; delayed
input cannot resolve a replacement request. Legacy permission commands remain
compatible. Shared composer responses will use this identity-bearing command.

`UI/composer/` and ComposerSession now own submit/edit/cancel/interrupt intent,
permission controls with request identities, attachment readiness, command
suggestions/destinations and semantic controls. ApplicationSession owns the
composer alongside conversation/navigation/agents. Hosts supply authoritative
workspace/session/unsaved context; native editors retain text input, cursor,
upload resources and draft checkpoint storage. Prepare/commit preserves failed
delivery and newer editor revisions, and rejects stale submissions. Three shared
behavior tests cover routing/delivery, readiness/edit guards and permission
identity. A production Rust composer fixture supports browser integration tests.

The shared model is an incremental checkpoint. TUI and host/browser integration
remains in progress and must be verified and selectively committed; its presence
in the worktree does not yet prove complete composer parity. Delegated agents
stopped with workspace-credit errors. Foundation saves also remain unavailable;
Git and these notes preserve the recoverable state.

Composer adapter verification in progress after checkpoint 896b47d: production
Rust browser fixture and mock UI composer channel added. Seven desktop draft
context checks now pass. These caught a real accepted-editor comparison bug:
JSON object key order differed between Rust serialization and the browser,
preventing draft clearing. Native effect matching now compares snapshot field
values with a canonical identity. Shared suggestion selection and guarded editor
replacement are wired into Web. Concurrent design commits incorporated the Web composer adapter; transport and
TUI changes remain uncommitted pending permission, attachment/edit, host and
selective staging checks. Initial dirty
Composer snapshot is `/tmp/yeet-composer-host-start.tsx`; native snapshots are in
`/tmp/yeet-composer-native/`. Preserve concurrent design edits.

The full worktree library suite now passes 348 tests, with the same two known
unrelated worktree assertions excluded. It caught and fixed native permission
input capture and approval hint casing regressions. Shared composer environment
now explicitly supports freezing editor input during permission shortcut capture;
Web preserves its drafting behavior. TUI opts into capture. The delegated agents
remain stopped after credit errors; continue from current source and snapshots.

Composer adapters now route TUI Enter/slash/extension submission, permission
shortcuts and interrupt through the shared controller. Terminal input/history,
geometry and focus remain native. Remote/Tauri publish revisioned composer views
and safe editor effects; command delivery precedes commit. Web renders shared
permission controls and suggestions, and clears only matching editor snapshots.
The browser permission adapter retains return-focus ownership across keyed card
replacement and restores it on resolution without taking focus from another
control. Working design edits incorporated into concurrent commits are preserved.

Verification of the selectively staged adapter checkpoint: 347 isolated library
tests passed with existing runtime assets configured, excluding the same two
known worktree assertions. Web/RN typechecks and three desktop host tests passed.
Seven desktop draft-context checks passed. Ten permission accessibility checks
and one desktop semantic permission check passed; three inapplicable viewport
cases skipped. Unrelated native/provider changes remain unstaged.

Remaining composer work includes shared edit-entry annotation conversion, native
message control bindings and toolbar/menu ownership. Shared settings, graphical
Home/Files/Diff parity, actual thin Platforms ownership and residual Harness
runtime moves remain required for the full architecture goal.

Agent execution now lives under `Harness/agent.rs` and its 35-file subtree.
The root `agent` module is a compatibility reexport. Turn coordination, cache
continuity, progress/retry policy and tool protocol are unchanged; only restricted
visibility paths and the scripted test fixture location needed adjustment.
Group orchestration and its Harness command intents now live under `Harness/agents`; the root `crate::agents` re-export preserves existing imports.
All-target compilation and all 81 existing agent tests passed, including four
scripted continuity checks. Six source-layout checks passed in the worktree.

Runtime approval arbitration now lives in `Harness/permission.rs`; root
`permission` is a compatibility reexport. Request waiting, notifier callbacks,
closure and atomic request-ID matching are unchanged. The existing stale-request
regression and all-target compilation passed. Shared UI owns presentation and
control eligibility; Harness owns waiting and resolving the actual request.

An outstanding dependency audit finding: Harness settings projection still uses
root theme palette resolution/catalog and exposes resolved colors in runtime
settings. Theme configuration storage and platform-neutral UI palette projection
need a later separation without introducing a Harness-to-UI dependency.

`UI/settings/` and SettingsSession now own ordered root sections, typed
choice/toggle/navigation/editor controls, semantic icons, stable selection,
provider/capability actions and shared availability policy. Flex eligibility
uses authoritative model/provider authentication facts; settings-working blocks
mutations consistently. Prepare/commit preserves rejected editor delivery.
Context shorthand/reset uses the existing configuration parser; theme values
remain Harness commands rather than UI persistence or palette resolution.

The exact staged shared Settings model passed three behavioral tests and
all-target compilation in an isolated checkout. Native/Web root adapters and
transport are still being verified before their own checkpoint. Advanced
settings forms and graphical content parity remain incomplete.

Shared Settings root adapters now consume the same ordered controls/actions in
TUI and Web. Native root index arithmetic and duplicate policy have been removed;
stable control IDs survive dynamic capability insertion. Advanced forms remain
native pending later extraction. Remote/Tauri publish separate Settings revisions
and safe effects, with Harness delivery before committing editor intent.
Browser tests invoke the production Rust reducer rather than duplicate policy.

The exact staged adapter snapshot passed 351 library tests with explicit runtime
assets, excluding the same two previously identified assertions. Eight native
settings checks and the Agent Group return-navigation regression passed. Eight
desktop/mobile browser checks cover commands, Flex eligibility, busy/offline
behavior and dismissal. Web/RN typechecks and desktop host compilation passed.
Unrelated worktree changes remain preserved.

Home now consumes canonical `UI/home` resource presentation, recent-target
history, ordered activity groups, empty messages, inspector actions and stable
selection/scroll behavior. Harness owns the nonblocking Git refresh worker.
`src/workbench` remains a compatibility facade. TUI keeps native viewport
geometry, rendering and input translation; its existing Home design is preserved.
This is an application composition seam, not graphical Home parity: Web has no
new Home design and Expo remains blank pending the user’s specifications.
