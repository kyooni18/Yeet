# Harness, tool and projection audit (2026-11-17)

Scope: how built-in tools are declared, where prompts are assembled, how tool
state reaches the TUI and the web client, and where docs and code disagree.
This is the baseline for the harness-to-web redesign; later sections record
what was changed in response.

## Baseline

Recorded in a Linux container (4 CPUs, 16 GB), before any change:

- `cargo build --example ui_{toolbar,agents,conversation,composer,settings,home,shell}_fixture`: all seven build (3m31s cold).
- `cargo test --lib -- harness::tools:: harness::agent:: shared_ui::conversation harness::service::`: 149 passed, 0 failed.
- `npx playwright test --project=laptop` (default spec set): 42 passed, 4 skipped (viewport-inapplicable).
- `YEET_LEGACY_TESTS=1 npx playwright test tests/navigation-shell.spec.ts --project=laptop`: 3 failed. The spec targets the retired React client (`Open sidebar` button); it is not part of the default set.
- Mün at the pinned revision does not build with `pnpm build` alone (`@mun-ui/compiler` imports `node:fs` without Node types). The CI recipe in `.github/actions/setup-mun` works: add `@types/node@22` at the workspace root, then build the `@mun-ui/vite...` and `@mun-ui/web...` filters.
- Importing `web/tests/mockRemote.ts` runs six synchronous `cargo build --example ...` calls. With warm artifacts this costs about 30 s of freshness checks before the first test; with cold artifacts it blocks the Playwright loader for the whole build, which reads as a hang. Build the fixtures first.
- Only Chromium is installed here, so the WebKit phone/tablet projects cannot run in this container.

## Tool inventory

| Tool | Access | Attached by default | Notes |
| --- | --- | --- | --- |
| `read_file` | read | yes | `read_files` is a hidden legacy alias |
| `search_workspace` | read | yes | |
| `apply_file_edits` | write | yes | `patch` or `changes` |
| `run_shell` | shell | yes | contract in `SHELL_EXECUTION.md` |
| `shell_job` | shell | yes | |
| `read_artifact` | read | yes | |
| `web_search`, `web_read` | read | yes, when web search is enabled | defined outside the base list |
| `ask_user` | interactive | yes, main agent only | registered by `enable_user_questions` |
| `search_tools` | control | yes | synthesized in `agent/tool_discovery.rs` |
| `outline`, `find_symbol` | read | no | |
| `read_document`, `analyze_data` | read | no | |
| `artifact_info`, `search_artifact` | read | no | |
| `memory_save`, `memory_forget` | write | no | |
| `memory_search`, `memory_get` | read | no | |
| `find_capabilities`, `activate_capability` | control | no | reached through `search_tools` |
| `request_shell_permission` | shell | no | |
| `computer_use`, `computer_use_reset` | control | no | hidden when the runtime is unavailable |
| `context_status`, `new_context`, `context_history` | control | no | defined in `agent/context.rs` |
| agent-group tools | control | when a group is attached | `agents/group/*commands.rs` |
| `list_files` | read | never | execution-only compatibility path |

Skills, MCP servers and Workers add tools at activation time through the
catalog maps; they are not part of this table.

## Where a tool's contract is defined

Before this audit a built-in tool was described in up to ten places, each
keyed by a string match on the tool name:

| Concern | Location |
| --- | --- |
| Name, description, JSON schema | `Harness/tools/definitions.rs::base_tool_definitions` |
| Owning capability (settings toggle) | `definitions.rs::BUILTIN_CAPABILITIES[*].tools` |
| Write/shell prohibition for scoped agents | `tools/catalog.rs::execution_prohibited` |
| Research (read-only) allowance | `tools/catalog.rs::research_tool_allowed` |
| Default attachment and its order | `agent/tool_discovery.rs::BASIC_TOOLS` |
| Execution | `Harness/tools.rs::execute_gated` |
| Progress title while running | `service/transport.rs::tool_activity_title` |
| Completion / failure title | `service/events.rs` (inline match) |
| Transcript title and icon | `UI/conversation/tools.rs::project_tool` (two matches) |
| Compact verb, TUI task rows | `UI/conversation/compact.rs` (two tables, different casing) |
| TUI glyph | `Platforms/tui/ui/views/sessions/tools.rs::tool_icon` |
| Rich result shape | `UI/conversation/rich.rs::project` |

The same tool therefore had four vocabularies: `run_shell` was "Running
command" (progress), "Ran command" (completion), "Command" (transcript) and
"Ran Shell" / "Running Shell" (compact). `compact.rs::tool_action_label` and
`definitions.rs::is_coding_builtin_tool` / `is_general_builtin_tool` had no
callers.

## Where prompts are assembled

| Prompt | Location |
| --- | --- |
| System instruction (one sentence) | `agent/policy.rs::SYSTEM_INSTRUCTION` |
| Tool descriptions and schemas | `tools/definitions.rs`, `agent/context.rs::tools`, `agent/tool_discovery.rs` (`search_tools`), `agents/group/*commands.rs` |
| Per-turn overlays (global/project instructions, memory, agent-group guidance, goal) | `agent/turn_setup.rs` |
| Runtime capability line | `agent/coordinator_support.rs::capability_guidance` |
| Context orientation and rollover carry-over | `agent/context.rs` |
| Goal judge | `agent/goal.rs` |
| MCP-facing variants (`read_file`, `run_shell`, `web`) | `tools/definitions.rs::direct_mcp_tool_definitions` |

Tool order is part of the prompt-cache ABI: the default surface is attached in
a fixed order and later tools are appended, never reordered.

## Where docs and code disagree

1. `Extending the Harness.md` says a direct tool needs a `ToolDefinition` in
   `Harness/tools.rs` plus an execution branch. Definitions live in
   `Harness/tools/definitions.rs`, and a new tool also needed the name tables
   listed above, none of which the doc mentioned.
2. The same doc says the agent uses `find_capabilities` then
   `activate_capability`. Both are outside the default surface; the model
   reaches them through `search_tools`.
3. `coordinator_support.rs::attached_web_search_capability_result` tells the
   model to attach `web_search` through `search_tools`, but `web_search` is in
   the default surface whenever web search is enabled.
4. The Computer Use capability text says "native macOS applications with
   Codex's installed Computer Use runtime"; the tool description says "macOS,
   Linux, and Windows through the installed Open Computer Use runtime".
5. `REMOTE_PROTOCOL.md` lists tool statuses as `streaming`, `completed`,
   `failed`. The wire enum is `preparing`, `awaiting_permission`, `running`,
   `completed`, `failed`, `cancelled`, `interrupted`, `timed_out`,
   `suppressed`.
6. `REMOTE_PROTOCOL.md` does not describe the per-client UI channels
   (`ui_state`, `ui_conversation`, `ui_composer`, `ui_settings`, `ui_agents`,
   `ui_home`, `ui_diff` and their `*_action` commands) that
   `MULTIPLATFORM_MIGRATION.md` documents and both clients depend on.
7. `TUI_TOOL_AUDIT.md` and `HARNESS_TOOL_CONTRACTS.md` reference
   `cargo test --test source_layout`. The file is `Tests/source_layout.rs`;
   Cargo discovers `tests/` only, so on a case-sensitive filesystem (Linux,
   CI) the target does not exist and the size guard never runs.
8. `FRONTEND_ARCHITECTURE.md` and the root `AGENTS.md` describe the web client
   generically; it is a Mün application (`web/src/app/*.mun`). The React
   client under `web/src/components` was still present but unmounted.
9. `app/ui/remote/CompatibilityRemote.ui` promises "State: only active
   progress and consequential exceptions" and per-file diff counts as design.
   The Rust projection computed these (`TraceView::attention_label`,
   `DiffFile::added/removed`, `ActivityGroup::single_trace`) but did not
   serialize them, so `web/src/app/Chat.mun` and `toolPresentation.ts`
   re-derived them with regular expressions and counting loops.

## Decisions

Each decision is implemented in its own commit; rationale is kept here because
Foundation Memory was not reachable from this session.

- **One spec row per built-in tool** (`Harness/tools/spec.rs`). Name, access
  class, default attachment, research allowance, semantic kind and the four
  labels live in one `const` table. The catalog, discovery, progress events
  and the shared UI projection read it. It is a table and a lookup, not a
  registration framework: schemas and execution stay where they were.
- **Access class drives policy.** `deny_writes` blocks `write` tools and
  `deny_shell` blocks `shell` tools, instead of two hand-kept name lists.
- **One label vocabulary.** A tool has a noun title ("Command"), an active
  label ("Running command"), a done label ("Ran command") and a failed label
  ("Command failed"). Transcript titles are unchanged so existing TUI and web
  assertions hold; the compact/task vocabulary now matches the progress one.
- **Icons follow the semantic kind**, so the TUI glyph and the web icon cannot
  drift per tool name.
- **Derived presentation is serialized.** Attention, display label/context,
  detail availability, diff counts and terminal/source captions are fields of
  the Rust projection. The web client renders them and no longer parses
  status copy.
- **Not changed:** shell execution, permissions, background-job behavior,
  storage formats, provider authentication, and Remote compatibility
  guarantees. All protocol additions are optional fields.
