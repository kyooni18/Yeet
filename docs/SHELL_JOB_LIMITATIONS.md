# Shell job lifetime and current limitations

Shell jobs are owned by the session's in-process tool registry. A foreground
yield is not an execution deadline: it returns a job ID for later inspection.
An explicit `timeoutSeconds` is a deadline; sandbox wall-time limits continue to
apply even without one. Cancelling the foreground command or stopping a job
requests termination. Cancelling a `shell_job wait` only cancels the wait.
In the local session-switching frontend, an active shell job prevents replacing
the session; stop it explicitly first. Rebinding the registry to another session
revokes completed job handles and clears the tracked shell working directory.

This is not a persistent process supervisor. Dropping the registry requests
job termination, and a complete daemon restart cannot reconnect to these jobs.
Do not interpret an old job ID as evidence that a process is still running.
An HTTP MCP session remains available past the normal idle expiry while one of
its shell jobs is running; reconnect with the same MCP session ID. Explicitly
closing the session still requests job termination. Once the owning runtime is
gone, an old session or job ID is rejected rather than reported as running.
Abrupt process death and descendants that escape process groups or are
reparented can defeat cleanup; process-tree discovery is inherently racy.

Pipe execution remains the default. Explicit `pty: true` enables a Unix PTY;
`shell_job` supports `input`, `resize`, and `interrupt` (terminal Ctrl-C).
PTY output is merged and reported with stream `pty`. On macOS, sandboxed
PTY jobs receive ioctl permission restricted to their own slave device so
terminal settings and resize work without granting access to other terminals.
Windows PTY is not yet implemented: a PTY request fails before launch, including
when sandboxing is required. Windows-only regression tests cover this fail-closed
behavior and cmd, PowerShell and (when installed) Git Bash quoting, Unicode,
cwd and exit-status handling, executable resolution, workspace-boundary path
handling, and process-tree cleanup. They are compiled locally with
`cargo check --tests --target x86_64-pc-windows-gnu` and executed natively by
the Windows x64 and ARM64 CI runners.

Native Windows CI has exposed unresolved correctness issues: Git Bash's MSYS
runtime fails to create its named objects inside the AppContainer on the hosted
runner, and the cmd cwd-report wrapper can lose a nonzero exit status. Do not
rely on sandboxed Git Bash or cmd exit-status reporting until these are fixed
and verified natively. Windows-native CI is not yet green.
The compatibility Web client has an experimental interactive terminal for jobs
exposed in expanded tool details; its Remote request/response path has mock-client
coverage and a real-server stale-job test. Real-server reconnection and end-to-end
terminal behavior are not yet fully verified. Web terminal input is serialized;
a disconnect discards unacknowledged and queued input because delivery of the
in-flight write cannot be determined safely. The user must re-enter it.
The TUI does not yet provide an interactive terminal.
The bounded disk-backed
transcript offers byte-offset, base64-encoded stream events through
`shell_job output`. Clients must incrementally decode
each stream. Interleaving is reader-observation order, not a precise ordering of
writes across stdout and stderr. It is not a terminal emulator.

On Unix, shell cancellation and deadline expiry attempt SIGINT, then SIGTERM,
then SIGKILL. `YEET_SHELL_SIGNAL_GRACE_MS` controls each grace interval (default
500 ms, capped at 5000 ms) on both platforms. Windows first attempts a console
break and a non-forced `taskkill` tree request, then escalates to
`TerminateJobObject`. Every Windows child process is assigned to a Job Object
with kill-on-close, so descendants cannot escape forced cleanup and orphans are
reaped when the owning handle closes, including after the leader exits or the
Yeet process dies. When job assignment is refused by the OS, the previous
`taskkill /T /F` tree fallback remains.
