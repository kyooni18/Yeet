# Yeet

Yeet is a native agent CLI/TUI with a TypeScript runtime, Remote WebUI, MCP support, and cross-platform releases.

## Install

Prebuilt releases support macOS, Linux, and Windows on ARM64 and x64. Node.js 20 or newer is required.

Download the archive for your platform from GitHub Releases, extract it, then run:

```sh
./install.sh
```

On Windows PowerShell:

```powershell
.\install.ps1
```

Linux releases also include Debian packages.

To build from source, install Rust stable, Node.js 20+, npm, and pnpm (or Corepack), then run:

```sh
git clone https://github.com/kyooni18/Yeet.git
cd Yeet
npm --prefix RuntimeSource ci
./Scripts/install-local.sh
```

On Windows, use `Scripts\install-local.ps1`.

## Use

Start the TUI:

```sh
yeet
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
./Scripts/rebuild-runtime.sh
./Scripts/build-remote-web.sh
cargo test --all-targets
```

## Releases

Release assets include SHA-256 checksums and GitHub build-provenance attestations. See [`docs/RELEASING.md`](docs/RELEASING.md) for the release process and [`docs/PLATFORM_SUPPORT.md`](docs/PLATFORM_SUPPORT.md) for platform details.

## License

Apache-2.0. See [`LICENSE.txt`](LICENSE.txt).
