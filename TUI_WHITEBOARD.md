# TUI tool-call presentation

# TUI scheduled tasks

`/tasks` opens the terminal Tasks manager without sending a chat message.
Tasks list their schedule, enabled/running state, next due time, last result
and session ID. n creates, e edits, p pauses/resumes, d requests typed deletion,
r refreshes; arrows/j/k select and Esc closes. The editor uses Tab to cycle
literal fields, Ctrl+S to save, and Ctrl+B on Folder to browse directories
(Enter descends, Backspace goes up, Ctrl+S chooses the current directory).
Schedules are timestamps with explicit timezone, intervals in seconds, or
the next provider-reported 5h quota reset (one-shot, not a guessed timer).
Folder, prompt and model are captured per task.

Rust Harness owns validation, durable storage, claims, execution and a typed
command/state API. Only the TUI has a new UI. The background daemon keeps a
single scheduler owner alive while enabled jobs exist; no OS wake/startup
integration is installed. Overdue jobs run once on restart, intervals skip
missed ticks, and interrupted claimed runs are not automatically replayed.
Each run creates an independent saved session with existing sandbox policy;
unattended permission/question prompts fail rather than silently approving.

# TUI response selection and context menu

Dragging selects the visible conversation text without bubble padding or work
decorations; Ctrl+C copies that selection. Right-clicking inside a selection
keeps it, while right-clicking elsewhere clears the stale selection. The menu
puts exact-message copying first: assistant responses copy their original
Markdown rather than rendered/wrapped terminal text. Selection copying, paste,
and clear have explicit enabled states, and pointer movement highlights the
active menu item. Message edit/regenerate controls remain available below the
clipboard actions.

Ctrl+Shift+C/V and Cmd+C/V handle copy/paste when forwarded by the terminal.
Paste uses the same path as the context menu; terminal bracketed paste remains
supported. Plain Ctrl+V is not bound. Ctrl+C copies an active
transcript selection; without a selection it still interrupts/quits.
The default light palette uses dark semantic text colors (including success,
warning, and user labels), not bright icon colors on near-white surfaces.

Tool calls use the existing compact transcript rows, not raw JSON as their
primary presentation. Each call shows its authoritative lifecycle status:
Preparing, running, Awaiting approval, Completed, Failed, Timed out, Cancelled,
or Interrupted. Suppressed calls remain hidden. Status takes priority when
terminal width is limited; normal states must not use error styling.

Existing disclosures retain full arguments and output, with named detail
sections (Input, Output/Result, Error, etc.) and duration when available.
Grouped calls retain individual statuses in their expanded child rows.
No new screens or native frontend layouts are introduced.

Shell output shown in the existing tool disclosure interprets carriage returns
as a move to the beginning of the row rather than deleting the remaining text.
This static preview is not a terminal emulator or an interactive shell UI; ANSI
control sequences are still stripped for safety, and overwrites use character
positions rather than terminal cells (so wide/combining glyphs can differ).

# TUI attachments

When a terminal delivers a file drop as a bracketed paste, a complete list of
existing absolute paths is queued in the focused chat composer. Pasting such
paths behaves identically because terminals do not distinguish paste from drop.
The existing composer shows attachment names above the prompt, or a count when
only one row is available; Ctrl+X removes the last one. PNG/JPEG/WebP/GIF are
sent as image inputs; other files are referenced by their local paths for the
agent's filesystem tools. Queue up to
eight files, with a combined 20 MiB limit. Ordinary text paste is unchanged.
Slash commands leave the queue intact; a submitted message consumes it, and
changing composer workspace/session discards it.

# TUI session management

Sessions opens with `s` from unfocused chat or the `/sessions` command (no
function keys). In normal mode hjkl are strictly Left/Down/Up/Right aliases,
not activate/back shortcuts. Horizontal arrows do nothing in these vertical
lists. The picker uses g/G to jump, Ctrl+U/D to page, Enter to open, n to
create, and q/Esc to close. `/` enters search;
Enter/Esc returns to normal mode without opening a row. `e` opens management for
the selected stable session ID (not the live filtered index). The nested panel
offers rename (r), move workspace (m), and delete (d); j/k selects, Enter
continues, and q/Esc returns without changing data. Editing fields keep
literal text input, so navigation letters never steal names or paths.
Rename and workspace path fields support paste and Ctrl+U; Tab cycles known
workspace paths. Deletion requires typing `delete`, and creates a recovery
archive using the existing export facility before removing the session.
Loaded sessions cannot be moved or deleted; running sessions cannot be renamed.
Rust Harness commands and SessionStore own validation and persistence. Both
portrait and landscape pickers advertise the same management shortcut.

# TUI memory management

User approved building a functional terminal manager using existing TUI patterns while leaving the OpenPencil-controlled app blank. `/memories` and the Memories entry in `/settings` open one local overlay, never a model prompt or conversation entry. Protected text stays exclusively in the local manager and is never placed in shared Harness/Remote replay state.

Use existing modal surfaces: j/k or arrows select; Enter inspects source/text; `/` edits search; kind/privacy filters and page controls are explicit. Add/edit uses a literal multiline draft with paste, Ctrl+U clear and Ctrl+S save. Delete and declassification require typed confirmation for captured IDs/revisions, including bulk selections. Escape cancels without saving. Local-only agent defaults stay unavailable until local provenance/egress guards are ready; administrative protection of stored text remains safe. Settings expose independent system enablement and validated embedding-profile selection. Model preparation must be an explicit confirmed background task, never a render-time download. Backend memory code owns storage, validation and policy; the TUI owns only input and rendering.
