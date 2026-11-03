# Yeet memory system plan

Status: proposal, not implemented. This document plans core behavior and API contracts, not UI layouts.

## 1. Product contract

Build two independently enabled, text-embedded memory systems sharing storage, retrieval and privacy enforcement:

1. **Explicit memory**: the agent deliberately saves durable knowledge: preferences, project decisions, constraints, discoveries and validated fixes. The user can also inspect, correct and delete it.
2. **Whole-session memory**: Yeet automatically indexes committed conversation content across context windows and sessions, without requiring the agent to decide what deserves remembering. Summaries help navigation; source excerpts remain retrievable.

“Whole-session” describes capture coverage, not access scope. Session history is searchable across sessions in the same workspace by default; searching other workspaces requires explicit permission. Explicit memory defaults to workspace scope, with an intentional user-global scope for preferences. Do not silently promote project knowledge to global knowledge.

Both systems have independent `enabled` and `localOnly` settings. Interpret Local-only as a privacy guarantee, not merely “stored on disk.” All memory storage is local in this proposal, even when cloud embedding is chosen.

### Local-only contract

- Expose the Local-only control only for trusted local inference configurations, including supported Ollama and llama.cpp configurations. Do not expose it for cloud providers.
- A protected memory is captured, embedded, queried, summarized and consumed only through local execution. Cloud providers cannot retrieve it, receive it as prompt context or process it through cloud embedding/reranking/summarization.
- Turning Local-only off permits future shared memories; it does **not** declassify existing protected records. Declassification is a separate explicit user action with a preview of what can be sent externally.
- Local-only settings remain remembered but inactive when using a cloud provider. On a cloud provider, new memories belong to the shared partition if that memory system is enabled; existing local-only memories remain inaccessible.
- Do not automatically duplicate a protected record into the shared partition. Derived records inherit the strictest privacy of their sources.
- An OpenAI-compatible API, provider name, loopback URL or label such as “local” alone is not proof of local execution. A local proxy can forward requests to a cloud backend. Unknown/custom endpoints are untrusted for protected memory until explicitly configured as trusted local; document that this is a user trust assertion, not remote attestation.

## 2. Separate the decisions

Keep these independent:

| Decision | Values |
| --- | --- |
| Memory kind | Explicit / whole-session |
| Access scope | Workspace / user-global; source-session filter where applicable |
| Privacy partition | Shared / local-only |
| Embedding execution | Cloud adapter / Yeet-managed local model |
| Consumer | Current generation provider and its trusted execution classification |

Supported combinations:

| Memory privacy | Embedding | Consumer |
| --- | --- | --- |
| Shared | Cloud | Cloud or local |
| Shared | Yeet-managed local | Cloud or local |
| Local-only | Yeet-managed local only | Trusted local only |

Choosing local embedding does not make shared memory local-only: its retrieved text may still be sent to a cloud generation provider. Conversely, choosing a local generation model does not prevent cloud embedding unless Local-only is enabled. Explain both distinctions wherever settings are eventually designed.

If a cloud embedding profile is selected when Local-only is enabled, require a managed local profile before indexing; show a blocked/pending state until it is ready. Never silently fall back to cloud execution. Model downloads require network access, but contain no conversation text; offline inference begins after installation. Local-only is not a promise that unrelated agent tools have no network access.

## 3. Fit the existing architecture

Reuse these existing boundaries rather than creating a frontend memory engine:

- `Harness/agent/context.rs`: exact context-window archives, crash-safe rollover and active-window manifest. These stay authoritative and are not replaced with lossy vector memory.
- `Harness/session_store.rs` and its event/object modules: durable session identity and committed history. Index only committed data, not streamed partial responses.
- `Harness/agent/history.rs` and `request_assembly.rs`: append-only context/cache behavior. Append bounded retrieval results at turn boundaries; do not rewrite an existing submitted prefix to update memories.
- `Harness/tools/catalog.rs`, `definitions.rs` and domain executors: agent memory tools, authority checks and permissions.
- `RuntimeSource/src/embeddings.ts`, provider `embed` methods, and `bridge-protocol.ts`: existing cloud transport and embedding bridge operations. OpenAI/Gemini adapters and OpenRouter's OpenAI-compatible inheritance are transport building blocks, not proof that every configured model or credential supports embeddings.
- `src/project_settings.rs`, Harness settings/model contracts and the existing Remote protocol: authoritative configuration and frontend-neutral status. Frontends present state and commands only.

Proposed Rust boundary: `Harness/memory/` with store, policy, ingestion/chunking, retrieval, embedding adapters and managed-model lifecycle modules. Keep coordinators small. Cloud adapters delegate external I/O to the existing provider bridge. Yeet owns local inference separately from chat-provider processes; use a supervised worker if native dependencies warrant isolation.

Foundation MCP remains an external integration, not the storage dependency for this native system. Do not silently ingest its contents or replace user instruction files such as `AGENTS.md`.

## 4. Storage and record lifecycle

Use a local SQLite store for canonical memory text, metadata, FTS and durable indexing jobs. Vectors are a rebuildable index, never the only copy of knowledge. Start with bounded, partition-filtered exact vector search plus FTS; adopt a packaged ANN/vector extension only when measured corpus size/latency justifies it. Do not require a separate vector server.

Use separate shared and local-only stores/index namespaces to reduce accidental mixing, plus policy checks at every access boundary. Private file permissions are required; encryption at rest is a separate follow-up, not an implied guarantee of Local-only. Store outside versioned workspaces, keyed by stable workspace identity, following Yeet's existing application-data conventions. Avoid synchronized/exported directories by default.

Minimum record data:

- Stable ID, kind, scope, workspace ID, privacy partition, timestamps and schema revision.
- Canonical text and content hash; optional title/tags.
- Source session/window, stable event/message range and revision; artifact references when relevant.
- Status (`active`, `superseded`, `deleted`), explicit supersession links and source provenance.
- For vectors: embedding profile fingerprint, dimensions, text/preprocessing revision and embedding status/error.

Persist source changes and an outbox entry atomically in the memory store. Session writes and this store are not one transaction: reconcile committed session revisions to create missing jobs after a crash. Jobs are idempotent using source revision, chunker version, content hash and embedding fingerprint. No network request while holding session-state locks.

Corrections supersede explicit records rather than silently merging contradictory claims. Similarity alone is not evidence that two facts are equivalent. Index deletion/tombstones immediately exclude stale results, with asynchronous physical cleanup. Source-session deletion cascades to session chunks and derived summaries; explicit facts linked to the source require a defined cascade/retain choice, defaulting to deletion. Check tombstones again before prompt assembly to fence deletion races.

Deleting a memory removes local text/vector/FTS/cache copies according to retention settings. Explain that deletion cannot retract content already sent to a cloud provider or erase external backups. Whole-session indexing does not authorize indefinite raw-session retention; retain existing session policy and cascade its deletions.

## 5. Explicit memory flow

Provide a small proposed tool surface, names subject to the existing tool contract:

- `memory_save`: text, workspace/global scope, optional tags and `supersedes` IDs. Kind is always explicit; privacy is assigned by trusted runtime policy, not an agent-supplied downgrade.
- `memory_search`: query plus permitted kind/scope/source filters and a bounded limit.
- `memory_get`: fetch exact text and source references for permitted IDs.
- `memory_forget`: deletion under normal mutation/approval policy; bulk deletion requires user confirmation.

Encourage saving stable decisions, reusable discoveries and explicit user preferences, not every tool result or speculative inference. Capture source attribution automatically. Secrets, credentials and authentication payloads are excluded; configurable sensitive-file/content rules apply before storage and embedding. Detection is defense in depth, not a guarantee that arbitrary prose contains no secrets.

Commit canonical text first, then embed asynchronously. A failed embedding leaves a visible pending record with lexical search available in its allowed partition. Do not make successful knowledge capture depend on provider uptime. New explicit saves are returned to the agent with their ID and indexing state.

## 6. Whole-session capture and cutting policy

Preserve the exact transcript in the existing session/context stores. Embed a normalized, provenance-linked text view, not raw protocol JSON or hidden model reasoning. Exclude credentials, auth/debug payloads, request-only runtime overlays, duplicate boilerplate, binary data and ephemeral streaming deltas. Record indexing gaps/exclusions rather than claiming every byte was embedded.

Use three layers:

1. **Episodes**: user request through its completed assistant/tool exchange, partitioned further at explicit task changes, context-window boundaries or privacy/authority changes. In-progress tool-call groups are never treated as complete episodes.
2. **Leaf chunks**: retrieval-sized text within each episode. Initial target: 800 tokens, maximum 1,200 tokens or the embedding model's smaller safe input budget; approximately 100 tokens of overlap only within an episode. Tune from retrieval evaluation, not as fixed product guarantees.
3. **Episode/window synopses**: optional navigation records linking to leaves. Generate only with a privacy-compatible model and bounded budget. Initial implementation may omit model-generated summaries; never substitute a summary for complete leaf coverage.

Cut in this order: completed conversational/tool group, message, paragraph, sentence, then tokenizer-safe split for an oversized block. Keep a tool call's name/arguments/result association in metadata and concise textual headers, even when a large result spans multiple chunks. Do not overlap across privacy boundaries. Avoid repeating huge source blobs: index useful textual content with stable artifact spans and retrieve details on demand, under the same permissions/privacy policy.

Index after completed turns, at durable rollover and at session close; periodically checkpoint stable completed exchanges during long runs. A monotonically advancing source cursor plus reconciliation covers crashes and missed hooks. Revisions from edit/regenerate create a new source revision and invalidate obsolete derived chunks; do not merge incompatible branches. Optional indexing of older sessions is an explicit local operation, with a separate disclosure/confirmation if their text will be cloud-embedded.

“Global” does not mean automatically inject all past sessions. Retrieval is scoped to the current workspace by default and obeys per-source permissions. User-global explicit preferences are a separate lane. Cross-workspace session search is opt-in.

## 7. Retrieval and context insertion

1. Determine the current provider trust, effective memory settings, allowed workspace/global scopes and authorized partitions **before** any query embedding or external call.
2. Build a bounded query from the current user request and allowed recent context. Never send a protected query to a cloud embedding profile, even if searching only shared documents. A tainted local session uses local query embedding or lexical search.
3. Run partition-filtered FTS and vector retrieval in matching embedding spaces. Fuse ranked lists, deduplicate overlap, and expand neighboring chunks only when needed. Do not compare raw similarity scores across models/spaces.
4. Prefer active explicit facts for stable preferences/decisions; use source chronology and supersession to resolve changes. Session excerpts are historical evidence, not automatically current truth. Show conflicting evidence rather than inventing a merged fact.
5. Enforce a bounded injection budget (initial target 2,000 tokens, capped by available context budget). Include memory IDs, source references, dates and provenance. Use `memory_get` or existing context-history access for further detail.
6. Append a labeled memory-data block at a turn boundary. Retrieved text is untrusted reference material, never a system instruction or authorization to execute tools. Preserve the existing append-only submitted-prefix invariant.

Expose indexing lag, blocked profiles, retrieval sources and failures through core status; cloud failures degrade to allowed lexical retrieval, never another provider without consent. Cancellation and queue backpressure must not block interactive chat or rollover persistence.

### Provider switching is a privacy boundary

Filtering search results alone is insufficient. Local-only excerpts may already be in the current history, tool results, summaries or derived assistant messages.

Track protected provenance/taint through retrieval and subsequent generated context. Before switching to a cloud provider, block dispatch if retained history has local-only provenance. Offer continuation in the local session or a fresh cloud-safe branch without protected/derived context. Do not silently summarize that history with a cloud model, remove only the original excerpt, or treat ordinary context rollover as declassification. All request paths, including retries, background titles/summaries, workers and compaction, must apply the same egress policy. Whole-session ingestion inherits source taint instead of following only the newly selected toggle.

This protects Yeet-managed memory flows; it cannot identify private material a user independently pastes into an unrelated session. Be explicit about that limit.

## 8. Embedding backends

### Cloud

Reuse the existing embedding bridge and provider credentials, with a separately selected embedding profile rather than assuming the chat model can embed. Support OpenAI (for example `text-embedding-3-large`), Gemini embedding endpoints and supported OpenRouter embedding models. Treat the requested Gemini Embedding 2 name and all catalog availability/API IDs as candidates to validate against the provider before shipping; do not hardcode marketing names as working endpoints. OAuth/chat subscriptions do not necessarily grant embedding API access.

Validate model support, dimensions, batch/token limits and query/document input modes. Extend the bridge request shape for task types/instructions or dimensions where required; current `model` + `input` transport alone does not express every embedding model's contract. Preserve cancellation, retry/backoff and provider usage reporting. Ensure embedding payloads are not written to ordinary API debug logs.

### Yeet-managed local

Yeet downloads, verifies, loads and runs a model independently of Ollama/llama.cpp chat servers. Local embedding must work when no chat-provider daemon is running.

Use a pinned model manifest: repository/revision, artifact names, checksums, license, supported runtime/format, tokenizer, pooling, normalization, query/document instructions, dimensions, token limit and expected resource footprint. Candidate: the requested Nemotron 3.5 Embedding 0.6B, subject to verification of exact published model ID, license, artifacts and runtime support. Do not commit to a GGUF/ONNX conversion or engine until verified artifacts pass a quality/resource spike; a different supported embedding model may be the first shipped option.

Provide resumable downloads, disk-space checks, cancellation, atomic installation, checksum verification and removal. Download only on user approval; support preinstalled/offline artifacts. Verify artifacts without executing repository-supplied code. Separate model installation from loading; bound worker concurrency, memory and idle lifetime. Prefer available acceleration with an explicit CPU fallback; report unsupported devices or load failures rather than falling back to cloud. Validate on macOS arm64, Linux and Windows before declaring platform support.

### Embedding identity and migration

Fingerprint provider/endpoint identity or pinned local artifact, resolved model/revision, dimensions, normalization, pooling, query/document instructions and preprocessing version. Same dimensions do not imply compatible vectors. Keep profile-specific indexes and query each with its matching encoder. A model change starts resumable reindexing; atomically activate a completed generation, retain the old index until cutover, and expose progress. Local-only data never participates in a cloud reindex. Endpoint/model aliases that change without a stable revision need explicit invalidation/versioning policy.

## 9. Delivery checkpoints

1. **Contracts and technical spike**: settle provider trust/egress metadata, settings, schemas, deletion semantics and exact local model/runtime compatibility. Verify existing cloud embedding APIs with supported credentials and review logging. No UI design work.
2. **Store and explicit memory**: local canonical text, privacy partitions, scopes, FTS, lifecycle tools and policy enforcement. Add local model installation/inference and one verified cloud adapter; embedding jobs remain resumable. Ship explicit semantic retrieval once both lanes are validated.
3. **Whole-session ingestion**: durable cursors, chunking/provenance, crash reconciliation, edit/regenerate and retention handling. Index new opted-in sessions first; backfill is separately authorized.
4. **Retrieval and session safety**: hybrid search, bounded append-only insertion, source citations and protected-history provider-switch enforcement across all request paths. Keep cross-session auto-retrieval gated until these protections pass.
5. **Product integration**: expose settings/status/management through core and Remote contracts; implement frontend controls only after the user supplies or requests the design. Tune chunking and retrieval with a small representative corpus, then consider summaries/ANN indexes.

Meaningful commits should separate contracts/store, embedding execution, ingestion and retrieval integration. Do not ship the intermediate storage steps as a claim of end-to-end Local-only safety.

## 10. Focused acceptance checks

Keep tests small and behavior-driven:

- **Privacy/egress**: a transport spy sees no protected document/query/context in cloud embeddings, generation, retries or background calls; changing a toggle or switching providers cannot downgrade existing taint. Cover unknown “local” endpoints and cloud fallback rejection.
- **Capture integrity**: completed source groups are covered exactly once after reconciliation; interrupted tool exchanges, crashes, rollover and edit/regenerate do not drop coverage or return stale revisions.
- **Scope/lifecycle**: unrelated workspaces and protected IDs cannot be fetched by direct ID or vector search; superseded/deleted records and their derived indexes stop appearing immediately.
- **Embedding compatibility**: pin a tiny fixture per supported profile for query/document formatting, dimensions and preprocessing; a profile change cannot mix vector spaces. Managed inference works without an external local provider and offline after installation.
- **Retrieval quality**: a small fixed corpus includes exact preferences, paraphrased decisions, tool outcomes, conflicting old facts and overlapping chunks. Check relevant sources fit the context budget without rewriting the submitted prefix.

Open validation gates: exact Nemotron/Gemini model identifiers and availability; managed inference runtime/artifact compatibility; acceptable download/RAM/latency budgets by platform. These need evidence during implementation, not assumed support in this plan.
