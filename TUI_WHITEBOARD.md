# Yeet TUI Workbench Whiteboard

Status: living design whiteboard, not a frozen specification.

1. Top tabs: What view state do I have open?
2. Context rail: What nearby/recent things are relevant to this view?
3. Main surface: What am I looking at or working on right now?

The bottom composer remains the direct way to ask or command Yeet.

Session picker activity is daemon-owned: show `running` or `permission` for any
live session, including detached sessions, independently of the current-session
marker. Idle sessions retain their age. Activity is ephemeral, not saved to disk.

## Non-negotiable mental model

### Views are peers, never children of Sessions

The workbench owns navigation and open view state. Sessions is one peer view,
not the app root or a parent shell for Overview, Files, Diff, Agent, Settings,
or other views. Session activity, errors, permissions, and conversation changes
must not choose another view's content or reset its local navigation. Shared
chrome and global dialogs belong to the workbench, not to the session renderer.
Closing a resource view returns to the workbench Overview, not implicitly to a
conversation. Overview has its own mixed recent-object rail, not a sessions menu.
These rules supersede the historical conversation-centric migration notes below.

### Tabs are stateful view instances

Tabs are NOT view-type buttons.

A tab represents one concrete open view state, similar to an IDE editor tab.

Examples:

- Overview for workspace A
- guidance_taem.c at line 418 with a local selection
- Agent: Landing with its own internal section selected
- Run MM305 with output position and filters
- Settings with Providers selected
- a session/conversation view

Conceptually:

    Tab
      id
      view_kind
      resource_identity
      title
      local_navigation_state
      selection
      cursor
      scroll
      filters
      sidebar_state
      view_specific_state

Switching tabs restores the complete state of that view, including the contextual rail/sidebar.

Do not model Files, Agent, Database, etc. as permanent tabs that merely select a mode. A File view and another File view can coexist as different tabs.

### The left area is contextual, not global navigation

The left column changes meaning with the active tab.

Examples:

- Overview -> unified recent/relevant objects
- File -> outline, symbols, related files, local history
- Agent -> agent sections, context, activity, related runs
- Settings -> table of contents
- Database -> collections, queries, recent objects

Do not turn it back into a fixed global menu.

### Command Palette and View Switcher are different

Cmd+P is the Command Palette.

The View Switcher is a separate overlay for switching among open view instances and, optionally, opening a new view.

The Command Palette executes actions.
The View Switcher changes what is visible.

Do not merge them just because both are searchable overlays.

## App shell

High-level structure:

    +--------------------------------------------------------------+
    | Yeet | [tab] [tab] [tab] [+]                                 |
    +------------------+-------------------------------------------+
    | contextual rail  |                                           |
    |                  |              active view                  |
    |                  |                                           |
    |                  |                                           |
    +------------------+-------------------------------------------+
    | > Ask / command Yeet...                                      |
    +--------------------------------------------------------------+

Use secondary background colors mainly for:

- tab strip
- contextual rail
- selected/focused rows
- overlays
- compact controls

The main view should mostly stay on the base background.

Avoid filling the main area with card surfaces.

## Overview view

Overview is a workspace-level status document.

It should NOT look like a web dashboard.

The main Overview surface is one vertically scrollable document with a natural reading order.

Suggested section order:

    Overview
    workspace / branch / compact status

    Now
    Active Goal
    Recent Activity
    Changed Files
    Runs
    Issues
    Suggested Next Action

Optional sections may appear only when they have meaningful content:

- Recent Findings / Notes
- Artifacts
- Approvals / Decisions Needed
- Usage / budget warning
- Tests / build health

### Main Overview rules

- Single vertical scroll.
- No grid of cards.
- Prefer whitespace, alignment, indentation, and horizontal rules.
- Section titles are anchors, not cards.
- Keep each section compact by default; allow expansion/detail views.
- The current state should be understandable from the first screenful.
- Do not duplicate information already obvious in the left rail unless the main page needs it for the workspace narrative.

## Overview contextual rail: Unified Recent Objects

The Overview rail is NOT a navigation sidebar and NOT a table of contents.

It is a unified, mixed list of recently relevant objects.

Possible object kinds:

- Agent
- Commit
- Push
- Issue
- Session
- Run
- Goal
- Artifact
- File
- Pull request, if available
- Approval / decision
- other openable Yeet resources

Example:

    RECENT

    * Agent
      Landing
      working - 2m

    * Commit
      8f3a2c1
      Improve TAEM pitch control - 5m

    ! Issue #42
      Crosswind instability
      open - 12m

    o Session
      MM305 tuning
      28m

    ^ Push
      main -> origin/main
      41m

Do not group rows under headings such as Agents / Sessions / Git. Mixing types is intentional.

The rail answers: What have I been dealing with recently?

### Recent object row

Each row should be compact:

    [type/status glyph] primary title                 age/status
                        optional secondary detail

Requirements:

- one stable resource identity
- one primary label
- optional secondary/status line
- compact age/status metadata
- semantic icon/glyph
- selected/focused state
- open action

### Deduplication

Recent is object-oriented, not raw-event-oriented.

Examples:

- An agent performing 15 actions should normally still occupy one Agent row whose state and timestamp update.
- A session should occupy one row.
- A run should occupy one row.
- Separate commits remain separate because each commit is a separate object.
- Repeated file reads do not spam the rail; a file may be represented once if it is itself considered a recent object.

The main Activity section remains the event-oriented history.

### Ranking

Default ordering is recency, with a small priority bias for:

1. currently active / running
2. requires user attention
3. recently changed
4. ordinary recent objects

Do not make the ranking clever enough to become unpredictable.

## Opening objects from the rail

Selecting an item should open its corresponding view state.

Examples:

- Agent -> Agent view
- Commit -> commit/diff view
- Issue -> Issue view
- Session -> Session view
- Run -> Run detail view
- File -> File view

Open question: when an identical resource identity is already open, should Enter focus the existing tab or create another stateful instance?

Preferred default for now: focus an existing exact-identity tab; provide an explicit open-another-instance action later if real workflows require it.

## Tabs

Tabs behave like IDE tabs.

Required operations:

- create/open a view instance
- close current tab
- switch previous/next
- switch by searchable View Switcher
- restore local state when revisiting a tab

Likely bindings, subject to final keymap:

- Ctrl+W: close current tab
- Ctrl+Tab: next / MRU tab
- Ctrl+Shift+Tab: previous / MRU tab
- View Switcher shortcut: TBD
- Cmd/Ctrl+P: Command Palette, not View Switcher

### Tab overflow

Do not let a long tab strip destroy the layout.

Candidate policy:

- active tab is always visible
- adjacent tabs get priority
- inactive titles truncate
- View Switcher is the authoritative way to reach off-screen tabs
- no horizontal tab-strip scrolling as the primary mechanism

### Tab persistence

Open question: whether tabs persist across Yeet restarts.

If implemented, persist serializable view state only. Runtime-only handles must rehydrate safely or degrade to a useful static view.

## Marquee text for constrained widths

Use marquee behavior for text that cannot fit horizontally.

Target behavior is similar to iPhone Now Playing Live Activity: the fixed-width viewport stays put while the text reveals itself by moving inside it.

Preferred animation: ping-pong marquee.

    start -> pause -> scroll left -> pause -> scroll right -> pause -> repeat

### Where marquee is useful

Primary candidates:

- selected Overview recent-object row
- active tab title
- selected row in other contextual rails
- selected result in View Switcher, if needed

Do NOT animate every truncated row at once.

Non-focused rows remain static and truncated.

### Marquee trigger

Only animate when all are true:

- text display width exceeds available width
- row/tab is active or focused
- UI has been stable/focused for a short delay

Suggested defaults for prototyping:

- start delay: 700 ms
- edge pause: 800 ms
- speed: roughly 8 terminal columns / second
- frame/tick cadence: 50-100 ms, chosen to avoid wasteful redraws

These are tuning values, not contracts.

### Marquee implementation rules

Terminal widths are display cells, not bytes or Rust char counts.

Implementation must be safe for:

- Unicode grapheme clusters
- wide CJK characters
- combining characters
- emoji / variation sequences where terminal support allows

Never slice a UTF-8 string by display-column index.

Use grapheme-aware iteration plus terminal display-width accounting.

The row's right-side metadata should stay anchored. Only the title viewport should marquee.

Example:

    [Agent] Investigating MM305 crosswind ene...     2m
            <------ title viewport --------->

When selected, only the title viewport moves.

### Reduced motion / accessibility

Provide a way to disable or globally reduce marquee animation.

When disabled, use deterministic truncation.

Animation must never be necessary to understand the object's type or state.

## Overview main surface: detailed proposal

### 1. Header

Compact only.

    Overview
    KSPShuttleLander / main

Optional right-aligned compact status is fine, but avoid a second dashboard row of badges.

### 2. Now

The most important current work.

    NOW
    Working on MM305 crosswind tuning for TAEM landing.
    Running simulation -> analyzing result -> adjusting lateral guidance.
    In progress | MM305 | 24m

### 3. Active Goal

Goal text plus concise checklist/progress.

Avoid large progress cards.

### 4. Recent Activity

Event-oriented timeline/log.

This is distinct from the Overview rail:

- rail = recent OBJECTS
- activity = recent EVENTS

Examples:

- edited file
- ran command
- agent delegated work
- test passed
- commit created
- decision requested

### 5. Changed Files

Git-style list.

Keep descriptions optional and compact.

### 6. Runs

Latest relevant runs, not an exhaustive run browser.

Show status/result/duration and open on Enter.

### 7. Issues

Only blockers/warnings that matter to current workspace state.

Do not turn Overview into an issue tracker.

### 8. Suggested Next Action

One strong next action is better than a menu of suggestions.

Can expose a command or action that is directly executable/editable.

## Scrolling behavior

Overview main surface scrolls vertically.

The contextual rail has its own independent selection/scroll position.

Tabs retain both when switching away and back.

Suggested focus model:

- Tab strip
- Context rail
- Main surface
- Composer
- Overlay when open

Focus movement should be predictable and visible.

Main-surface scroll should not accidentally move the context rail.

## Responsive behavior

### Wide

- contextual rail visible
- main surface gets majority of width
- metadata may align into columns within a section

### Medium

- narrower rail
- secondary row details reduced
- main remains single-column

### Narrow

- main surface wins
- contextual rail collapses into an overlay/drawer
- tab strip shows active plus as many neighbors as fit
- active tab title may marquee
- Overview remains usable without the rail

Never convert the Overview main surface into a cramped multi-column card grid to use extra width.

## Color / surface rules

Main background: base surface.

Secondary background: use primarily for:

- tabs
- contextual rail
- selected rows
- overlays
- input/composer chrome

Main document:

- mostly base background
- horizontal rules
- text hierarchy
- semantic foreground colors
- very occasional inline highlight

Semantic colors:

- green: success / running-positive
- yellow/amber: warning / needs attention
- red: failure / error
- cyan/blue: selection / navigation / links
- muted: metadata

Avoid assigning every section its own decorative color.

## Data model sketch

Names are illustrative.

    struct WorkbenchTab {
        id: TabId,
        title: String,
        view: ViewInstance,
    }

    enum ViewKind {
        Overview,
        File,
        Agent,
        Run,
        Session,
        Commit,
        Issue,
        Settings,
        // ...
    }

    struct ViewInstance {
        kind: ViewKind,
        identity: ResourceIdentity,
        scroll: ViewScrollState,
        selection: ViewSelectionState,
        local_nav: LocalNavigationState,
        sidebar: SidebarState,
        filters: ViewFilters,
        payload: ViewPayload,
    }

Overview recent rail:

    struct RecentObject {
        identity: ResourceIdentity,
        kind: RecentObjectKind,
        title: String,
        detail: Option<String>,
        status: Option<RecentStatus>,
        updated_at: Timestamp,
        attention: AttentionLevel,
    }

Keep resource identity separate from display title so tab dedupe/open behavior is stable.

## Recent-object data sources

The aggregator should accept adapters rather than hard-code one giant query.

Likely sources:

- sessions/workspaces
- agent runtime
- runs
- local git commits and pushes
- issues if a source is available
- artifacts/files
- goals/approvals

Rendering must consume already-collected state.

Do not run blocking git/network/shell commands from a frame render function.

Use cached/event-driven/background refreshed state.

## Current code reality

### Sessions implementation slice (2026-09-29)

The active Sessions view is integrated into the existing conversation shell,
using `TUI-Mockup.fig` → `Desktop / Session` as its visual reference. This is
a deliberately scoped slice; the workbench tabs and other views described below
remain future work.

- `Mode::Chat` hosts the active conversation. `Mode::Sessions` remains the
  searchable session picker, with its existing loading and keyboard behavior.
- `src/ui.rs` is a facade. `src/ui/sessions.rs` owns the transcript viewport,
  selection, and message presentation; `src/ui/sessions/tools.rs` owns tool traces.
  Shared rendering, text layout, and live progress live in concern-based modules
  under `src/ui/`; the arbitrary `ui_parts` includes are removed.
- Reuse the shell's workspace/session rail, responsive geometry, theme tokens,
  composer, status, and overlays. No replacement shell or global tabs are added.
- Conversation entries and live bridge fields are the source of displayed data.
  Mockup text belongs only in design references, never in runtime state. Missing
  conversation data has an explicit empty/unavailable state.
- Tool rows keep their recorded order and use backend operation labels/details
  when present. The backend updates the current activity entry in place, so
  its live title and detail are also shown in the persistent task strip.
- Use Nerd Font equivalents for the mockup's raster/vector tool icons. The
  terminal controls the actual font; the application keeps the selected theme.

The initial whiteboard described the native TUI as conversation/session-centric.
It remains the current shell and now hosts the active session conversation view.

Key current implementation:

- src/ui/shell.rs
  - draws a responsive shell around the conversation
  - sidebar is currently workspace + session oriented
  - current SidebarRowKind is Workspace / Session
  - current rows truncate into fixed single-line geometry
- src/ui/responsive.rs
  - owns responsive layout metrics
- src/ui/theme.rs
  - owns surface/selection/theme primitives
- src/ui/composer.rs
  - existing bottom interaction surface
- src/ui/dialogs/navigation.rs
  - likely relevant to navigation/switching overlays
- src/app.rs and src/app/*
  - application state / selection / session state

The implementation should evolve these concepts rather than bolt a second unrelated shell beside them.

`src/ui/shell.rs` continues to own the workspace/session rail for this view. Its
rows remain compact and session-oriented; a future non-session view may provide
different contextual content within the same shell.

## Migration strategy

Avoid a one-shot rewrite.

### Phase 0 - State model

Introduce workbench tab + view-instance state without radically changing rendering.

Goal: prove multiple stateful view instances can coexist.

### Phase 1 - Tab strip

Add IDE-like tab rendering, switching, closing, and restoration.

Keep the existing conversation as one possible view during migration.

### Phase 2 - Overview main surface

Add the vertical Overview document with real data where already available and explicit placeholders/adapters where not.

No card grid.

### Phase 3 - Unified recent rail

Replace the Overview sidebar content with RecentObject aggregation.

Start with sources already available locally:

- sessions
- current workspace
- git commits
- active agent/run state

Then extend.

### Phase 4 - View Switcher

Add the searchable open-view switcher.

Keep it separate from Cmd+P Command Palette.

### Phase 5 - Marquee

Add a reusable focused-overflow text viewport.

Use it first in the recent rail and active tab.

Add tests for Unicode width, edge pauses/state machine, narrow widths, and non-overflow text.

### Phase 6 - Migrate other views

Move File, Agent, Settings, Run, Session, etc. into the same stateful-view contract one by one.

Do not block the Overview work on every future view being complete.

## Guardrails

Do NOT:

- make Session the root identity of the whole UI
- turn the Overview rail into a fixed global navigation menu
- group the Overview rail into Agents / Sessions / Git sections
- make tabs synonymous with view kinds
- make the Overview main view a grid of cards
- use secondary filled backgrounds all over the main document
- add an embedded terminal view just because Yeet runs commands
- merge Command Palette and View Switcher
- animate all truncated text simultaneously
- execute expensive data collection during frame rendering

## Initial acceptance criteria

The first convincing Overview prototype should satisfy all of these:

1. Top strip visibly behaves like stateful IDE tabs.
2. Overview left rail shows mixed recent objects from at least three object kinds.
3. No fixed Overview navigation menu occupies the rail.
4. Main Overview is one vertically scrollable document.
5. Main Overview contains no card grid.
6. Secondary background is concentrated in tabs, rail, selection, overlays, and composer.
7. Activity is event-oriented; recent rail is object-oriented.
8. Switching away from Overview and back restores main scroll + rail selection/scroll.
9. Long selected recent-object text can marquee without moving right-side metadata.
10. Non-selected overflowing rows stay static/truncated.
11. Narrow layout can hide/collapse the rail without breaking Overview.
12. Existing session workflows remain reachable during migration.

## Open questions

Keep these visible until answered by implementation experience.

- Exact shortcut for View Switcher?
- MRU vs linear semantics for Ctrl+Tab?
- Should opening an already-open exact resource focus it or duplicate it?
- Should open tabs persist across restart?
- How much recent-object ranking bias should active/attention states receive?
- Which issue sources belong in native Overview initially?
- Should commits and pushes be separate recent-object kinds when a push contains several commits?
- How should the rail represent one agent with several concurrent runs?
- Marquee timing/speed after real terminal testing?
- Should the rail retain its own history per Overview tab/workspace?
- What is the minimum terminal width at which the rail stays visible?

## Suggested first implementation slice

Do the smallest architecture-first slice:

1. Define ViewKind, ResourceIdentity, ViewInstance, and WorkbenchTab.
2. Wrap the current conversation UI as a view instance so existing behavior keeps working.
3. Add a minimal Overview view instance.
4. Add a top tab strip with two real stateful tabs.
5. Add Overview main vertical sections using existing state only.
6. Replace Overview's rail with a small unified RecentObject list using sessions + git commits + one active run/agent source.
7. Only after the interaction feels correct, add additional sources and marquee.

This keeps the architecture honest before polishing.

## Whiteboard maintenance

This file is intentionally a whiteboard.

Agents working on this area should update it when:

- a design decision becomes settled
- an open question is answered
- an implementation constraint invalidates an assumption
- a phase is completed
- a better interaction model is discovered

Do not silently let implementation diverge from this file. Either follow it or update the whiteboard with the new decision and reason.


## Current native Overview implementation note

The Home/Overview renderer uses the mockup's Desktop / Overview shell geometry and fixture labels (sessions rail, activity/file rows, decision inspector, tabs, composer). `cargo run --example tui_preview -- home 144 44` was raster-text inspected against the OpenPencil frame: its 232px rail and 540px activity pane scale to 23 and 54 terminal cells, respectively, and the inspector copy follows the same ordering and approximate vertical anchors. The inspector is offset to the mockup's measured 54px gap after the activity pane. This is a structural/content comparison, not a pixel-diff verification: terminal cell metrics and glyph rendering differ, and some fixture/detail positioning is still approximate. Do not claim pixel identity. The src/ui tree is organized into views, components, dialogs, support, and task responsibility folders (with shared render/shell modules at the ui root).


### Desktop Diff implementation

The Desktop / Diff frame (0:444 in ~/Desktop/TUI-Mockup.fig; the supplied underscore path is actually hyphenated on disk) defines a 232px changes rail, 958px main diff, and 250px inspector beneath the shared tab strip. Implement this as a Git-backed resource view, not FilesState.diff. Each diff tab owns its repository, selected path, context mode and scroll; file tabs and diff tabs coexist. Launcher opens/reuses the current repository diff; Files diff action targets the actual selected/open file. Render HEAD-to-working-tree changes including staged, unstaged, untracked, deleted and binary paths, with explicit clean/non-repository/error states. Controls must use real state (full file/changes only, file selection, hunk movement, refresh, open file). Do not copy fixture comments or pretend that mockup review metadata exists.


## Session input focus

The session composer keeps explicit input focus independently of draft contents. Esc unfocuses without discarding the draft; plain `i` refocuses without inserting the shortcut. Unfocused sessions accept transcript navigation, not draft edits or submission. Clicking the composer restores focus. The composer caret and surface reflect focus. Focus shortcuts are view-local: Files `i` opens/unlocks its find field; other dialogs retain their own input handling and never redirect `i` to the session composer.

### Session input focus

Session composer focus is explicit: Esc leaves editing without discarding the draft; i resumes editing without inserting the activation key. Unfocused navigation does not edit, paste into, or submit the draft. Composer styling/caret reflect focus, and clicking the composer restores it. Focus shortcuts are view-local: Files i activates its own search field; popup text entry remains owned by the popup.

### TUI responsiveness

Keep transcript styling/wrapped-row offsets cached across input and scrolling frames. Invalidate for conversation replacement, active streamed content, width, tool expansion, session and palette changes. Render only logical lines intersecting the viewport rather than wrapping all preceding scrollback. Cache memory scales with transcript text, not an entire terminal-sized scrollback buffer. Drain bounded input bursts per frame; cap backend processing by elapsed time as well as event count so background activity cannot indefinitely starve input.

Desktop Diff is now implemented in src/app/diff.rs and src/ui/views/diff.rs. Launcher Diff and Files `d` enter the same repository-owned review tab; each tab retains selection/context/scroll, supports shared keyboard cycling and mouse activation/close, and Enter opens the selected actual file tab. The view uses the mockup's rail/main/inspector proportions, numbered unified patches, real Git metadata and counts. `f` toggles full context, `n/p` moves hunks, arrows select/scroll, and `r` refreshes. A compact control row replaces the mockup composer: this review view does not falsely advertise agent submission or mock review comments. Narrow layouts hide the inspector/rail while retaining keyboard navigation. Comparison is structural, not pixel-identical. Focused Diff and navigation tests pass; the wider app/UI runs observed existing failures in two transcript wheel tests and the Home composer-row expectation amid concurrent workspace changes.

### Session work selection

With the composer unfocused (`Esc`), Up/Down or k/j select visible reasoning
and tool-work headers and keep the selected header in view. Enter, Space, or e
expands the selected work unit; the draft is never edited/submitted by these keys.
Selection is hidden while the composer or session rail has focus. A click on a
work header also selects it when the composer is unfocused; drag-to-copy remains
independent. Active/failed tool groups retain automatic expansion.

Reasoning summaries and reasoning text are shown as standalone transcript blocks,
with the full provider-supplied text visible separately from the compact header.
Tool calls stay in their own dropdowns; a reasoning summary never owns or expands
the following tool list. Reasoning blocks can be collapsed independently from tools.

## TUI subsystem ownership

The terminal frontend lives in repository-root `tui/`, integrated into the
existing Rust crate through `src/lib.rs`. This is a behavior-preserving first
step, not a replacement renderer or a completed state-model rewrite.

- `tui/app.rs` and `tui/app/`: existing UI state, input, selection, and navigation.
- `tui/ui.rs` and `tui/ui/`: rendering, shell, components, dialogs, views, and
  responsive layout helpers.
- `tui/runtime.rs`: local terminal lifecycle/event loop and legacy remote-TUI
  loop, including disconnect handling, clipboard integration, and cleanup.
- `src/main.rs`: CLI dispatch and non-terminal remote commands only.
- `crate::app` and `crate::ui`: compatibility re-exports; new terminal code
  should use `crate::tui`.

Backend services, shared theme definitions/text utilities, and reusable workbench
resource collection remain outside `tui/`. They must not acquire dependencies on
terminal focus or geometry. Historical source paths elsewhere in this document
refer to the pre-relocation implementation.

Next architectural steps stay incremental: shared layout/hit-test geometry,
stable view-instance identities, explicit actions/effects, focus scopes and
overlays, then reusable interaction behavior. Avoid creating empty abstractions
or moving shared domain services just to fill out a directory tree.

## Shared UI kit: initial implementation

`tui/kit` now contains a frame-local `Ui`, clipped typed `HitMap`, shared row
visibility policy, stateful `View<T>`/`Tabs<T>` with stable instance IDs,
`FloatingView` placement/blocking policy, and bounded expiring `Toasts`.
Diff controls paint and register their geometry together and dispatch typed
`DiffAction`s instead of storing separate per-control target fields. Diff and
Home share row visibility calculations. The view switcher uses floating-surface
placement. Backend error events enqueue six-second non-focus-taking toasts while
retaining the existing backend error state; toast painting follows all view paths.

This is deliberately an initial slice: existing workbench tabs have not yet been
migrated to `Tabs<T>`, and floating input/focus restoration still uses existing
mode routing. Buttons currently share mouse geometry, not automatic keyboard
focus/navigation. Selection/scroll states and declarative container composition
remain follow-up work; do not claim a complete UI runtime yet.

## Shared action and runtime agent boundary

`src/agents/actions.rs` owns agent frontend commands and navigation intents,
independent of Skyline. Root `actions`, model, and TUI paths retain compatibility
exports. `App::dispatch_action` interprets the common
Action envelope: navigation stays local, commands use the existing Harness
pipeline. Geometry, focus, and rendering remain in `tui/`. This is an incremental
boundary, not a migration of every widget-specific action or editor keystroke.
`src/agents/` owns process-wide runtime identity and snapshots, separate from
conversation storage, provider history, UI state, and private model reasoning.

## Stateful workbench tabs: first migration

File and Diff views now use the shared `Tabs<T>` store. Terminal navigation,
including painted tab actions, carries stable instance IDs rather than vector
positions. Closing a tab cannot retarget an old action to its neighbor. The
shared agent/wire navigation API retains positional compatibility; its adapter
resolves an index to an instance ID at dispatch time.

Each file view owns a typed `FileViewState`: resource identity, contextual rail
directory and selection, filters/find lock, display options, hints and local
count prefix. The browser owns a separate state. Switching or reopening a file
restores its state and refreshes external data without clearing its filters;
opening another browser directory preserves existing file views. Selection is
clamped if external contents change. Diff instances retain independent review
selection, display mode and scroll through the same tab lifecycle.

Diff hit targets and patch bounds are now separate frame geometry, not fields
of persistent `DiffState`. Navigation and every draw invalidate old geometry;
pointer routing requires the currently painted instance identity. Empty/tiny
frames cannot retain clickable controls from the previous frame.

This is a bounded migration, not a complete component runtime: Home/Session
remain singletons, other geometry still uses existing target collections, and
focus/overlay lifecycle and narrower component inputs remain follow-up work.


## Session startup welcome

The initial Sessions view uses the shared Yeet brand welcome renderer
(`shell::draw_welcome`). Do not add a Skynet-style/custom operator landing screen
or screen-specific input behavior. Normal composer submission and Esc focus
handling apply. The welcome remains separate from the workbench Overview and the
saved-session picker.
