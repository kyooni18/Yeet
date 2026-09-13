# Yeet Remote WebUI integration status

Last updated: 2026-09-09 (KST)
Integration owner: `@REMOTEINTEGRATE`

## Completion gate

The final Remote migration is intentionally gated on both primary implementations being present in this shared repository and building successfully:

- `@REMOTECORE`: semantic, versioned WebSocket Remote backend/protocol.
- `@WEBUI`: functional Vue 3 / TypeScript / Vite WebUI under `web/`.

The semantic WebUI is now the default production Remote path. The legacy browser-TUI remains intentionally available behind `yeet remote --legacy-tui` as a compatibility fallback for this migration cycle. The normal semantic server does not register `/api/frame` or `/api/input`; those terminal-specific endpoints exist only on the explicit legacy path.

Current shared-tree observation:

- `@REMOTECORE` protocol v1 is implemented at `/api/ws` over Axum/Tokio and talks directly to the existing semantic background frontend/backend path.
- `web/` contains the Vue 3 / TypeScript / Vite WebUI and `web/dist/` is embedded into Yeet at build time.
- `src/main.rs` launches the semantic WebUI path by default and uses the Ratatui/TestBackend renderer only for `--legacy-tui`.
- `src/remote/server.rs` serves embedded WebUI assets with SPA fallback/cache policy while keeping `/api/*` outside SPA fallback.
- Remote authentication is service-wide rather than workspace-scoped. The legacy per-workspace auth document is migrated into `~/.yeet/remote/auth.json`, and auth changes are reloaded across running Remote daemons.
- Authenticated semantic clients may select a workspace in the WebUI. The daemon startup workspace is only the default; each `(workspace, client_id)` gets an isolated semantic runtime.
- `docs/REMOTE_PROTOCOL.md` is the authoritative browser/backend wire contract.

`@WEBUI` integration note: protocol-v1 assistant/reasoning streams are delta-first. The final store appends `delta` for `reset=false`, replaces from authoritative `content` for `reset=true`, batches semantic events on `requestAnimationFrame`, and preserves the server conversation revision rather than manufacturing browser-side revisions.

## Protocol / frontend state

Protocol version: `1` (`/api/ws`, semantic WebSocket backend implemented by `@REMOTECORE`).
WebUI build: present under `web/` and embedded by the Rust build integration.
Production default: semantic Vue/Vite WebUI.
Legacy fallback: `yeet remote --legacy-tui`.

## Production asset integration contract

The production Remote experience must remain a single Yeet command and must not require a Node or Vite development server at runtime.

The production asset integration is now present:

1. `web/` owns the Vue/Vite source and emits `web/dist/`.
2. `build.rs` generates embedded asset metadata for the Rust build.
3. The Axum Remote server serves `index.html` plus hashed assets directly from the Yeet binary; hashed assets are immutable-cacheable while `index.html` is no-store.
4. SPA navigation falls back to `index.html`; `/api/*` paths never fall through to the SPA.
5. `yeet remote` remains a single-command production experience without a Node server; Vite remains available for development/HMR.
6. `--legacy-tui` keeps the previous rendered-terminal browser path isolated as a fallback.

## CLI compatibility matrix

| Behavior | Baseline | Final target | Status |
| --- | --- | --- | --- |
| `yeet remote [WORKSPACE]` | detached legacy browser-TUI | detached semantic WebUI | implemented |
| `--workspace PATH` | supported | preserve | implemented |
| positional workspace | supported | preserve | implemented |
| `--bind ADDRESS` | supported | preserve | implemented |
| `--origin URL` | supported | preserve/security-sensitive | implemented |
| `remote status` | supported | preserve | implemented |
| `remote stop` | supported | preserve | implemented |
| `remote auth status` | supported | preserve | implemented |
| access-key auth commands | supported | preserve | implemented |
| passkey auth commands | supported | preserve | implemented |
| `--legacy-tui` | absent | legacy fallback | implemented |
| normal Ratatui TUI | supported | no regression | regression-tested |

The existing `--size COLSxROWS` option is terminal-specific. Preserve it for `--legacy-tui`; do not force terminal dimensions into the semantic WebUI protocol.

## Mobile / responsive QA matrix

All rows must be exercised against the production build and, where meaningful, the Vite development build.

| Viewport / behavior | Required checks | Status |
| --- | --- | --- |
| modern iPhone portrait | safe areas, composer, keyboard, streaming scroll, drawers | PASS: WebKit iPhone 16 Pro project |
| modern iPhone landscape | low-height composer, selectors, permission sheet, keyboard | PASS: WebKit 852x393 project |
| small-width phone | wrapping, code/tool overflow, minimum tap targets | PASS: WebKit 320x568 project |
| iPad portrait | sidebar/sheet breakpoint, composer, transcript width | PASS: WebKit 820x1180 project |
| iPad landscape | split-pane layout, inspector, session switching | PASS: WebKit 1180x820 project |
| laptop | normal desktop flow | PASS: Chromium 1366x768 project |
| desktop | three-pane flow and resize | PASS: 1440x960 project |
| ultrawide | bounded readable transcript, stable side panels | PASS: Chromium 2560x1080 project |
| iOS software keyboard open/close | `visualViewport`, `100dvh`, safe-area bottom, scroll anchoring | PASS: visual-viewport geometry regression coverage on phone/tablet WebKit projects |
| long code block | horizontal scroll without page blowout | PASS: bounded long-content regression coverage |
| long tool result | bounded/collapsible card without transcript replacement | PASS: bounded long-content regression coverage |
| streaming transcript | user-follow vs manual-scroll behavior | PASS: semantic streaming + manual-scroll regression coverage |

## Reconnect / session reliability matrix

The client must reconnect to the same selected session/run identity and must never silently attach to an unrelated session.

| Scenario | Expected result | Status |
| --- | --- | --- |
| reload during generation | resume same session/run and continue stream/state recovery | PASS: persisted client/session cursor + real reload + replay tests |
| temporary Wi-Fi loss | reconnect with bounded backoff; recover snapshot/deltas | PASS: disconnect/reconnect simulation + backend replay tests |
| phone lock/unlock | reconnect without duplicate submit or session jump | PASS by equivalent socket-loss/resume path; visual viewport state is independent of transport identity |
| several-second disconnect | recover same session; reconcile missed events | PASS: 1024-event replay + snapshot resync + browser cursor test |
| network-interface change | same as transient disconnect | PASS by transport-loss/reconnect path |
| multiple tabs | deterministic per-client/session behavior; no state stealing | PASS: independent tab client identity test + backend affinity tests |
| switch workspaces | reconnect same authenticated client into selected project without re-authentication | PASS: Rust workspace resolver + Playwright selector/reconnect coverage |
| switch sessions while another run exists | background run remains associated with its original session | PASS: backend scoped-runtime/session-affinity tests + semantic session controls |
| interrupt during reconnect | interrupt is idempotent/reconciled after reconnect | PASS: queued interrupt is emitted after same-client reconnect |
| backend daemon restart | explicit disconnected/restarted state; safe resync | PASS structurally: fresh protocol discovery plus non-resumed snapshot clears stale cursor state |
| stale client cleanup | server releases dead-client resources | PASS: inactive runtime TTL is bounded; streaming runtimes are retained |

## Authentication QA matrix

Security behavior must not be weakened to simplify WebUI integration.

| Scenario | Status |
| --- | --- |
| access-key login | PASS: real Rust server browser test |
| invalid access key | PASS: real Rust server browser test |
| passkey login | PASS: real WebAuthn flow with virtual platform authenticator |
| passkey enrollment | PASS: real one-time enrollment with virtual platform authenticator |
| expired enrollment | PASS: Rust auth lifecycle regression test |
| expired auth session | PASS: Rust lifecycle test plus real stale-session invalidation test |
| unauthorized WebSocket | PASS: authenticated upgrade enforced and regression-tested |
| incorrect Origin | PASS: WebSocket + browser auth Origin validation regression-tested |
| reload after authentication | PASS: Remote-wide cookie + real reload/protocol test |
| browser without WebAuthn | PASS: passkey action is suppressed and access-key fallback remains available |
| same credential across workspaces | PASS: auth cookie/credential store no longer derives identity from workspace |

## Cross-frontend regression gate

Before final migration, verify all of the following with the ordinary Ratatui frontend as well as the semantic WebUI where applicable: normal TUI startup, background sessions, multiple sessions, model selection, reasoning selection, capabilities, sandbox, permissions, provider/auth flows, interrupt, and session restore.

Remote refactoring must keep presentation-only state out of the shared backend/model layer. Semantic Remote messages should represent frontend-independent state/actions rather than terminal coordinates, key presses, or Ratatui rendering artifacts.

## Playwright / E2E target matrix

Use the real Rust Remote server by default. Mocked transport tests may supplement but not replace real-server coverage.

Required cases: authentication, first load, submit, streaming response, interrupt, tool card, reasoning, session creation, session switching, model/reasoning controls, permission prompt, reconnect, narrow mobile viewport, landscape mobile viewport, large Markdown response, code block, and long transcript.

The final harness should also record enough diagnostics on failure to debug mobile/reconnect flakes: browser console, page errors, WebSocket close/reconnect state, current selected session id, screenshots, and Playwright traces on retry.

## Performance gate

The semantic WebUI must demonstrate:

- no 60 ms `/api/frame` polling;
- no whole-terminal HTML regeneration;
- no full-transcript replacement for each streamed token/chunk;
- bounded WebSocket/state-update rates;
- bounded server/client memory during long conversations;
- responsive scrolling while output streams;
- incremental transcript/tool/reasoning updates rather than coarse full-state serialization where avoidable.

Performance instrumentation should distinguish protocol bytes/messages from DOM/render cost so an optimization on one side cannot hide a regression on the other.

## Baseline verification

- `cargo test --all-targets`: PASS on the final integrated tree: 336 unit tests plus 6 source-layout tests, 0 failures.
- `cargo clippy --all-targets -- -D warnings`: baseline is not clean due to unrelated existing lints in cache, MCP server, tool support, and UI code. These are recorded as pre-existing and are not being mixed into Remote integration work.
- Legacy auth/frame/input Rust tests pass behind explicit `legacy_tui: true` fixtures.
- `@REMOTECORE` semantic/legacy Remote regression suite: PASS, including protocol negotiation, bounded frames, strict sequence/revision reconnect validation, session affinity, stale-session rejection, expired enrollment, and explicit legacy-fallback parsing.
- WebUI production build: PASS (`pnpm build`).
- Responsive/mock browser suite: PASS, 53 applicable cases and 51 intentional viewport-inapplicable skips across small phone, modern iPhone portrait/landscape, iPad portrait/landscape, laptop, desktop, and ultrawide projects. The suite covers semantic submit/streaming, reconnect cursor recovery, workspace selection, settings, mobile drawers/sheets, controls, permissions, multiple tabs, long transcript/code/tool output, manual-scroll preservation, and visual-viewport keyboard geometry.
- Real embedded-server suite: PASS, 5 tests. Coverage includes semantic negotiation/initial state, command/ack traffic, no legacy frame/input path on the default server, reload identity, fatal version mismatch, malformed first-frame rejection, access-key auth, stale-session invalidation, wrong Origin, and real passkey enrollment/login through a virtual platform authenticator.

## Legacy cleanup gate

Semantic parity, reconnect/auth reliability, responsive Playwright QA, and Ratatui regressions now pass. Removal of the following is nevertheless deferred for this migration cycle because `--legacy-tui` is intentionally retained as a compatibility escape hatch:

- `src/remote/page.html`;
- `src/remote/render.rs`;
- browser terminal input emulation;
- `RemoteInput` mouse/key/resize protocol;
- `/api/frame`;
- terminal buffer publishing.

These pieces now belong only to the `--legacy-tui` fallback and are not used by the production-default semantic WebUI path.

## Current blockers

There are no blockers to shipping the semantic Vue/Vune WebUI as the default `yeet remote` frontend. The retained `--legacy-tui` implementation is a deliberate compatibility fallback, not a dependency of the normal server.

Future hardening can add prolonged soak/profiling runs for exact browser/server memory curves under very long model streams. The current architecture already removes 60 ms frame polling and terminal HTML regeneration, uses compact semantic deltas, batches browser updates per animation frame, bounds per-client replay history, lazily renders long transcript rows with `content-visibility`, and bounds expandable tool/result surfaces.
