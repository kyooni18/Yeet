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

The terminal application still mixes most UI state and navigation transitions
with terminal input, rectangles, rendered transcript caches and hit targets.
Its renderer still selects and orchestrates application views. Migrating the
TUI into the shared architecture remains required.

The working browser client remains in `web/`. `RemoteShell.tsx` independently
owns sidebar, inspector and sheet visibility, dismissal priority and compact
navigation interactions. The existing Remote v1 protocol exposes Harness
state/events/commands, not shared application UI projections.

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
