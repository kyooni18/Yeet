# Yeet

Yeet is a native agent CLI/TUI with a TypeScript runtime, Remote WebUI, MCP support, and cross-platform releases.

## Install

Prebuilt macOS and Linux releases require Node.js 20 or newer at runtime, but they do not require Rust, npm, or a local Yeet source checkout.

Install the latest signed-by-checksum GitHub release:

```sh
curl -fsSL https://raw.githubusercontent.com/kyooni18/Yeet/main/install.sh | sh
```

The installer detects macOS/Linux and arm64/x86_64, downloads the matching GitHub Release archive, verifies its SHA-256 sidecar, and installs Yeet under `~/.local`. Set `YEET_PREFIX` to change the install prefix or `YEET_VERSION` to install a specific release.

Homebrew on macOS or Linux:

```sh
brew install kyooni18/tap/yeet
```

Homebrew supplies the Node.js runtime dependency automatically. Linux releases also publish native `.deb` packages for amd64 and arm64.

To explicitly build and install the current source checkout instead of downloading a release:

```sh
git clone https://github.com/kyooni18/Yeet.git
cd Yeet
./install.sh --build
```

Source and release installers build or stage the replacement before stopping
running Yeet executables and their runtime, tool, and MCP child processes. They
replace the complete runtime tree and binary together, including other Yeet
installations on PATH, then restart background,
remote, and HTTP MCP services with their previous arguments. Sessions, API keys,
and MCP configuration are retained. Interactive sessions reopen in new terminal
windows when a desktop terminal is available; headless terminals print the reopen
command. External stdio MCP clients must reconnect. On macOS the process inventory also
requires Python 3 and `lsof` to preserve arguments containing spaces.

Windows source installation remains available through PowerShell:

```powershell
iwr https://raw.githubusercontent.com/kyooni18/Yeet/main/install.ps1 -UseBasicParsing | iex
```

Foundation is optional and is not cloned or built with Yeet. Enable it when needed:

```sh
yeet install foundation
```

That clones Foundation into Yeet-managed state, runs its normal npm build, registers its stdio MCP server, and enables memory for the current workspace. Remove it with `yeet uninstall foundation`. Apple Foundation Models remain an optional Foundation-specific build and are not part of this path.

Other components use the same CLI surface:

```sh
yeet install web
yeet install mcp
yeet install computer-use
yeet install status
```

`yeet install web` enables Yeet's native Exa web search on supported platforms without installing Python, npm packages, or a search server. It uses the remote Exa MCP service over HTTPS; `yeet search test` checks connectivity. Advanced users can still configure other providers, including SearXNG for language, category, time and pagination filters.

The built-in `computer_use` tool uses [Open Computer Use](https://github.com/iFurySt/open-computer-use), installed by `yeet install computer-use`. This requires npm and installs the pinned upstream package in `<Yeet state>/computer-use`, independently of Codex. `YEET_CONFIG_DIR` controls the state directory; `YEET_NPM` overrides npm. No source checkout or separate MCP registration is required. Use `yeet install status` to check it and `yeet uninstall computer-use` to remove it. Restart active Yeet sessions after installing or removing the runtime. macOS requires 14+ with Accessibility and Screen Recording permissions; Linux and Windows need a logged-in graphical session.

`mcp` is a mode of the main Yeet binary; its install/uninstall commands start and stop the local MCP server daemon rather than building a separate executable. `remote` is also built directly into the Yeet binary and has no separate installation or managed service lifecycle (`yeet install remote` notes that it is already built-in).

## Use

Yeet reads user-wide instructions from `AGENT.md` in its configuration directory
(`YEET_CONFIG_DIR`, or the platform's default Yeet state directory). It also reads
each workspace's root `AGENT.md`, `AGENTS.md`, and `YEET.md` on every turn.
Nested files with those names are loaded when file tools touch their subtree;
the nearest nested instruction directory applies. These files are user-managed:
Yeet does not create or overwrite them when loading instructions.

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

## Maintenance

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

## Frontend development

The future UI design specification starts at [`app/ui/ContentView.ui`](app/ui/ContentView.ui) and the meaningful regions under `app/ui`. Edit those design documents and ask an AI agent to synchronize the normal implementation; they are never interpreted at runtime.

The blank foundation for future user-designed UI is [`frontend/react-native`](frontend/react-native/README.md), using React Native and Expo for iOS, iPadOS, Android and web. [`frontend/desktop`](frontend/desktop/README.md) hosts its web export in Tauri 2 and connects to the existing Rust Harness for macOS, Windows and Linux. The current `web` client remains the release default during incremental parity work. See [`docs/FRONTEND_ARCHITECTURE.md`](docs/FRONTEND_ARCHITECTURE.md) for boundaries and migration details.

## Releases

Release assets include SHA-256 checksums and GitHub build-provenance attestations. See [`docs/RELEASING.md`](docs/RELEASING.md) for the release process and [`docs/PLATFORM_SUPPORT.md`](docs/PLATFORM_SUPPORT.md) for platform details.

## License

Apache-2.0. See [`LICENSE.txt`](LICENSE.txt).
