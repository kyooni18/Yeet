# Harness tool contracts

These contracts implement the first correctness phase and selected ergonomics from
[the November 2, 2026 research](HARNESS_TOOLS_RESEARCH_2026-11-02.md). They preserve
the existing shell runtime, transactional edit backend, public tool names, and
stable tool ordering.

## Shell requests and retained results

Native `run_shell` accepts exactly one of `command` or `executable`. Direct
executable arguments are literal strings and do not pass through a shell.
`args` requires `executable`; shell initialization options belong to command
execution. The MCP adapter additionally accepts a `job` object and rejects
requests that mix job operations with execution options, even when sent without
client-side schema validation. MCP derives its execution and job properties
from the same definitions as the native tools.

```json
{
  "executable": "/usr/bin/printf",
  "args": ["%s\n", "spaces, 한글, and $(literal text)"],
  "workingDirectory": "/path/to/project"
}
```

`yieldSeconds` limits the foreground wait; its default remains 120 seconds.
`timeoutSeconds` is an execution deadline. A yielded process keeps running under
its returned `jobId`. A wait ending or a stop request being accepted does not
prove the process exited. Successful terminal results report `state: exited`;
running results report `state: running`. Existing untyped worker failures remain
explicitly `unknown`, with `workerFinished`, rather than being mislabeled as
startup failures.

Quick foreground commands retain their job record and disk log just like
background commands. Completed records are retained for one hour, with a limit
of 64 completed records per owning runtime. Cleanup is lazy during subsequent
tool activity. Running jobs are not evicted by completed-record cleanup, and the
separate running-job limit remains 32. Explicit `forget`, runtime/session reset,
or daemon termination revokes the handle. This is not process reattachment after
a daemon restart.

For MCP, retrieve retained output through the same tool:

```json
{
  "job": {
    "action": "output",
    "jobId": "<returned jobId>",
    "offset": 0,
    "maxBytes": 8192
  }
}
```

Native callers use `shell_job` with the inner object directly. Output defaults
to `format: text` and returns `chunks: [{stream, content}]`, `nextOffset`, and
`hasMore`. Continue with `nextOffset`; offsets count retained raw bytes. UTF-8
decoding is independent for stdout, stderr, and PTY output, including characters
split across pages or interleaved stream events. Invalid byte sequences are
displayed with replacement characters. `format: events` retains the original
base64 event protocol for byte-exact consumers. The Remote terminal's lower-level
API remains an event API.

| Field | Meaning |
| --- | --- |
| `responseTruncated` | The returned command summary was shortened. A requested output page has its own `hasMore` continuation. |
| `captureComplete` | No captured bytes have been lost so far; it can be true while the process is still running. |
| `captureTruncated`, `captureError` | Capture hit a retention limit or failed; rereading cannot restore missing bytes. |
| `hasMore` | More retained bytes exist after this page's cursor. |
| `finished` / process state | Whether execution/log production has finished; independent of capture integrity. |
| `artifactComplete` | Legacy artifact metadata indicating a finalized, complete transcript. |

The existing 64 MiB per-log and 512 MiB session log budgets still apply. A
response excerpt is not a guarantee that the underlying capture is complete.
MCP does not export `read_artifact`; its output hints point to `run_shell job`.

## Reads and model-visible evidence

`startLine` and `endLine` are inclusive absolute line numbers. An alternative
`lineCount` explicitly requests a count from `startLine`; it cannot be combined
with `endLine`. Reversed bounds and non-positive/non-integer bounds are rejected
instead of being reinterpreted as a count.

```json
{"path":"src/lib.rs","startLine":120,"lineCount":40}
```

`nextStartLine` continues after the returned page. `firstUnreadLine` separately
identifies the earliest gap in retained read coverage. Reading a later range no
longer turns page continuation into a suggestion to restart at line 1. Reads
always use current file contents; `refresh` remains a compatibility option.
Native reads support up to eight requests; direct MCP deliberately supports one
file per call and says so in its description.

Coverage represents complete source lines actually delivered to the caller, not
bytes fetched internally. A partial trailing line or a synthetic truncation
marker does not grant coverage. If the final model-output budget externalizes a
native read, the affected files' retained read evidence is conservatively
revoked; a narrow source reread is required before editing. Other paths' evidence
is preserved. Reading an artifact is not silently treated as a fresh source
snapshot.

## Edit correctness and outcomes

Use exactly one of `patch` or `changes`. Existing files require a retained
snapshot and coverage of the edit target even under Unlimited permissions.
Filesystem authorization and edit concurrency are separate: unrestricted path
permission no longer disables stale-write protection. Creates do not require an
existing-file read.

Successful edits return a normalized content `revision`, a public revision
snapshot, and authoritative `seenRanges`. Coverage transforms previous ranges
through the concrete edit plan and adds supplied replacement text. A small edit
does not make the rest of a file read. Created files supplied in full have full
coverage, even when their displayed diff is shortened. The wrapper verifies the
post-edit digest before caching these ranges and constrains daemon coverage
when rehydrating a snapshot.

`replaceText` defaults to exact matching. Explicit `matchPolicy: relaxed` allows
only unique whole-line trailing-whitespace or uniform-indentation recovery.
Relative nesting must remain consistent. `replaceAll` operates on exact
matches; it does not broaden a relaxed fallback. Results report actual
`matchModes` per replacement: `exact`, `trailing_whitespace`, or `indentation`.

| Field | Contract |
| --- | --- |
| `mutationOutcome: unchanged` | Rejected before mutation, no mutation requested, or rollback was confirmed. |
| `mutationOutcome: applied` | The write committed, including when a later diagnostic or presentation step failed. |
| `mutationOutcome: unknown` | Transport, commit, or rollback evidence cannot establish the final state; inspect files before retrying. |
| `diagnosticsStatus: skipped` | Disabled or no diagnostics provider configured. |
| `diagnosticsStatus: passed` | A configured provider completed without error diagnostics. |
| `diagnosticsStatus: failed` | Error diagnostics were returned or the provider failed; `diagnosticsError` records provider failure. |

Errors retain a machine-readable `code` and `mutationOutcome` on both native
and MCP paths. Mutating RPCs are not automatically retried after a broken
response. Large edit responses keep their small outcome/status fields visible
while retaining the exact result in an artifact.

Permission checks and noncritical preparation occur before the shared workspace
lock. Commit-time identity/digest checks remain in the backend. Same-directory
pathname replacement, staged writes, journals, and rollback are preserved.
Multi-file visibility remains sequential rather than one globally atomic
filesystem operation. BOM and supported newline handling retain their existing
behavior.

Missing-path resolution now joins the missing suffix to the canonical existing
ancestor. A file created through a Mac `/var` or workspace alias therefore has
the same identity when it is read or edited later. Leaf-symlink restrictions
remain in force.

## Output formatting and installed-schema diagnostics

Built-in result formatting uses the tool identity. Unknown external tool data
is not recursively stripped merely because it contains keys such as `data`,
`snapshot`, `route`, or `durationMilliseconds`. Output budgets still apply, with
retained artifacts for recovery. Shell receipts and edit outcomes precede large
content so truncation preserves the information needed for recovery.

MCP initialization, discovery, and tool-list metadata include
`com.yeet/harness.buildRevision` and `schemaFingerprint`. The build revision is a
diagnostic Git revision with a dirty suffix when appropriate (or `unknown` for
source archives). The fingerprint is SHA-256 over the exported native tool
catalog, excluding independently changing attached MCP servers. A source change
does not itself refresh an already running binary or a client's cached schema.

## Verification and remaining work

The focused regression suite covers executable argv fidelity, invalid mixed
requests, completed foreground MCP log retrieval, raw-event compatibility,
split UTF-8, capture loss, retention expiry, read bounds, narrow coverage,
Unlimited-mode stale writes, explicit matching policies, canonical create
identity, rollback outcomes, diagnostics failures, and result formatting.
Relevant Rust filters are `mcp_server::contract_tests`, `shell::log::tests`,
`shell_jobs::tests`, `process_execution_tests`, `tools::io::tests`,
`text_edit::contract_tests`, `editing::contract_tests`, `tool_protocol::tests`,
`token_efficiency`, and `artifact_output`. The backend contract suite is
`RuntimeSource/test/edit-backend.test.mjs`; build RuntimeSource before running
Rust tests that launch its edit daemon.

Verified on the Mac on November 2, 2026:

- `npm --prefix RuntimeSource run build`: passed.
- `node --test RuntimeSource/test/edit-backend.test.mjs`: 15 passed.
- `cargo test --offline --lib --no-run -j 2`: compiled successfully.
- The Rust test binary with the focused filters above: 62 passed, 0 failed.
- Changed Rust files were formatted, and `git diff --check` passed.

These are focused contract checks, not the full repository test suite or a
comparative agent-performance benchmark. The installed daemon was not restarted
to perform them.

The following research items remain separate work: RPC deadlines/cancellation
receipts/idempotency, moving backend diagnostics outside the lock, environment
overlays and pipe stdin, caller-controlled foreground output budgets, complete
typed startup/signal/timeout outcomes, broader search and native I/O concurrency,
Windows-native verification, and measured model/yield/output-budget tuning.
No comparative token, latency, or task-success improvement is claimed from this
correctness patch.
