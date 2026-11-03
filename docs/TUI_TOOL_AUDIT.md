# TUI and tool audit

## Validated changes

- The focused composer now accepts Ctrl+V through the same system-clipboard
  request path as the context menu. Bracketed paste and forwarded Cmd+V are
  unchanged. Existing editing coverage also checks the paste request is consumed
  exactly once without changing the draft prematurely.
- Default light (Adwaita) accent, warning, success, and user foregrounds are
  darker. One contrast regression test requires 4.5:1 for normal semantic text
  roles on the default content surfaces, and user text on its tinted surface.
  Dark themes, custom themes, and frontend layouts are unchanged.
- Nested instruction discovery previously loaded only the deepest instruction
  directory. It now includes all applicable ancestor directories below the root,
  parent-first, retaining duplicate suppression. The existing test checks this
  hierarchy, root exclusion, and lexical workspace escape rejection.

## Verification

`cargo test --lib`: 491 passed after these changes (490 passed before changes).
This exercises real tool registry reads/edits, externalized read continuation,
concurrent edit sessions, shell cwd persistence, long-command handoff, partial
output, retained job output, PTY input/resize/interruption, and MCP contracts.
It also covers transcript selection/context menus, original Markdown copy,
Unicode composer editing, root/global instruction loading, and nested overlays.

`cargo test --bin yeet` also builds successfully (no binary-local tests).
`cargo test --test source_layout`: five passed, one failed because
`Harness/tools/shell_jobs.rs` is 1,241 lines against a 1,200-line target. This
failure predates the audit: that file is unchanged from starting commit
`a08073a`. It needs responsibility extraction, not an increased limit.

These results are not a claim of production reliability across all platforms:
no visual evaluation in the user's terminal, live provider evaluation, or
Windows/Linux desktop clipboard round trip was performed. Tool availability
search did not expose Foundation Memory in this session, so durable findings
are preserved here and in Git instead.

## Remaining issues, in priority order

1. **Nested instructions arrive after the tool round.** Root/global instructions
   are injected during turn setup, but deeper instructions are discovered by
   `finish_tool_round`. An agent that edits an unfamiliar subtree before reading
   it can mutate it before receiving its rules. Agents should read relevant
   instruction files before editing; the runtime should eventually preflight
   scoped instructions before mutation. Shell commands and workspace searches
   do not currently trigger nested discovery. Instruction files are model
   guidance, not enforced filesystem permissions.
2. **Selection is viewport-relative.** Dragging clamps to the visible transcript;
   it does not provide document-style selection across offscreen history.
   Use right-click Copy message for a complete original response. More complete
   keyboard/composer selection and drag autoscroll need dedicated UX work.
3. **Clipboard capability is terminal-dependent.** macOS copy tries pbcopy and
   falls back to OSC52; other platforms currently copy through OSC52. Terminals
   can reject OSC52 without acknowledging failure. Paste uses pbpaste,
   PowerShell, wl-paste, or xclip. Missing helpers and remote-local clipboard
   differences remain. Clipboard subprocess I/O is synchronous on the UI loop.
   Terminal-native paste remains the most portable fallback.
4. **Non-default light/custom palettes are not contrast-guaranteed.** The new
   regression protects the default Adwaita roles, not every theme, syntax color,
   border decoration, or foreground/background combination. A terminal visual
   pass is still warranted.

Related tool lifecycle contracts and their deliberate limits are documented in
`HARNESS_TOOL_CONTRACTS.md`; restart reattachment is not guaranteed.
