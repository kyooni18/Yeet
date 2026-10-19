# TUI input latency: investigation report (2026-10-19)

Status: diagnosis only. Nothing changed, committed or restarted.

## 1. Root cause

The TUI main thread is saturated by **quadratic Home-projection work that runs on every keystroke and every backend state snapshot**, even when the Home screen is not visible.

- `HomeState::view` dedups the "recent" list with a linear `any()` scan inside a loop. This is O(N²) in N = sessions + recent views + tasks + **changed files** ([UI/home/mod.rs:96-106](../../UI/home/mod.rs)).
- Every equality check on a `ResourceTarget::Diff(PathBuf)` goes through `Path == Path`. That walks `Components` from the back (`std::path::Components::next_back` → `parse_next_component_back`), so each comparison is expensive.
- In the AIS workspace, N is large: 181 saved sessions and 264 git entries (1,796 with `-uall`).
- `HomeState::project` (which calls `view`) runs **2× per keystroke** and **3× per backend snapshot**:
  - Keystroke: `handle_key` → `commit_composer_without_projection` → `advance()` → `fingerprint()` (one `home.project`) + `advance_with_fingerprint` (a second `home.project`) ([UI/application_session.rs:585-594, 815-830, 831-848](../../UI/application_session.rs)).
  - Snapshot: `merge_state` → `refresh_with_conversation_entries` → `fingerprint()` + `advance_with_fingerprint()` + `projection()` (3 × `home.project`) ([Platforms/tui/app.rs:358-360](../../Platforms/tui/app.rs), [UI/application_session.rs:789-805, 340-356](../../UI/application_session.rs)).

### Why keys appear in batches

The event loop is single-threaded: read keys → draw → apply backend events ([Platforms/tui/runtime.rs:178-283](../../Platforms/tui/runtime.rs)). The crossterm reader thread is idle and fast, so keys queue promptly in the mpsc channel. But while the main thread spends tens to hundreds of ms in `merge_state`/`handle_key` projections, nothing drains the channel. Each queued key also pays its own 2× projection before the frame. When the loop gets back to input it drains up to 64 queued keys, processes them all, and paints **once**, so they appear together. During streaming, snapshots keep arriving (the daemon was at 42% CPU on `sendto`/`recvfrom`), so the starvation repeats.

## 2. Evidence

**Running binaries.** The TUI is pid 71719 (`yeet`, ttys003) and the AIS daemon is pid 71720 (`yeet __background-daemon /Users/kyooni18/Code/AIS`). `~/.local/bin/yeet` has the same SHA-1 (`ba11fe4b…`) and the same UUID (`EFEB8B9B…`) as `target/release/yeet`, built 12:22, after 3a4be2e. So both fixes are in the running process.

**CPU.** The TUI was at 95% and all of it on the main thread (`ps -M`: UTIME 3:00 on the main thread, ~0 on the others). The crossterm reader thread was blocked in `kevent` and is not spinning. The daemon was at 42%, almost all in socket send/recv.

**`sample 71719 5`.** I symbolized it by rebuilding the identical tree with `strip=none, debug=line-tables-only` and byte-pattern-matching the addresses:

| samples (of 3684 main-thread) | frame |
|---|---|
| 3450 (94%) | under `ApplicationSession::refresh_with_conversation_entries` |
| 1159 / 1141 / 1083 | its three call sites: lines 799 (`fingerprint`), 801 (`advance_with_fingerprint`), 802 (`projection`). Each one is a `home.project` |
| → | `HomeState::project` (mod.rs:156) → `HomeState::view` (mod.rs:102) |
| ~2400 leaf | `std::path::Components::next_back` / `parse_next_component_back` |
| 162 (4%) | rendering/draw path |

So rendering accounts for about 4% of main-thread time and Home projection for about 90%.

**Micro-benchmark** of the same dedup pattern (`rustc -O`, Diff paths and session ids shaped like the real ones):

| N | per `project` | per keystroke (2×) | per snapshot (3×) |
|---|---|---|---|
| 465 (tracked status) | 1.3 ms | 2.6 ms | 3.9 ms |
| 1997 (`-uall`) | 50 ms | 100 ms | 150 ms |

This counts only the dedup. The real calls also serialize `fingerprint()` via `serde_json::to_value`, which includes the whole `HomeView` and `diff_views`.

## 3. Why the earlier fixes did not help

- 4a43d6e (redraw pacing) and 3a4be2e (keystroke priority, avoiding transcript clones) targeted **rendering** and **transcript cloning**. The profile shows draw at about 4%, so those were not the bottleneck.
- The 300-tool-call synthetic benchmark grows the *conversation*. It has an empty Home (no sessions, no git changes), so N≈0 and the quadratic term never shows up. Its 30→7 ms gain was real for transcripts but unrelated to this workload.
- "Prioritize keystrokes" only reorders work inside a loop iteration. `handle_key` itself does 2× `home.project`, and the snapshot that was already being applied cannot be preempted.

## 4. Minimal correction plan

In order. Each step stands alone.

1. **Make the dedup O(N).** In `HomeState::view`, track seen targets in a `HashSet<&ResourceTarget>` (`ResourceTarget` already derives `Hash`). One-line algorithmic fix. Hashing a `PathBuf` hashes its components once per item instead of N times.
2. **Stop projecting Home on unrelated updates.** Cache `HomeView` in `ApplicationSession`, recompute it only when `home.content` or `home.selected` changes (`update_home_content`, `commit_home`, navigation selection), and reuse the cached value in `fingerprint()`, `advance_with_fingerprint()` and `projection()`. Composer keystrokes and streaming snapshots never touch Home content.
3. (Follow-up) Replace the per-keystroke `fingerprint()` (a `serde_json::Value` of every sub-view) with per-subsession revisions, which `conversation` already uses. Also consider the daemon sending deltas or revision-only snapshots while streaming. That is secondary: the daemon side is 42% of one core in another process and does not block the TUI thread.

## 5. Reproduction and regression test

- **Live repro:** run the TUI in a workspace with many saved sessions and many changed or untracked files (AIS today). Start a streaming turn and type. Compare `sample <pid> 5` before and after: the `HomeState::view` / `Components::next_back` frames should disappear.
- **Controlled repro (isolated):** point a throwaway `HOME`/`~/.yeet` at a temp dir containing a git repo with ~2,000 untracked files and ~200 fake sessions. Drive the TUI under a PTY (e.g. `expect`/`script`), timestamp written keys against the first frame containing them, and measure with and without a fake streaming backend.
- **Regression test (one, meaningful):** a unit test in `UI/home` or `application_session` that builds `WorkspaceContent` with about 5,000 diff items and about 500 sessions. It should assert that `commit_composer_without_projection` stays well under a generous bound (e.g. 20 ms), or more robustly that `home.project` is not invoked on a composer commit (a counter or cached-revision check). Prefer the structural assertion so the test is not timing-flaky.

## 6. Expected impact and remaining uncertainty

- Step 1 alone takes per-projection cost from about 50 ms to well under 1 ms at N≈2,000. Keystroke handling should drop to low single-digit ms and snapshot application should stop starving input.
- Step 2 removes the work from the keystroke and streaming paths entirely.
- Uncertainties:
  - I did not dump the exact live N, because the daemon's git snapshot settings decide whether untracked files are expanded. It sits between 465 and about 2,000.
  - I did not instrument per-event queue-wait times in the live process, because that needs a rebuilt binary and the brief forbids replacing processes. The batching explanation follows from the 100% main-thread profile plus the loop structure.
  - The Web Remote process was mostly idle while I sampled (0.1%), so I could not compare Remote connected vs disconnected without disrupting services.
