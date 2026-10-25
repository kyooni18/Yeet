# Terminal frontend

This directory owns Yeet's terminal UI and is a module of the existing `yeet`
crate, not a separate Cargo package. `src/lib.rs` loads `Platforms/tui/mod.rs`.

## Current structure

- `app.rs`, `app/`: persistent UI state and interaction behavior.
- `ui.rs`, `ui/`: shell, rendering, responsive layout, views, components, dialogs.
- `runtime.rs`: terminal lifecycle, event loops, clipboard, local and legacy
  remote-terminal entry points.

The public entry points are `yeet::tui::run()` and
`yeet::tui::run_remote(options)`. State and rendering are available through
`yeet::tui::app` and `yeet::tui::ui`; legacy `yeet::app` and `yeet::ui` remain
compatibility aliases of those modules, not duplicate implementations.

## Boundaries and evolution

Backend/runtime services, domain models, reusable workbench resource collection,
and shared theme/text utilities remain outside this directory. TUI adapters may
consume those services; services should not own focus, selection, or geometry.

Preserve existing behavior while extracting shared geometry, typed view-instance
state, actions/effects, focus routing, and overlay lifecycle. Split modules when
real behavior warrants it rather than adding empty framework scaffolding.

Read and maintain `TUI_WHITEBOARD.md` before structural UX changes. Keep stateful
view tabs, contextual rails, and separate command-palette/view-switcher behavior.

## Managing sessions

Open Sessions with **s** from unfocused chat (Esc leaves the composer), or submit
**/sessions**. No function keys are used for sessions.
Use **j/k** to navigate, **g/G** for first/last, **Ctrl+U/D** to page,
**l/Enter** to open, **n** for new, and **q/Esc** to close.
**/** enters search; **Enter/Esc** returns to normal mode with the filter kept.
Press **e** to manage the selection, then **r** to rename, **m** to move workspace,
or **d** to delete. The menu also supports j/k, l/Enter, and h/q/Esc to go back.
Fields accept paste; Ctrl+U clears them. In Move workspace, enter an existing
directory or press Tab to cycle known workspace paths. Moving retains history
but resets the session's working directory and extra context roots.

Delete requires typing `delete` and Enter. A verified recovery archive is kept
under the Yeet configuration directory's `exports/` folder. Loaded sessions
cannot be moved or deleted; switch to a new session first (other clients or
detached runtimes may still hold it). Stop a running session before renaming it.
Esc cancels the edit, then returns from management to the searchable picker.

## Verification commands

```sh
cargo check --all-targets
cargo test --lib tui::
```

## Shared kit

`kit::Ui` paints controls and records typed actions in `kit::HitMap` using the
same clipped rectangle. Begin the hit map on every frame, including empty/tiny
views; resolve input only against the latest painted geometry. Diff is the first
consumer. `visible_start` is shared with the Home session list.

`Tabs<T>` owns independent `View<T>` instances and stable IDs; it provides
open/activate/close with neighboring-tab fallback. File and Diff tabs now use this store. File instances own their resource,
rail directory/selection, filters and display options; the browser retains its
own state. Terminal actions carry IDs; shared/wire index navigation is translated
at the adapter boundary. Diff pointer geometry lives in a separate frame record,
invalidated on navigation and each paint (including tiny frames). Home/Session
remain existing singleton surfaces, not generic view instances. `FloatingView` centralizes bounded centered placement and
modal blocking policy, used by the view-switcher layout. Its policy must still
be applied by event routing; it is not an automatic focus manager.

`Toasts` retains at most four notifications, expires them against an explicit
clock, and paints without hit targets or focus. Backend errors currently use it.
The kit is an incremental foundation, not a completed declarative framework.
