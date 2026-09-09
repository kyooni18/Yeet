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
git clone https://github.com/Yeet-AI/Yeet.git
cd Yeet
npm --prefix RuntimeSource install
./Scripts/install-local.sh
```

### Windows (PowerShell)

```powershell
git clone https://github.com/Yeet-AI/Yeet.git
Set-Location Yeet
npm --prefix RuntimeSource install
.\Scripts\install-local.ps1
```

The installer builds Yeet and installs it to a local executable directory. Add
that directory to your `PATH` if needed. To build without installing:

```sh
npm --prefix RuntimeSource install
./Scripts/rebuild-runtime.sh
cargo build --release
```

On Windows, use `.\Scripts\rebuild-runtime.ps1` in place of the shell script.
`serve.sh` and `serve.ps1` provide the matching local development setup for
POSIX shells and PowerShell respectively.

Run `yeet` to start the TUI, or `yeet doctor` to check the setup. Configure a
model provider and credentials from the app or your environment before sending
requests. `yeet cache latest` reports per-attempt cache hits, writes, estimated
cache-surface size, intra-turn prefix/schema churn, and any rewrite of older
active-turn history that would break an otherwise reusable provider prefix
in the current workspace. Use `yeet remote` only when you want to expose the UI
in a browser.

### Codex Computer Use

On macOS, Yeet can use the Computer Use runtime bundled with the installed
ChatGPT/Codex app directly. It appears as the built-in `Computer Use`
capability (`computer_use` / `computer_use_reset`) and does not require adding
or linking an MCP server. Yeet discovers the newest installed
`unified-computer-use` Codex plugin at runtime, launches Codex's own CUA node
runtime lazily on the first call, keeps the JavaScript session persistent, and
routes native-app permission requests through Yeet's normal approval UI.

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
