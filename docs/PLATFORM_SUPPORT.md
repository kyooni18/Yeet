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

Tag pushes matching `v*` first verify that the tag matches the synchronized Cargo, RuntimeSource, and WebUI package versions, then run the focused release checks. Release artifacts are built on native macOS arm64/x64, Linux arm64/x64, and Windows arm64/x64 runners; each staged archive must report the expected version and pass `yeet doctor` against its bundled runtime before upload. Unix packaging additionally smoke-tests the bundled installer into a temporary prefix. Each package gets a SHA-256 sidecar and the release gets an aggregate `SHA256SUMS`. GitHub build-provenance attestations are emitted for release artifacts. Windows Authenticode signing is enabled automatically when the release environment provides `WINDOWS_SIGN_CERTIFICATE_BASE64` and `WINDOWS_SIGN_CERTIFICATE_PASSWORD`; unsigned development releases remain possible when no certificate is configured.

Extracted Unix archives include `install.sh`, which requires Node.js 20 or newer and installs the prebuilt binary plus bundled runtime without a Rust toolchain. Linux CI additionally creates Debian packages when `dpkg-deb` is available. Published macOS/Linux checksums are also used to render the Homebrew formula for the external tap.

## Linux containers

The supported Docker image uses Debian Bookworm with Node.js 22, runs Yeet as a non-root user, and places `tini -g` at PID 1 so Unix stop signals reach the Yeet child process group while orphaned children are reaped. The image also supplies a UTF-8 locale and a conservative `xterm-256color` terminal default for interactive TUI sessions. `YEET_RUNTIME_DIR` and `YEET_CONFIG_DIR` are explicit rather than depending on desktop-specific directory discovery.

Yeet-owned background runtimes, provider bridges, and extensions are placed in dedicated Unix process groups. Forced cleanup captures the descendant tree before signaling and terminates each Yeet-owned process or process group, so a child that creates its own group is not left behind when a supervisor is recycled. On Linux, descendant discovery reads `/proc` directly rather than depending on `ps`; a compatibility `ps` path remains for other Unix platforms.

Landlock is a host-kernel capability, so some Docker Desktop/VM kernels do not expose it even though the image itself is Linux. `yeet doctor` reports the detected Landlock ABI or an explicit unavailable/fail-closed status. When Landlock is unavailable, sandboxed shell execution remains disabled rather than falling back to unrestricted execution. A user who deliberately accepts the container itself as the security boundary can opt in to unrestricted shell execution with `yeet sandbox mode unlimited`; Yeet never enables that downgrade automatically.

The TUI requires both stdin and stdout to be terminals. In Docker, use `-it`; non-interactive invocations fail before background/TUI setup with an actionable diagnostic. Long-lived container services should use `yeet mcpserver run` so the MCP server remains in the foreground and participates in the container lifecycle. Wildcard binds such as `0.0.0.0` require an explicit `--public-url` because Yeet cannot infer the externally reachable MCP URL from a container bind address; for local port publishing use `--public-url http://localhost:7332/mcp`, and use the real HTTPS endpoint for remote exposure. Managed `mcpserver start` remains useful on normal Linux hosts but is not the recommended one-shot-container lifecycle.

`Scripts/test-linux-container.sh` builds the production image and verifies the packaged Node bridge with `yeet doctor`, non-root/config writes, collision-safe UID/GID remapping, named-volume config persistence, operation with a read-only image filesystem plus writable config/tmp/workspace mounts, deterministic no-TTY failure, and the real direct-entrypoint foreground HTTP MCP server through its `/health` endpoint plus clean `docker stop` handling. It also exercises the packaged Linux Landlock helper: supported kernels must enforce a read-only grant, while kernels without Landlock must fail closed with the documented diagnostic. CI runs this smoke on Ubuntu so container packaging and Linux isolation behavior are continuously checked in addition to native Linux tests.

## Support boundaries

Node.js 20 or newer remains a runtime requirement. Yeet's CI uses Node.js 22. Platform-native sandbox behavior is tested on current GitHub-hosted OS images; older kernels or Windows builds that lack the required OS isolation primitives are expected to fail closed rather than run model shell commands unrestricted.
