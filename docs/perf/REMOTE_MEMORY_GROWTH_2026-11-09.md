# Remote session memory-growth investigation — 2026-11-09

Status: **reproducible defects identified; no runtime fix applied**.
Code inspected: `d91e4e0`. The affected backend files are unchanged from
`a8bbb7b`, the checkout preceding the incident's UI work.

## Findings

A settings refresh can block the runtime's event consumer for the entire
remaining agent run. Meanwhile, streamed tool-call arguments produce full
session snapshots into an unbounded channel. Every pending snapshot owns a
separate transcript copy. This is unbounded retention, not proof of a classic
unreachable-allocation leak.

The following chain is present in the real code:

1. `frontend/shared/state/remoteStore.ts`: `init()` calls `requestRemoteState()`
   on every transport `onOpen`, including reconnects. The bootstrap command
   batch includes `request_settings`. Opening/refreshing Settings also requests it.
2. `Harness/service/run.rs`: `RunWorker::drive()` holds the coordinator mutex
   across `coordinator.run()`, including provider streaming and tool execution.
3. `Harness/service/settings.rs`: `request_settings()` takes that same mutex
   synchronously to reconfigure Web search, even though the command is a read
   refresh. It waits for the running agent to release the mutex.
4. `src/background/runtime_process.rs`: the runtime worker invokes
   `service.send(command)` before draining service events. A blocked settings
   command therefore prevents this runtime's event drain and later commands,
   including an interrupt queued behind it.
5. `Harness/service/run.rs`: `project_event()` omits the conversation for most
   text/reasoning deltas, but **not `ToolCallDelta`**. Each argument fragment
   updates the transcript and publishes `state_envelope(&state.state)`.
6. `Harness/service/transport.rs`: `state_envelope()` deep-clones the full state.
   `Harness/service.rs` creates the service-event channel with `mpsc::channel()`;
   `Harness/service/sync.rs::EventSender` sends every snapshot without a bound
   or producer-side coalescing. This remains possible while settings waits:
   the running producer does not need to reacquire the coordinator mutex.

There is also an unbounded runtime-worker-to-daemon event channel in
`src/background/runtime_process.rs`. The daemon coalesces updates in
`src/background.rs`, but only **after** they have passed through these queues.
Its bounded socket writers cannot protect memory already retained upstream.

## Controlled reproductions

Two temporary diagnostic tests were placed in `Harness/service/sync.rs`, run
against the actual private service/projection/queue types, then removed. No
live provider call or developer daemon was needed. The service test used a
fresh temporary workspace and a private `YEET_CONFIG_DIR`. No production
services were restarted and no user sessions were modified.

### Unbounded snapshot retention

Procedure:

- Make a `SharedSession` containing one 65,536-byte user entry.
- Use the real `apply_agent_event()` to apply one-character `ToolCallDelta`s.
- Publish through the real `state_envelope()` and `EventSender`, matching
  `project_event()`'s tool-delta path.
- Pause consumption while producing a capped number of events.
- Count queued envelopes and the retained user-content bytes when draining.
- Sample the test process RSS using `ps -p PID -o rss=`.

Fresh-process measurements:

| Events | Queued snapshots | Retained transcript-content bytes | RSS baseline | RSS after production |
| --- | --- | --- | --- | --- |
| 256 | 256 | 16,777,216 (16 MiB) | 9,488 KiB | 29,312 KiB |
| 2,048 | 2,048 | 134,217,728 (128 MiB) | 9,456 KiB | 157,424 KiB |

The 2,048-event run also measured 83,744 KiB RSS halfway through (1,024 events).
The live transcript was only 64 KiB; the argument being streamed was only
2 KiB. The retained history alone scales as `pending_events × transcript_bytes`.
Other cloned state and allocator overhead add to that. RSS is sensitive to
allocator reuse, so byte counts and pending-envelope counts are the stronger
invariant; separate fresh-process runs avoid confusing reuse with boundedness.

### Settings refresh stalls the consumer

Procedure:

- Spawn a real `HarnessService` in a private temporary configuration/workspace.
- Mark its state streaming and hold its coordinator mutex, representing the
  lock held by `RunWorker::drive()` during a run.
- On the command/consumer thread, call `send(RequestSettings)`.
- Verify the command has not returned after 300 ms.
- While the mutex remains held, publish 256 real tool-delta snapshots using
  the same state/queue path as above. The command remains blocked.
- Release the coordinator mutex. The command returns and the service can drain.

Observed result:

```text
request_settings_waited_for_coordinator=true queued_after_unlock=258
```

The extra envelopes are normal service publications. Both controlled probes
passed their diagnostic assertions. This confirms the blocking/retention
mechanism, not the original incident's exact sequence of browser actions.

## Relationship to the reported cutoff

Local incident evidence (2026-11-08, KST):

- Original run started 21:10:22.
- macOS Jetsam report at 21:19:26 lists Yeet PID 78742 as the largest process:
  1,250,074 pages × 16,384 bytes = approximately 19.075 GiB.
- The report dated 21:21:32.91 lists the same process at 1,366,560 pages,
  approximately 20.852 GiB, while other processes are killed for `low-swap`.
- A replacement workspace background daemon started 21:33:48.
- At 21:33:51 session recovery marked the old run interrupted. That is the
  recovery timestamp, not necessarily the original termination timestamp.

The original pre-request transcript's first 186 entries contain approximately
1,210,687 bytes of string values, before including catalogs or other state.
About 17,738 retained copies of those strings alone would account for 20 GiB.
This is a scale illustration, **not** a measured incident queue length.

The reproduced defects are a strong explanation for the observed behavior,
including stalled UI/control delivery and huge memory growth. However, there
is no retained heap dump, original command trace, or direct termination-signal
record establishing that a particular settings refresh triggered this exact
incident. The Jetsam snapshots mark that Yeet process active, with no kill
reason attached; do not claim they directly prove it was Jetsam-killed.

The later continuation completed independently at 23:48:09. The initial
inspection at approximately 23:01 happened while that continuation was active.

## Recommended repair boundaries

1. Make settings read refreshes nonblocking with respect to an active
   coordinator. Apply execution configuration at a safe turn boundary or use
   an explicit deferred-update mechanism; do not silently discard changes.
2. Coalesce cumulative snapshots **before enqueueing** at both internal event
   hops. Preserve the latest full transcript when replacing it with a compact
   snapshot, respect session boundaries, and preserve discrete RPC replies and
   errors. Simply adding a blocking bounded channel under the live-state mutex
   could introduce another deadlock.
3. Reduce full-transcript work on streamed tool arguments, without hiding the
   in-progress tool call or changing protocol ownership.
4. Regression coverage should exercise a busy coordinator plus settings
   refresh, a slow/paused consumer with many tool-argument fragments, final
   transcript/permission delivery, and interrupt responsiveness. Keep stress
   sizes capped; reproducing 20 GiB of consumption is unnecessary and unsafe.
5. Add lightweight queue-depth/retained-byte diagnostics so future backend
   replacement reports can distinguish allocation pressure from a transport
   reconnect. A heap profile is still needed if growth persists after these
   reproduced retention paths are repaired.
