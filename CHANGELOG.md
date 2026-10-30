# Changelog

## [Unreleased]

## [0.1.6] - 2026-10-30

- Harden opt-in Remote: enforce browser Origin checks, throttle bad access keys, bind to loopback by default, and refuse unauthenticated non-loopback binds.
- Harden the MCP HTTP server: reject cross-origin browser requests, cap OAuth client registrations, and throttle authorization-key attempts.
- Permission prompts now offer allow once or allow for session; session approvals are saved with the session.
- Disabled tools, Skills, and MCP servers now ask for permission instead of failing the model's request.
- Offer to install the Computer Use runtime when the model requests it and it is missing.
- Split web search installer and result normalization into separate modules.

## [0.1.5] - 2026-10-30

- Stabilize terminal redraw verification during asynchronous Git refresh.

## [0.1.4] - 2026-10-30

- Add lightweight native web search and improve search configuration.
- Improve Linux shell handling and headless permission prompts.
- Improve Windows terminal input and suppress background console windows.
- Update web dependencies.

## [0.1.3] - 2026-10-26

- Add richer tool detail views for file edits, shell commands, and file reads, with explicit lifecycle status.
- Add TUI session management with rename, workspace moves, safe deletion, and Vim-style navigation and search.
- Stream shell output to paged disk transcripts and hand foreground timeouts off to background jobs.
- Improve shell timeout handling, output diagnostics, and working-directory accounting.
- Fix cold provider bridge startup and gaps in multiline TUI message backgrounds.

## [0.1.2] - 2026-10-23

- Improve TUI and Remote workbench navigation, tool displays, reasoning visibility, and rendering responsiveness.
- Preserve conversation context and current-run progress across context rollover.
- Improve file editing reliability, shell working-directory persistence, and tool output handling.
- Add main-agent user questions and improve goal-mode controls and blocked-goal handling.
- Improve provider compatibility, session persistence recovery, and shared Remote/TUI runtime behavior.

## [0.1.1] - 2026-10-09

- Improve agent group orchestration, task lifecycle handling, and agent controls.
- Improve TUI and WebUI workspace, session, composer, and activity behavior.
- Improve provider authentication and OpenAI reasoning compatibility.
- Improve installer replacement lifecycle handling.

## [0.1.0] - 2026-09-15

- Native Rust CLI/TUI with TypeScript provider runtime.
- Remote WebUI, MCP server, Skyline, extensions, and computer-use integration.
- OpenAI, Codex CLI, Claude, Gemini, OpenRouter, OpenCode, and custom OpenAI-compatible providers.
- macOS, Linux, and Windows releases for ARM64 and x64.
- Self-update support with SHA-256 verification.
- Docker support and release provenance attestations.
