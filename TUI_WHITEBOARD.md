# TUI tool-call presentation

Tool calls use the existing compact transcript rows, not raw JSON as their
primary presentation. Each call shows its authoritative lifecycle status:
Preparing, running, Awaiting approval, Completed, Failed, Timed out, Cancelled,
or Interrupted. Suppressed calls remain hidden. Status takes priority when
terminal width is limited; normal states must not use error styling.

Existing disclosures retain full arguments and output, with named detail
sections (Input, Output/Result, Error, etc.) and duration when available.
Grouped calls retain individual statuses in their expanded child rows.
No new screens or native frontend layouts are introduced.

# TUI session management

The existing Sessions picker remains searchable. Ctrl+E opens management for
the selected stable session ID (not the live filtered index). The nested panel
offers rename, move workspace, and delete; Esc returns without changing data.
Rename and workspace path fields support paste and Ctrl+U; Tab cycles known
workspace paths. Deletion requires typing `delete`, and creates a recovery
archive using the existing export facility before removing the session.
Loaded sessions cannot be moved or deleted; running sessions cannot be renamed.
Rust Harness commands and SessionStore own validation and persistence. Both
portrait and landscape pickers advertise the same management shortcut.
