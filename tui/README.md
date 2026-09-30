# Terminal frontend

This directory owns Yeet's terminal UI and is a module of the existing `yeet`
crate, not a separate Cargo package. `src/lib.rs` loads `tui/mod.rs`.

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

## Verification

```sh
cargo check --all-targets
cargo test --lib tui::
```
