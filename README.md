# Yeet

Version 0.1.0.

Yeet is a SwiftPM package containing both the Swift call-core library and the `yeet` executable. The TypeScript provider/auth/Skill/MCP bridge is shipped as a SwiftPM resource, so a normal build does not require npm or TypeScript.

Runtime requirement: Node.js 20 or newer. `YEET_NODE` can point to a specific Node executable.

## Build

```sh
swift build
swift test
swift run yeet doctor
```

The package depends on [SwiftTUI](https://github.com/SwiftTUI/swift-tui) for terminal UI support.

## Layout

```text
Package.swift
Sources/
  HarnessCallCore/
    Resources/Runtime/       prebuilt JS bridge runtime
    CallCoreClient.swift
    BundledBridge.swift
    ...
  Yeet/
    Agent/                    runtime, coordinator, direct tools, tool-call assembly
    YeetApp.swift
    CLI/YeetCLI.swift
    Utilities/ConfigStore.swift
RuntimeSource/               editable TypeScript source
  src/edit-backend/          snapshot-safe transactional file editing backend
Scripts/
  rebuild-runtime.sh
  check.sh
  install-local.sh
Tests/
```

## Model IDs

Public call APIs accept exactly one model identifier in `provider/model` form. The first slash separates the provider from the provider-specific model name.

Examples:

```text
openai/gpt-5
anthropic/claude-sonnet-4-5
gemini/gemini-2.5-pro
openrouter/anthropic/claude-sonnet-4-5
local/qwen3-coder
```

Swift:

```swift
import HarnessCallCore

let core = try await CallCoreClient.startBundled()
let result = try await core.complete(
    .init(model: "openai/gpt-5", messages: [.user("hello")])
)
print(result.text)
try await core.shutdown()
```

Streaming:

```swift
for try await event in core.stream(
    .init(model: "openai/gpt-5", messages: [.user("hello")])
) {
    if case .textDelta(let text) = event {
        print(text, terminator: "")
    }
}
```

## State directory

By default all user state is kept under:

```text
~/.yeet/
  config.json
  credentials.json
  mcp.json
  skills/
```

Use `YEET_CONFIG_DIR` to override this location, which is also useful for tests.

The config directory is created with mode 0700 and credential/config files are kept at mode 0600 where POSIX permissions are available.

## Authentication

API key:

```swift
try await core.setAPIKey(key, for: "openai")
let status = try await core.authStatus(for: "openai")

let models = try await core.listModels(for: "openai")
// Provider-local names, for example ["gpt-5", "gpt-4.1"]
```

Browser auth:

```swift
try await core.loginInBrowser("openrouter")
```

Gemini OAuth can receive `BrowserLoginOptions` with client ID, client secret, project ID, scopes and timeout. Refresh tokens are reused by the bridge when available.

CLI examples:

```sh
printf '%s' "$OPENAI_API_KEY" | swift run yeet auth set-key openai
swift run yeet auth login openrouter
swift run yeet auth status
```

## Skills

Skills are discovered from `~/.yeet/skills/<name>/SKILL.md`. Discovery loads only metadata; full instructions and supporting files are loaded on demand.

```swift
let summaries = try await core.listSkills()
let skill = try await core.loadSkill("review-code")
let reference = try await core.readSkillFile("review-code", path: "references/checklist.md")
```

## MCP

Both stdio and Streamable HTTP servers can be stored in `~/.yeet/mcp.json`.

```swift
try await core.setMCPServer(
    .stdio(name: "workspace", command: "npx", args: ["-y", "some-mcp-server"])
)

let tools = try await core.listMCPTools(server: "workspace")
let result = try await core.callMCPTool(
    "workspace/search",
    arguments: ["q": "runtime"]
)
```

Resources and prompts are supported through `listMCPResources`, `readMCPResource`, `listMCPPrompts` and `getMCPPrompt`.

## File editing backend

The TypeScript runtime includes a snapshot-safe file editing backend in
`RuntimeSource/src/edit-backend`. It confines paths to a workspace root,
tracks which lines were shown to the model, conservatively rebases stale
snapshots, and commits multi-file changes transactionally. The structured API
and `hashline`, unified `apply_patch`, and tolerant `sloppy` dialects are
exported from the runtime entry point.

```ts
import { EditBackend } from "./RuntimeSource/src/edit-backend/index.js";

const edits = new EditBackend({ root: process.cwd() });
await edits.initialize();
const file = await edits.read({ path: "Sources/example.swift", startLine: 1, endLine: 40 });
await edits.apply({
  changes: [{
    path: "Sources/example.swift",
    snapshot: file.snapshot,
    edits: [{ kind: "replace", range: { start: 12, end: 12 }, text: "let answer = 42" }],
  }],
});
```

The JSONL daemon is built as `dist/edit-backend/daemon.js` and accepts
`read`, `preflight`, `apply`, and `applyDialect` requests. The Swift
`YeetEditClient` in `Sources/HarnessCallCore/YeetEditClient.swift` speaks that
protocol when a frontend needs a native client.

Interactive sessions use a persistent `LiveHarnessRuntime` and an
`AgentCoordinator`. The coordinator keeps one model history across turns and
executes workspace tools directly. `DirectToolRegistry` discovers capability
metadata cheaply, then activates one Skill or MCP server on demand; the runtime
reuses one bridge and one edit daemon rooted at `workspaceRoot`. `ToolCallAssembly`
is shared by the lead stream and reconstructs delta-only provider calls.

## CLI

```text
yeet doctor
yeet model get
yeet model set provider/model
yeet run [--model provider/model] [prompt]
yeet auth status [provider]
yeet auth set-key provider
yeet auth login provider
```

The full command list is available from `yeet --help`.

## Editing the TypeScript bridge

Normal Swift builds use the prebuilt resource and do not invoke npm. If the TypeScript runtime is modified:

```sh
cd RuntimeSource
npm install
cd ..
./Scripts/rebuild-runtime.sh
```

`npm install` supplies the TypeScript compiler and Node type declarations used by the source build; the runtime has no production npm dependencies.

Run the complete verification suite with:

```sh
./Scripts/check.sh
```

The package version is intentionally kept at 0.1.0. For Git distribution, tag the repository as `0.1.0` or `v0.1.0`.
