# Platform support

Yeet treats macOS, Linux, and Windows as native desktop/server targets. Platform support is gated in CI rather than inferred from conditional compilation.

## Supported tiers

| Platform | Architecture | CI level | Shell sandbox | Local IPC | Release artifact |
| --- | --- | --- | --- | --- | --- |
| macOS | arm64 | Full | sandbox-exec | Unix socket | `.tar.gz` |
| macOS | x86_64 | Release build | sandbox-exec | Unix socket | `.tar.gz` |
| Linux | x86_64 | Full | Landlock via `nono` | Unix socket | `.tar.gz`, `.deb` |
| Linux | arm64 | Full native | Landlock via `nono` | Unix socket | `.tar.gz`, `.deb` |
| Windows | x86_64 | Full | AppContainer via `io-harness` | authenticated loopback TCP | `.zip` |
| Windows | arm64 | Full native | AppContainer via `io-harness` | authenticated loopback TCP | `.zip` |

`Full` means Rust format/check/test/Clippy, RuntimeSource build/tests, CLI/runtime smoke, and release builds are expected to pass on that target. Linux and Windows also execute native sandbox smoke tests. Remote WebUI builds on all primary desktop OSes; browser E2E runs on Linux.

## Configuration and state

`YEET_CONFIG_DIR` always wins when set. Existing `~/.yeet` installations remain in place after upgrade so a user never silently splits state between the Rust host and RuntimeSource. New clean installations use:

- macOS: `~/.yeet` for compatibility with existing Yeet/Codex workflows.
- Linux: `$XDG_CONFIG_HOME/yeet`, falling back to `~/.config/yeet`.
- Windows: the roaming application-data directory under `Yeet`.

Sensitive directories and files are hardened to user-private permissions. Unix uses restrictive mode bits. Windows removes inherited ACL entries and grants full control to the current user, SYSTEM, and local Administrators. Windows local daemon endpoint files receive the same protection and carry a per-listener random authentication token.

## Shell contract

`run_shell` is a non-interactive automation command runner, not an interactive terminal emulator. It captures stdout/stderr and does not promise PTY/ConPTY semantics. The native default shell is POSIX shell semantics on macOS/Linux and `cmd.exe /D /S /C` semantics on Windows. PowerShell is supported by invoking `powershell.exe`/`pwsh` explicitly in a command.

Sandboxed commands are fail-closed. Linux uses Landlock through `nono`; Windows uses an AppContainer helper. Sandboxed execution denies network access by default. An unsupported network allowlist or secret-injection request is rejected rather than silently widening permissions.

## Process and daemon lifecycle

Unix background daemons are detached into their own session/process group and tree shutdown attempts SIGTERM before SIGKILL. Windows uses detached process groups and tree-aware `taskkill`; shutdown attempts a non-forced tree termination before falling back to `/F`.

Unix local control channels use filesystem Unix-domain sockets. Windows uses `127.0.0.1` with a random per-listener bearer token stored in the private endpoint file. An unauthenticated process that only discovers the TCP port cannot issue local daemon commands.

## Browser authentication

Browser OAuth uses the platform-native launcher with fallbacks:

- macOS: `open`.
- Windows: Explorer, then PowerShell `Start-Process`.
- WSL: `wslview`, Windows Explorer, then `xdg-open`.
- Linux desktop: `xdg-open`, then `gio open`.

When no opener is available, Yeet prints the authorization URL instead of failing the callback flow, which keeps headless/server authentication usable.

## Computer Use

Yeet's Computer Use integration itself is platform-neutral: it discovers the newest usable Codex `unified-computer-use` plugin from `CODEX_HOME`, validates native absolute executable/script paths, and exposes its persistent `js`/`js_reset` session. Actual desktop control availability still depends on whether the installed Codex distribution provides that plugin and whether the operating system grants the plugin's native automation permissions. Yeet fails with an explicit unavailable-runtime error when it is not installed; it does not silently substitute browser automation.

## Releases and integrity

Tag pushes matching `v*` build release artifacts on native macOS arm64/x64, Linux arm64/x64, and Windows arm64/x64 runners. Each package gets a SHA-256 sidecar and the release gets an aggregate `SHA256SUMS`. GitHub build-provenance attestations are emitted for release artifacts. Windows Authenticode signing is enabled automatically when the release environment provides `WINDOWS_SIGN_CERTIFICATE_BASE64` and `WINDOWS_SIGN_CERTIFICATE_PASSWORD`; unsigned development releases remain possible when no certificate is configured.

Extracted archives include `install.sh` or `install.ps1`, which install without a Rust toolchain and can also uninstall the application. Linux CI additionally creates Debian packages when `dpkg-deb` is available.

## Support boundaries

Node.js 20 or newer remains a runtime requirement. Yeet's CI uses Node.js 22. Platform-native sandbox behavior is tested on current GitHub-hosted OS images; older kernels or Windows builds that lack the required OS isolation primitives are expected to fail closed rather than run model shell commands unrestricted.
