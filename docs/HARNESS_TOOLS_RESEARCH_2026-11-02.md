# Yeet harness tools: research and improvement proposal

Date: November 2, 2026 (Asia/Seoul)  
Repository: /Users/kyooni18/Code/Yeet  
Inspected revision: 7466f3396d251ccc1582840ad3062d5899e5da0e  
Scope: Shell first, Edit second, then navigation, output handling, and tool orchestration.

## Recommendation

Keep Yeet's existing tool architecture and improve the contracts surrounding it. The repository already contains the difficult foundations: shell selection and direct executable invocation, foreground yielding, detached jobs, Unix PTYs, disk-backed output, snapshot-aware editing, transaction recovery, bounded tool results, and stable lazy tool discovery. Replacing those systems would discard useful work.

The first implementation phase should make three guarantees dependable:

1. A shell result clearly identifies whether a process started, remains running, or exited, and distinguishes the reason execution ended from the act of waiting for it.
2. Every shortened result says what was omitted and provides a retrieval path that is actually available through the same interface.
3. Every edit applies against the intended file state, with a truthful mutation outcome and narrowly preserved read coverage.

After those corrections, reduce model-facing complexity and evaluate improvements in total task cost. A shorter schema, a smaller result, or fewer public tools is not independently sufficient evidence of better efficiency.

This is research, not an implementation of the proposed source changes. No build, unit test, service restart, benchmark, or comparative model evaluation was run. Repository inspection used Yeet MCP, without Work mode. Two harmless interface probes were performed: an executable-only shell request that was rejected before launch, and a batched read request that was rejected by the server. The initial checkout was clean and HEAD matched origin/main. The connected server binary was not proven identical to the inspected checkout; deployment observations and source conclusions are kept separate below.

## 1. What Yeet already does well

The native tool surface starts with read_file, search_workspace, apply_file_edits, run_shell, shell_job, read_artifact, optional web tools, and ask_user. Extra tools are discovered lazily. ToolDiscovery preserves a stable attachment order and a warm tool surface across turns. This is worth retaining because schema churn can undermine prompt caching. [Y1]

Native run_shell already distinguishes yieldSeconds from timeoutSeconds. Its initial foreground window defaults to 120 seconds, after which a still-running process returns a job handle instead of being killed merely because the foreground wait expired. Explicit background execution, event-driven waits, Unix PTY input/resize/interrupt, process cleanup, and shell-specific launch behavior exist. Working-directory persistence is documented; environment variables and shell functions are not a persistent interactive-shell session. [Y2, Y3]

Shell output already has bounded capture, head/tail presentation, failure diagnostics, and a disk-backed stream log. The older keyword summary has been removed. The actor route currently adds metadata in the inspected output function; this research found no basis for proposing removal of a hypothetical extra LLM summarizer. [Y4]

Editing already supports patches, exact-text-style replacement, line edits, and named declaration operations. Rust lowers text and syntax operations to the shared backend's concrete edit representation. The backend validates whole plans, rejects duplicate targets and overlapping edits, preserves executable permission bits and BOM/selected newline style, stages writes, checks expected file digests, journals commit state, and attempts recovery/rollback. These are implemented strengths, not proposed additions. [Y8, Y9, Y10]

The engineering task is to make those capabilities consistently visible and usable.

## 2. Shell: highest-priority findings

### S1. The executable contract is inconsistent across three layers

**Source-confirmed inconsistency:** the checked-out MCP schema advertises executable plus args, but the MCP adapter dispatch at src/mcp_server.rs checks only whether command is nonempty or a job is present. An executable-only request falls into its rejection branch. Native execution has a separate direct-executable path. [Y2, Y5]

**Observed connector behavior:** the harmless executable-only probe was rejected even earlier by the connected connector schema. It required command and treated executable and args as additional properties. No process was launched. This establishes deployed schema drift, separately from the adapter bug in the checkout.

The fix should cover the definition, exported schema, dispatch, examples, and compatibility handling together. Represent the underlying request as mutually exclusive shell-program, executable-with-arguments, and existing-job variants. Derive each public schema from that contract. Native and MCP surfaces may intentionally differ, but their supported variants must be explicit.

Structured argv is especially useful for spaces, Unicode, and shell metacharacters. It should not require the model to reconstruct a quoted shell string after choosing the supposedly shell-free interface. Policy checks still apply to direct execution.

Add a schema/build fingerprint for diagnosing installed-client drift. Do not solve this by restarting services during unrelated agent work.

### S2. Foreground MCP output can lose its retrieval path

The native and MCP paths differ materially:

- The MCP registry disables artifacts.
- ShellJobs still creates an ephemeral disk-backed event log when artifacts are disabled.
- A command completing within the foreground window is consumed through take_finished, which removes both job registry handles.
- The returned FinishedShellJob retains the optional artifact transcript, not the ephemeral event log.
- The MCP result may contain only the bounded stdout/stderr window, with no usable job handle or exported read_artifact tool to recover omitted text. [Y3, Y4, Y5, Y6]

This source trace establishes a missing supported retrieval path for omitted foreground MCP output. It was not reproduced by generating a large log during this research.

Background jobs have an output action and remain accessible until forgotten; the model should not have to predict log volume in advance to get that capability. Retain a completed job/log record with a bounded lifetime and return an output reference whenever the response omits retained content. The same retrieval API should work whether a command finished quickly, yielded, or was explicitly backgrounded.

There is another adapter mismatch in the job-result formatter: its large-output branch writes an artifact without checking artifacts_enabled. Direct MCP does not export read_artifact. Make the result provide an accessible log cursor rather than an unusable recovery instruction. [Y6]

### S3. Raw terminal events are not a good default model response

shell_job output returns byte-offset events with dataBase64. This is appropriate for terminal clients preserving arbitrary bytes and terminal control sequences. It makes an LLM consume an encoding format and manage streaming decoding just to inspect a compiler error. [Y6, Y7]

Provide a model-oriented text page with stream labels, a continuation cursor, and explicit completeness. Keep the raw event protocol for terminals and programmatic consumers. Decode incrementally so a UTF-8 character split across chunks is preserved. Treat ANSI escape processing as display behavior, with the raw transcript retained.

A suspected defect was ruled out: the generic model projector drops long fields named data, but these shell events use dataBase64. The projector does not erase these events through that exact rule. The generic rule remains too broad for arbitrary external tool results, discussed later.

### S4. Capture completeness and response truncation must remain distinct

Native logs have explicit completeness and truncation metadata. The log limits are 64 MiB per log and 512 MiB for the session budget in the inspected source. In-memory command capture is separately bounded at 64 KiB per stream. These are different boundaries. [Y3, Y7]

However, the native readable formatter describes an artifact as full output without consulting artifactComplete. Once retention is incomplete, that description is stronger than the evidence warrants. [Y11]

A result should distinguish:

- The response is shortened, but omitted content is retained.
- Capture itself stopped or failed, so some output is unavailable.
- The requested page has more retained data after its cursor.
- The process is still running and may produce additional data.

Preserve the useful existing head/tail window and failure diagnostics. Add a caller-controlled output budget and searchable or pageable retained logs. Never require rerunning a costly or mutating command merely to recover output.

### S5. Separate process state, termination reason, and wait outcome

The code already distinguishes yielding from a hard deadline. Preserve that distinction in every result shape.

A useful normalized machine result has process state such as running, exited, or failed_to_start. Separate fields describe exit code, terminating signal, timeout/cancellation reason, and whether cancellation was merely requested. A wait expiring does not mean the process exited; a successful stop request does not prove cleanup completed.

Job IDs should identify the owning runtime and remain useful through model context compaction. Yeet's documented lifetime is in-process: daemon restart is not supported process reattachment. Keep that limitation explicit. Durable completion receipts and retained logs are worthwhile without first building a separate persistent process supervisor. [Y3]

The current 120-second initial yield is a candidate for tuning, not a demonstrated performance bug. Compare a 5–10-second initial yield with the current behavior. Couple short yields to event-driven waiting and completion notifications; otherwise they can increase model polling and token use. A user progress update need not require a model inference.

### S6. Simplify execution choices and make state explicit

Keep the current run_shell and shell_job names if compatibility matters. A new naming system is not necessary.

For ordinary calls, the model should primarily choose the command or executable, working directory, optional hard deadline, output budget, and whether interaction is needed. Advanced shell initialization, terminal resizing, and transport routing belong in optional capability-specific fields.

The present mode values auto/direct/actor describe output routing, while direct executable invocation describes process launching. Those are easy to confuse. Since the inspected output code no longer substitutes an actor summary, reassess whether mode needs to remain in the common model schema. Preserve compatibility internally before removing fields.

Add a per-call environment overlay so callers do not need different shell assignment syntax on Unix and PowerShell. Keep clean noninteractive initialization as the default, and preserve explicit loadConfig/login controls. Prefer explicit workingDirectory in parallel or reproducible workflows; document any persisted cwd separately from a persistent shell environment. [Y2]

Pipe stdin and explicit EOF would be useful additions for nonterminal programs. Existing input operations are PTY-oriented. Treat these as new capabilities, not corrections to an advertised Windows PTY feature.

### S7. Platform fidelity precedes feature parity claims

docs/SHELL_JOB_LIMITATIONS.md records unresolved Windows-native issues involving sandboxed Git Bash and cmd exit-status reporting. It also explicitly states that Windows PTY support is absent. These are documented limitations, not Windows failures reproduced by this investigation. [Y3]

The next implementation phase should establish current Windows-native evidence and repair confirmed exit-status failures before describing all shell backends as equivalent. Returning a false success is more consequential than lacking a convenience operation. A capability report should accurately describe PTY, stdin, signals, and available shells per platform.

## 3. Edit: preserve the engine, improve the surrounding contracts

### E1. A small successful edit currently promotes the entire file to read coverage

The backend carefully propagates previously seen ranges and adds relevant diff ranges. The Rust wrapper then calls record_post_edit_snapshots, which sets the retained coverage to the full range 1..lines for every resulting file. [Y8, Y9]

For a newly created file entirely supplied by the model, complete coverage can be reasonable. For a small edit after a partial read, it overstates what the model knows. A later edit to unrelated, previously unseen lines can acquire backend coverage when the wrapper rehydrates that expanded range.

Preserve and transform prior coverage through the concrete edit plan, then add only content the agent supplied or actually received. Do not equate knowledge of the current file digest with knowledge of its entire contents.

A related final-rendering boundary needs an explicit invariant: reads record coverage before the agent applies its final per-round output budget. A later truncation can reduce the material actually delivered. Handler-level truncation already trims coverage in one path; the final formatter/budget path should do the same or clearly model recoverable-but-not-yet-viewed content. This latter issue is a source-derived coverage concern; it was not exercised by a runtime edit probe. [Y12, Y13]

### E2. Unlimited permission should not silently mean stale-write protection is disabled

The wrapper binds cached snapshots and requires coverage only when sandbox mode is not Unlimited. src/edit.rs maps unrestricted editing to unsafe, and the backend bypasses required snapshots and seen-line enforcement in that mode. [Y8, Y9, Y14]

Authorization and edit correctness are different questions. Permission to write a path does not establish that an edit was generated against its current contents.

Retain optimistic concurrency checks under unrestricted permissions. If a caller truly wants an unconditional overwrite, make that a distinct, explicit operation. This avoids coupling an approval preference to lost-update protection.

### E3. The advertised exact match is actually whitespace-tolerant

The model-facing description says replaceText uses exact text. The implementation first attempts exact matching, then tries whole-line matching with trailing whitespace ignored, followed by indentation-insensitive matching and reindentation of the replacement. Uniqueness and overlap rejection are valuable existing safeguards. [Y2, Y15]

The fallback can reduce frustrating failures caused by copied indentation. It can also change behavior in indentation-sensitive code or files where trailing whitespace matters.

Make the policy truthful and observable. Either default to exact matching and expose a deliberately limited relaxed policy, or document the chosen limited fallback and return the match mode used. Do not silently add broader fuzzy matching. Preserve relative indentation structure when applying any indentation repair, and include Makefiles, Python, YAML, and multiline strings in evaluation.

There is also a consistency question: replaceAll is handled by the exact-match branch, while the fallback accepts only a unique match. Clarify that contract rather than making the model infer it from retries.

### E4. The workspace lock spans approval, preparation, RPC, and diagnostics

WorkspaceEditLock is acquired at the beginning of apply_file_edits, before path-scope checks, snapshot rehydration, text/syntax resolution, and backend execution. The edit RPC then waits on a blocking read_line without a response deadline or cancellation check in that loop. [Y8, Y14, Y16]

This creates a concrete mechanism for one blocked or stalled edit to hold up other sessions sharing the workspace. The lock itself is cancellable while waiting to acquire it, but that does not make the lock holder's later RPC cancellable.

Resolve permissions and perform noncritical preparation before locking. Under the lock, revalidate file identities and versions and perform the commit-critical work. Release it before optional diagnostics or presentation. Add an RPC deadline and a recovery protocol that distinguishes a rejected edit from an edit whose commit outcome is unknown.

Do not automatically retry a mutation after a broken response pipe. Yeet already avoids the generic recoverable-read retry path for apply; preserve that choice. Add an operation receipt or idempotency mechanism so an interrupted caller can inspect the prior outcome safely.

The current daemon uses a no-op diagnostics provider by default, so this research does not claim that every ordinary edit runs a build or costly language-server check. The lock and response design should nevertheless remain correct when diagnostics are configured.

### E5. Preserve transactional writes and describe their guarantee accurately

The writer validates the plan before mutating targets, stages same-directory replacements, journals progress, creates rollback backups, uses no-clobber creation, and performs deletions after staged destinations are installed. Cleanup after a committed journal is best-effort rather than rolling back an already successful edit. [Y10]

Keep this implementation direction. Do not replace it with a simple writeFile path merely to copy another harness's interface.

The public word atomic needs a precise meaning. Each pathname replacement is atomic, but a multi-file plan becomes visible through sequential filesystem operations. Other processes can observe intermediate combinations. Recovery and rollback also have limits when another writer changes a file.

Return an explicit mutation outcome: unchanged, applied, partial, or unknown, with per-file details when necessary. An error does not always justify changed=false. A diagnostics error after commit is different from a rejected transaction. The backend currently commits before awaiting its optional diagnostics provider; a failing provider can therefore produce an error after writes have succeeded. [Y9]

BOM and uniform LF/CRLF preservation are already present. Mixed newline styles are normalized to a selected predominant style, so do not promise byte-for-byte preservation of all unchanged spans. Treat unsupported encodings explicitly; the current read paths use UTF-8 decoding with replacement behavior, but this review did not demonstrate file corruption.

### E6. Reduce choice at the model boundary without fragmenting correctness

Yeet already has a common lowering path for patch, replacement, and syntax edits. Strengthen that architecture.

Offer one primary editing shape per measured model profile, with uncommon operations discoverable on demand. For many tasks, targeted old/new replacement is easy to generate; for others, a patch is more economical. Keep all adapters behind the same path checks, version checks, overlap validation, transaction writer, and outcome reporting.

Do not ship a new family of edit tools just because other products have them. First evaluate the existing patch and replacement forms using Yeet's actual supported models. The current schema also allows shapes that runtime rejects, including missing both patch and changes or supplying both. A derived discriminated schema would make invalid requests harder to generate. [Y2]

## 4. Other tools and orchestration

### Navigation and search

search_workspace already uses an ignore-aware walk, defaults to literal search, and bounds scanning. Its public schema offers query, path, maxResults, caseSensitive, and regex. It lacks file globs, context-line control, file-only/count output modes, and continuation. The implementation caps files at 10,000 and individual file size at 2 MiB, while skipped oversized/unreadable files are not separately reported. [Y12]

Add the smallest capabilities that eliminate repeated shell searches: include/exclude filters, context, paths/counts/content modes, and a stable continuation or explicit refinement strategy. Report which limits stopped the search. No matches is not the same conclusion as no matches in the scanned subset.

Existing outline and find_symbol are syntax-based for Rust/TypeScript/JavaScript, with limitations stated in their descriptions. Retain that honesty. Add true language-server resolution only when it materially improves supported workflows.

### Read clarity and efficiency

The read handler interprets endLine below startLine as a count. This is a helpfulness heuristic with ambiguous semantics. Use unambiguous bounds or an explicit lineCount.

The current refresh parameter does not change the inspected always-fresh read path. Remove or redefine it. nextStartLine currently reports the first uncovered gap across cached ranges, which can point to line 1 after reading a later range. Separate continuation-after-this-page from first-unread-range. [Y8, Y12]

Even a narrow read currently reads and normalizes the whole file, computes a full snapshot hash, and creates several representations for the selected range. A versioned snapshot and line index can avoid repeated work on large files. Measure actual CPU, latency, and memory impact before committing to a large cache subsystem; bounded output alone does not bound underlying file-read work.

### Typed output projection

The generic model projector recursively drops keys such as snapshot, route, durationMilliseconds, and long data fields. These may be metadata for a Yeet-owned result but meaningful data in a third-party tool's payload. [Y11]

Give built-in tools explicit formatters over typed results. For unknown external tools, preserve semantics and bound content through a retrieval mechanism instead of globally deleting fields by name. Keep the useful separation between detailed machine output and concise model text.

The existing aggregate budget is 12 KiB for multi-call rounds, versus a 20 KiB boundary for a single-call round. It is a reasonable attempt to control context, but it can make batching disproportionately lossy. Tune it using total retries and rereads, not merely immediate response size. [Y13]

### Concurrency and discovery

Native read_file batches currently loop through requests sequentially. General parallel dispatch applies when an entire batch consists of MCP tools marked read-only. [Y12, Y17]

Extend bounded parallelism to independent native reads/searches after dependency analysis. Read-only is insufficient by itself: reads racing edits can see different revisions and update shared coverage. Execute independent I/O concurrently, then merge evidence deterministically against explicit snapshot versions. Shell calls with shared cwd and same-file mutations remain ordered.

Preserve stable schema ordering and lazy discovery. Consider reducing unnecessary search_tools → find_capabilities → activate_capability hops, but keep exact capability identity and permission requirements explicit. This is lower priority than Shell and Edit correctness. [Y1, Y2]

## 5. What to borrow from other advanced harnesses

These are current primary-source observations, not a ranking or measured performance comparison. Documentation was inspected November 2, 2026. GitHub main links below are mutable; no immutable external commit was obtained.

| Harness | Narrow verified lesson | Implication for Yeet |
| --- | --- | --- |
| Codex | Its current unified-exec handler separates yield time, optional timeout, output-token budget, and terminal session identity. | Preserve those separate dimensions in one coherent shell result contract. |
| Claude Code | Its public reference explicitly documents cwd/environment persistence and exact unique-string editing, including conditional handling of changed files. | Publish precise contracts and make recoverable stale edits deliberate. |
| OpenCode v2 | Background shell completion notifications and managed full-output files are documented; patch versus edit/write exposure varies by supported model. | Retain accessible output and evaluate model-specific edit presentation. |
| Pi | The output accumulator keeps a bounded decoded tail with file-backed output; Edit batches matches against original content and serializes same-file mutation. | Separate model excerpts from byte logs; keep consistent batch semantics. |
| Aider | Its edit-format documentation explains that format suitability depends on the model and that full-file output retransmits unchanged content. | Select presentation formats through evaluation, not preference. |

Do not assume that a competitor implements every proposed guarantee. For example, Pi's ordinary write path is not evidence of a crash-safe multi-file transaction. Yeet already has a stronger transaction architecture in that respect. Likewise, an output accumulator's bounded decoded tail is not a proof of an absolute process-RSS bound; stream buffering is a separate consideration.

OpenAI's agent-loop discussion also explains why stable tool ordering matters for prompt caching. Anthropic's tool-design guidance recommends evaluating realistic multi-step tasks using success, calls, tokens, runtime, and errors. Those principles align with Yeet's existing cache and actual-agent benchmark infrastructure. [X1–X8]

## 6. Proposed contracts

These are design sketches, not claims about arguments currently accepted by the connected tool.

A common Shell request should distinguish command text from executable/argv, with optional cwd, environment overlay, yield budget, hard deadline, output budget, and PTY selection. Preserve existing public names where possible.

A representative result is:

    {
      "state": "running",
      "jobId": "job_42",
      "workingDirectory": "/project",
      "output": "Compiling...",
      "nextCursor": "cursor_17",
      "responseTruncated": false,
      "captureComplete": true
    }

After termination, state becomes exited with exitCode, optional signal, and terminationReason. Failed startup has a distinct state. A cancelled wait leaves process state unchanged; stop can acknowledge cancelRequested while cleanup is pending.

Every short output result must either point to readable retained content or explicitly state that capture is incomplete. Completion and log retention should not depend on whether the original call happened to finish before yielding.

An Edit conflict should include an error code, operation index, affected path, mutationOutcome, and the narrow reread range required for recovery. A successful edit should include a concise change summary, an optional diff reference, a post-edit revision, and a separate diagnostics status. Use unchanged only when no mutation occurred or rollback was confirmed. Use unknown when a transport failure makes the commit outcome uncertain.

## 7. Implementation order

| Phase | Work | Acceptance condition |
| --- | --- | --- |
| 1: contracts and correctness | Fix executable MCP dispatch/schema drift; align read descriptions; retain foreground MCP logs; truthful completeness; repair coverage promotion and permission/correctness coupling. | Supported calls behave consistently; omitted output has a working retrieval path; stale or unseen edits are not silently accepted. |
| 2: model ergonomics | Decoded shell pages, precise outcomes, concise typed errors, clear read bounds, matching-policy metadata, simpler common schemas. | The agent can recover without guessing a tool shape, rerunning a command, or blindly retrying a mutation. |
| 3: concurrency and platform work | Narrow edit critical sections, cancellable RPC with receipts, independent native I/O scheduling, current Windows-native verification. | No indefinite shared-workspace blockage; deterministic mutation outcomes; supported shells report real failures correctly. |
| 4: measured tuning | Compare yield defaults, output budgets, edit profiles, and read indexing. | Equal or better verified task success with lower total latency/token/recovery overhead. |

Phase 1 can be split into small reviewable changes. Do not bundle a process-supervisor rewrite, a new UI, a new general scripting runtime, and a new editor into the same effort.

## 8. Evaluation plan

Use a compact, representative tool-contract suite before a large end-to-end campaign. Meaningful cases include:

| Group | Cases | Evidence to retain |
| --- | --- | --- |
| Shell execution | Quick success, nonzero exit, missing executable, literal argv metacharacters, Unicode cwd, per-call environment | Start/result state, exit code, actual argv/cwd, unintended interpretation |
| Shell lifetime | Foreground yield, explicit background, wait cancellation, interrupt, hard timeout, descendant holding a pipe | Process cleanup, no duplicate launches, wait versus execution outcome |
| Output | Large stdout/stderr, useful error in the middle, split UTF-8, truncated retention, completed foreground MCP retrieval | Exact retained evidence, valid cursor, readable model page, capture completeness |
| Editing | Disjoint edits, duplicate/overlapping matches, concurrent same-file changes, small edit after partial read, stale edit under unrestricted permissions | Intended diff, untouched content, coverage, mutation outcome |
| Edit recovery | Failure during commit, response loss after commit, rollback failure, diagnostics failure | Per-file state, operation receipt, no blind duplicate mutation |
| File details | LF/CRLF/BOM, mixed newlines, executable mode, unsupported encoding, symlink policy | Precisely documented preservation and rejection behavior |
| Navigation | Find a file, locate relevant code, read a narrow range, edit it | Calls, tokens, unnecessary full reads, correct file and revision |
| End-to-end | Small bug fix, targeted refactor, failed-test diagnosis, interrupted-task continuation | Task verifier result and total tool-related recovery cost |

Hold model/version, reasoning budget, repository snapshot, task set, prompt, environment, retry policy, and concurrency constant for A/B comparisons. Repeat stochastic cases enough to avoid treating one lucky run as a regression or improvement.

Measure verified completion rate, unintended modifications, malformed calls, retries, rereads, duplicate output, model input/output tokens, cached input, wall-clock time, first-progress latency, and process/log memory and disk use. Keep tool-only contract results separate from model-dependent task results.

Yeet already has a Terminal-Bench/Harbor adapter that invokes its actual AgentCoordinator through yeet agent, while yeet run is a raw model-call path. Reuse that infrastructure for the later end-to-end evaluation rather than introducing another benchmark harness. Nothing in that campaign was executed for this research. [Y18]

## 9. Evidence and limitations

Source conclusions are static traces at the pinned Yeet revision. The two live probes establish only the connected interface behaviors described above. No Windows execution, crash injection, large-output reproduction, or comparative benchmark was performed. Performance improvements are proposed mechanisms, not measured percentages.

The external comparison was reviewed separately from the direct repository inspection. Neither constitutes a measured head-to-head performance result.

The generic data-field projector rule was initially suspected of deleting Shell events; the complete trace disproved that specific hypothesis because Shell uses dataBase64. No such Shell data-loss claim is made here.

### Yeet source index

All paths below are relative to /Users/kyooni18/Code/Yeet. Links pin the inspected revision.

- Y1: [Harness/agent/tool_discovery.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/agent/tool_discovery.rs), especially basic surface, attachment order, warm restoration, and search.
- Y2: [Harness/tools/definitions.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/definitions.rs), native Shell/Edit/read definitions and direct MCP definitions.
- Y3: [Harness/tools/shell_runtime.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/shell_runtime.rs) and [docs/SHELL_JOB_LIMITATIONS.md](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/docs/SHELL_JOB_LIMITATIONS.md), launch, yield, collection, cwd, and lifetime.
- Y4: [Harness/tools/shell_runtime.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/shell_runtime.rs#L495), shell_output_result and output windows.
- Y5: [src/mcp_server.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/src/mcp_server.rs), artifact disablement near 380, read restriction near 636, shell dispatch near 644.
- Y6: [Harness/tools/shell_jobs.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/shell_jobs.rs), ephemeral log creation, take_finished, output, and large job results.
- Y7: [src/shell/log.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/src/shell/log.rs), capture limits, event dataBase64, text pages, completeness.
- Y8: [Harness/tools/editing.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/editing.rs), lock scope, snapshot binding, rehydration, post-edit coverage.
- Y9: [RuntimeSource/src/edit-backend/backend.ts](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/RuntimeSource/src/edit-backend/backend.ts), preparation, seen-line validation, rebase, commit, diagnostics, coverage propagation.
- Y10: [RuntimeSource/src/edit-backend/transaction.ts](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/RuntimeSource/src/edit-backend/transaction.ts) and [text.ts](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/RuntimeSource/src/edit-backend/text.ts), transaction and text envelope.
- Y11: [Harness/agent/tool_protocol.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/agent/tool_protocol.rs), recursive projection and readable results.
- Y12: [Harness/tools/io.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/io.rs), reads, search, limits, batching, and coverage recording.
- Y13: [Harness/agent.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/agent.rs#L507), [Harness/tools/token_efficiency.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/token_efficiency.rs), and [artifact_output.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/artifact_output.rs), final model-output boundary.
- Y14: [src/edit.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/src/edit.rs), apply, unsafe flag, RPC, and read-only retry behavior.
- Y15: [Harness/tools/text_edit.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/text_edit.rs), replacement, whitespace fallback, overlap handling, and patch lowering.
- Y16: [Harness/tools/edit_lock.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools/edit_lock.rs), workspace-scoped cross-process lock.
- Y17: [Harness/tools.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/tools.rs#L361) and [Harness/agent/tool_round.rs](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/Harness/agent/tool_round.rs), parallel MCP dispatch.
- Y18: [benchmarks/terminalbench/README.md](https://github.com/kyooni18/Yeet/blob/7466f3396d251ccc1582840ad3062d5899e5da0e/benchmarks/terminalbench/README.md), actual-agent benchmark entry point.

### External primary sources

- X1: [Codex unified-exec handler](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/unified_exec.rs).
- X2: [OpenAI: Unrolling the Codex agent loop](https://openai.com/index/unrolling-the-codex-agent-loop/), January 23, 2026.
- X3: [Claude Code tools reference](https://code.claude.com/docs/en/tools-reference), especially Bash and Edit behavior.
- X4: [OpenCode v2 tools](https://opencode.ai/v2/docs/tools/). This report uses v2 behavior explicitly.
- X5: [Pi output accumulator](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/output-accumulator.ts).
- X6: [Pi Edit implementation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/edit.ts).
- X7: [Aider edit formats](https://aider.chat/docs/more/edit-formats.html).
- X8: [Anthropic: Writing effective tools for agents](https://www.anthropic.com/research/writing-tools-for-agents), September 11, 2025.
