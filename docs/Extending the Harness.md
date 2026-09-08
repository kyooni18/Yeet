# Extending the Harness

Yeet has three extension layers. Keep them separate so optional functionality
does not enlarge every model request.

## Provider request capabilities

The TypeScript runtime owns provider-facing request transforms. Their metadata is
returned by the `list-harness-capabilities` bridge operation. Interactive users
choose the per-session attachment set with `/capabilities`, `/attach`, and
`/detach`.

Add a provider request capability under `RuntimeSource/src/`, register it in the
runtime capability registry, add Node tests, and rebuild with:

```sh
./Scripts/rebuild-runtime.sh
```

No Rust tool schema is needed for a request transform because the model never
calls it directly.

Vision is one such request capability. Image bytes live on the provider-neutral
`Message.images` field and provider adapters are responsible for translating
that field to their native multimodal request format. Keep local file paths out
of provider payloads.

## Direct tools

The base tool surface lives in `src/tools.rs`. A direct tool needs:

1. a `ToolDefinition` in `base_tool_definitions()`;
2. an execution branch in `ToolRegistry::execute()`;
3. a bounded result contract;
4. tests for validation, repeat suppression, or state mutation as appropriate.

Repository reads should go through the edit daemon so snapshots and hash-line
anchors remain consistent with transactional edits. Large results should be
stored as artifacts instead of returned inline.

Mutating shell behavior belongs in `src/shell.rs` and must stay behind the
exact-command permission path. Never add an unsandboxed fallback.

## Lazy Skills and MCP

Skill and MCP metadata is discovered cheaply through the Node bridge. The lead
uses `find_capabilities`, followed by `activate_capability` for one exact
capability. Activation adds only that Skill's file reader or that MCP server's
tool schemas to subsequent lead rounds.

Keep generated tool names ASCII-only and collision-safe. Tool protocol errors
should return structured errors to the lead rather than crashing the TUI.

## Rust Workers

Workers implement `HarnessWorker` in `src/workers.rs`:

```rust
pub trait HarnessWorker: Send + Sync {
    fn descriptor(&self) -> WorkerDescriptor;
    fn tools(&self) -> Vec<ToolDefinition>;
    fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
        workspace_root: &std::path::Path,
    ) -> anyhow::Result<String>;
}
```

Worker discovery is metadata-only. Register workers when constructing the
`WorkerRegistry`; the model receives their tool schemas only after explicit
activation.

## Provider bridge

`src/core.rs` implements the Rust side of the persistent JSONL protocol in
`RuntimeSource/src/bridge.ts`. New wire operations must be implemented and
tested on both sides. Keep protocol version 1 backward-compatible unless a
coordinated version change is required.

The client supports concurrent requests by request id, streamed model events,
explicit cancellation, auth/provider management, Skills, MCP, and model
metadata. stderr is diagnostic-only and must never carry secrets.

## Transactional edit bridge

`src/edit.rs` speaks to `RuntimeSource/dist/edit-backend/daemon.js`. Existing
file writes require the snapshot produced by a preceding read. Preserve this
contract when adding edit dialects or operations.

## Testing

Run the complete suite with:

```sh
./Scripts/check.sh
```

For a Rust-only change, `cargo test` is the fast path. For bridge/edit/provider
changes, rebuild and run the Node suite as well.
