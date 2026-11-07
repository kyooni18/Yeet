# Scheduled tasks (TUI)

Open `/tasks` in Yeet. This is a terminal manager, not a model/tool request.
No browser or native-app screens are added.

## Create and manage

- `n`: create; `e` / Enter: edit the selected job.
- Tab / Shift+Tab: move between Name, Folder, Model, Schedule and Prompt.
- Folder: type an absolute path, or Ctrl+B to browse directories. Enter descends,
  Backspace goes to the parent, Ctrl+S chooses the current directory, Esc cancels.
- Fields accept literal text and paste; Ctrl+U clears a field, Backspace removes
  its last character. Enter adds a newline in Prompt. Ctrl+S submits the job;
  errors leave the draft intact. Esc cancels an unsent edit.
- `p`: pause/resume future runs (does not interrupt a running job).
- `d`: delete, then type `delete` and Enter. Running jobs cannot be edited or
  deleted. Their saved sessions are never removed by task deletion.
- `j` / `k` or arrows select; `r` refreshes; `q` / Esc closes. The list refreshes
  automatically while open, retaining selection by task ID.

Schedule formats:

| Value | Behavior |
| --- | --- |
| `at 2027-01-01T09:00:00+09:00` | Once at that instant; an explicit timezone or `Z` is required. |
| `every 3600` | First run one hour after saving, then one hour after each completion. |
| `quota` | Once at the model provider's next reported **5h** quota reset. |

Quota scheduling snapshots the actual provider reset timestamp when saved. It
is **not** a repeating five-hour timer and never substitutes a weekly/unknown
window. If the provider does not expose a future 5h reset, saving fails; choose
a timestamp or interval instead. The list shows the resolved next due time.
Saving an existing paused/completed job preserves its paused state: use `p` to
enable it after editing its schedule.

## Execution and lifetime

The shared Yeet background service owns scheduling independently of the TUI.
Closing the terminal does not stop enabled tasks. Enabled jobs prevent the
lightweight daemon's ordinary idle shutdown. Four independent saved agent
sessions may run concurrently; additional due jobs wait in due-time order.
A job never overlaps itself. The worker checks due times approximately once
per second, rather than offering real-time precision.

Each job captures its prompt, canonical working folder and model. Execution
uses the existing Harness and that folder's sandbox/configuration. There is no
implicit auto-approval or unlimited-mode override. A request for permission or
a user answer fails the unattended job with an explanation. Runs time out after
one hour. Last outcome and session ID appear in Tasks; sessions use the ordinary
session store.

There is **no OS scheduler installation, reboot autostart or wake-from-sleep**.
Yeet's background service and an awake computer must be available. The Tasks
header reports when no scheduler owns the lease (for example, an embedded TUI
fallback). Reopen normal Yeet to start its background service.

After downtime, overdue jobs execute once; intervals skip missed ticks rather
than replaying a burst. Claims are persisted before dispatch. If the daemon
dies during a claim/run, the next scheduler records it as interrupted and does
not replay that occurrence, because its side effects may already have happened.
Intervals resume on a later tick. One-shot interrupted jobs need rescheduling.

## Backend contract for future platforms

`Harness/tasks.rs` owns serializable `TaskDraft`, `Schedule`, `Task`, `TaskAction`,
validation, persistence, transactional claims and execution. Data is stored in
`tasks.sqlite3` in Yeet's per-user configuration directory (`YEET_CONFIG_DIR`
is supported), globally across workspace folders. Daemons elect one scheduler
with an OS file lease; surviving daemons can take over after owner exit.

The existing Harness command/state transport exposes:

- `RequestTasks` (`type: "request_tasks"`): refresh `scheduled_tasks` and
  `tasks_scheduler_running` in `HarnessState`.
- `ManageTask { action, request_id }` (`type: "manage_task"`): save, toggle
  enabled, or delete. Every mutation echoes its correlation ID in
  `tasks_response_id` and returns success/error text in `tasks_notice`.
- `TaskAction` has a tagged `action` of `save`, `set_enabled` or `delete`.
  Save accepts `id: null` for a new job, otherwise an existing stable ID.
- `Schedule` has a tagged `kind` of `at` (RFC3339 `at`), `interval` (integer
  `seconds`) or `quota_reset`. The backend resolves quota telemetry, not a UI.

A future UI consumes these contracts without owning timers, SQLite, agent
execution or filesystem business logic. A durable non-daemon embedding host
can explicitly start `Scheduler::start()` once for its process lifetime;
ordinary embedded session runtimes do not create schedulers. Only the TUI
advertises the new composer destination today; other platform UIs are unchanged.
