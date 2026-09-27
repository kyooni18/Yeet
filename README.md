# Yeet

Yeet is a native agent CLI/TUI with a TypeScript runtime, Remote WebUI, MCP support, and cross-platform releases.

## Install

Build dependencies are Rust, Node.js/npm, and the platform C/C++ toolchain. Git is only needed to clone the source. Yeet uses npm everywhere; pnpm and Corepack are not required.

One-command source install on macOS/Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/kyooni18/Yeet/main/install.sh | sh
```

PowerShell:

```powershell
iwr https://raw.githubusercontent.com/kyooni18/Yeet/main/install.ps1 -UseBasicParsing | iex
```

The installer clones Yeet into `~/.local/src/yeet` (unless already run from a checkout), builds RuntimeSource and the Remote WebUI with npm, builds the Rust release binary, then installs the binary and runtime under `~/.local`. Override these locations with `YEET_SOURCE_DIR` and `YEET_PREFIX`.

Foundation is optional and is not cloned or built with Yeet. Enable it when needed:

```sh
yeet install foundation
```

That clones Foundation into Yeet-managed state, runs its normal npm build, registers its stdio MCP server, and enables memory for the current workspace. Remove it with `yeet uninstall foundation`. Apple Foundation Models remain an optional Foundation-specific build and are not part of this path.

Other components use the same CLI surface:

```sh
yeet install web
yeet install mcp
yeet install status
```

`mcp` is a mode of the main Yeet binary; its install/uninstall commands start and stop the local MCP server daemon rather than building a separate executable. `remote` is also built directly into the Yeet binary and has no separate installation or managed service lifecycle (`yeet install remote` notes that it is already built-in).

## Use

Start the TUI:

```sh
yeet
```

### Remote WebUI

Yeet Remote is built into the Yeet binary and is workspace-neutral: the Remote server itself owns no project directory and never derives one from the shell current directory. Each browser client restores or selects its own workspace, falling back to the user's home directory.

Start the semantic WebUI in the foreground:

```sh
yeet remote
```

Or detach a single Remote process in the background:

```sh
yeet remote --background
```

`--background` only detaches the current process. It does not install a launchd/systemd service or register a supervisor or restart policy, so a stopped or killed Remote stays dead and will not automatically respawn.

Check status or stop the background Remote:

```sh
yeet remote status
yeet remote stop
```

## Skyline

Skyline is Yeet's opt-in shared coordination layer for parallel workers. It
persists mission context, compact peer intent, evidence, messages, and
exclusive test-run state under `~/.yeet/Skyline`. Its native `deploy_agent`
operation can start another Yeet agent in an isolated background session and
returns the child session ID for TUI/WebUI follow-up.

Initialize its runtime and the current workspace with:

```sh
yeet skyline setup --workspace "$PWD"
```

In the TUI, use `/skyline on` or enable Skyline in Capabilities. MCP clients
can explicitly activate `builtin:skyline` through `activate_capability`, then
call the returned handle with `invoke_capability`. The standalone native
`deploy_agent` tool is available to Yeet agent turns and is intentionally not
exported as a direct MCP tool.

Check the installation:

```sh
yeet doctor
```

Check for updates:

```sh
yeet update check
```

Run `yeet update` to install the latest release.

## Docker

```sh
docker build -t yeet .
docker run --rm yeet doctor
docker run --rm -it -v "$PWD:/workspace" yeet
```

## Development

```sh
npm --prefix RuntimeSource ci
npm --prefix web ci
npm --prefix RuntimeSource run build
npm --prefix web run build
cargo test --all-targets
```

## Releases

Release assets include SHA-256 checksums and GitHub build-provenance attestations. See [`docs/RELEASING.md`](docs/RELEASING.md) for the release process and [`docs/PLATFORM_SUPPORT.md`](docs/PLATFORM_SUPPORT.md) for platform details.

## License

Apache-2.0. See [`LICENSE.txt`](LICENSE.txt).
