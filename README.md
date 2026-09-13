# Yeet

idk

## Requirements

- Rust stable (including `cargo`)
- Node.js 20 or newer
- npm

Rust builds the application; Node runs Yeet's TypeScript provider and tool
runtime.

## Install

### macOS / Linux

```sh
git clone https://github.com/kyooni18/Yeet.git
cd Yeet
npm --prefix RuntimeSource install
./Scripts/install-local.sh
```

### Windows (PowerShell)

```powershell
git clone https://github.com/kyooni18/Yeet.git
Set-Location Yeet
npm --prefix RuntimeSource install
.\Scripts\install-local.ps1
```

The source installers build Yeet and install it to a local executable directory. Add
that directory to your `PATH` if needed. Tagged releases also publish prebuilt
macOS/Linux archives, Debian packages, and Windows ZIP bundles with SHA-256
checksums and build-provenance attestations; extracted bundles include a toolchain-
free `install.sh` or `install.ps1`.

To build without installing:

```sh
npm --prefix RuntimeSource install
./Scripts/rebuild-runtime.sh
./Scripts/build-remote-web.sh
cargo build --release
```

On Windows, use the matching `.ps1` build scripts. See
[`docs/PLATFORM_SUPPORT.md`](docs/PLATFORM_SUPPORT.md) for the native Linux/Windows
CI matrix, state paths, sandbox/IPC guarantees, ARM64 coverage, and release policy.

Installed release bundles can update themselves with `yeet update`; use
`yeet update check` to check without installing. The updater selects the native
OS/architecture asset and verifies its published SHA-256 before installation.

Run `yeet` to start the TUI, or `yeet doctor` to check the setup. Configure a
model provider and credentials from the app or your environment before sending
requests. `yeet cache latest` reports per-attempt cache hits, writes, estimated
cache-surface size, intra-turn prefix/schema churn, and any rewrite of older
active-turn history that would break an otherwise reusable provider prefix
in the current workspace. Use `yeet remote` only when you want to expose the UI
in a browser.

OpenAI credentials are shown as separate catalog providers: `openai/*` uses
direct API-key billing, while `codex-cli/*` uses the ChatGPT/Codex subscription
login. Existing OpenAI browser-login credentials are migrated to `codex-cli`
when the runtime next starts.

Gemini and Anthropic credentials use the same separation: `gemini/*` and
`anthropic/*` are direct API-key providers, while `gemini-web/*` uses Google
browser OAuth and `claude/*` uses the installed Claude Code web login. Use
`yeet auth login gemini-web` or `yeet auth login claude` to sign in.

### Codex Computer Use

Yeet can directly use the `unified-computer-use` runtime supplied by an installed
Codex distribution. Discovery is platform-neutral: Yeet scans `CODEX_HOME`, accepts
native executable/script paths on macOS, Linux, and Windows, launches the newest
usable runtime lazily, keeps its JavaScript session persistent, and routes native-
app permission requests through Yeet's normal approval UI. Availability of desktop
control still depends on the Codex distribution actually providing that plugin for
the host OS; Yeet reports a clear unavailable-runtime error when it does not.

The older `Scripts/link-codex-computer-use.sh` path remains only for backwards
compatibility with existing setups; new installations should use the built-in
capability.

## MCP server

`yeet mcp` remains the MCP client/configuration command. Yeet's own server is
managed separately with `yeet mcpserver`, and exports Yeet's native tools
directly without starting a Yeet agent turn or asking a model what to do.

The default managed HTTP daemon listens on `127.0.0.1:7332`, generates an
access key on first start, and serves MCP at `/mcp`:

```sh
yeet mcpserver
yeet mcpserver start --port 8443 --workspace /path/to/default-project
yeet mcpserver status --port 8443
yeet mcpserver restart --port 8443
yeet mcpserver stop --port 8443
yeet mcpserver list
```

The HTTP transport is session-aware. A successful `initialize` response returns
`Mcp-Session-Id`; clients should send that header on later requests and may
terminate the session with `DELETE /mcp`. Each session owns an isolated tool
runtime, so independent clients and agent sessions can execute concurrently
without sharing file snapshots, artifacts, shell jobs, or Computer Use state.
Calls within one session stay ordered to preserve that state. If a second call
overlaps a busy session for too long, Yeet returns an explicit retryable 503
instead of tying up an HTTP worker indefinitely.

Clients that do not send `Mcp-Session-Id` use a bounded legacy runtime pool
instead of one global process. Independent/stateless calls are distributed
across lanes, while snapshot edits, background-job handles, artifact handles,
and Computer Use state retain the affinity needed for follow-up calls.

`GET /health` reports process/runtime liveness and `GET /ready` reports whether
at least one runtime lane is immediately available. Runtime responses are also
bounded by tool-aware timeouts; an unresponsive isolated runtime is restarted
before later requests are accepted.

Different ports are independent managed daemons, including separate auth and
daemon state. `--bind HOST` selects the listen interface. For a reverse proxy
or non-loopback bind, set the externally reachable canonical endpoint with
`--public-url https://host.example/mcp`.

HTTP authentication supports a static Bearer key or OAuth. Key material is
stored only as an Argon2 hash. OAuth uses authorization-code + PKCE and serves
MCP protected-resource metadata, OAuth authorization-server metadata, Client ID
Metadata Documents (CIMD), and legacy dynamic client registration:

```sh
yeet mcpserver auth key generate --port 8443
yeet mcpserver auth mode key --port 8443
yeet mcpserver auth mode oauth --port 8443
yeet mcpserver auth status --port 8443
```

`--auth key|oauth|none` can select the mode while starting or restarting a
daemon. Unauthenticated mode is restricted to loopback binds, and OAuth public
URLs must use HTTPS except for localhost testing. In OAuth mode the configured
MCP key is the human authorization secret entered on Yeet's authorization page;
clients use the issued OAuth Bearer token for `/mcp` calls.

For MCP hosts that spawn a stdio server instead of connecting over HTTP:

```sh
yeet mcpserver stdio --workspace /path/to/default-project
yeet mcpserver stdio-config --workspace /path/to/default-project
```

Every exported native tool accepts an optional `workspace` argument. It selects
the project directory for that call, while snapshots, artifacts, and background
shell jobs remain isolated per workspace. The exported names include
`read_file`, `apply_file_edits`, `run_shell`, `web_search`, `read_document`,
`computer_use`, and `computer_use_reset`; Computer Use is backed directly by the
installed Codex runtime and does not require a user-configured Computer Use MCP.
Headless Yeet MCP calls fail closed instead of hanging if Codex requests an
interactive native-app approval. There is no `yeet_run` agent wrapper.
`sandbox_get` and `sandbox_configure`
manage the same per-workspace `.yeet/sandbox.json` used by the CLI and TUI.
Yeet accepts MCP 2026-07-28 and legacy 2025-era clients.

## Development

```sh
cargo test
npm --prefix RuntimeSource test
```
