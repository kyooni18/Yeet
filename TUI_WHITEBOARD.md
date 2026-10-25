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
