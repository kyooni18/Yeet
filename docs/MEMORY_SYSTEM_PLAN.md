# Yeet memory system plan

Status: implementation in progress; see checkpoint log below. Exact model/runtime compatibility remains subject to the validation gates below. This document specifies functional behavior and core/API contracts, not UI layouts; requirements are not claims of shipped functionality.

## Executive summary

- Two independent memory systems: agent-generated explicit knowledge and automatically indexed whole-session history.
- Canonical memory text and indexes live in local storage. Cloud embedding is an explicitly selected processing service, not cloud memory storage.
- Reusable embedding profiles: initially one shared profile for both systems and one Yeet-managed local profile for protected data. Chat-model selection never silently selects an embedding profile.
- Yeet downloads and runs validated local embedding artifacts independently of chat-provider daemons.
- Automatic bounded recall at user-turn/rollover boundaries plus explicit agent search/read tools.
- One manager accessible via `/memories` and `/settings`: browse, edit, delete, privacy, setup and indexing operations.
- Rust owns memory policy, lifecycle, retrieval and authority; existing provider/Remote/frontend boundaries remain authoritative.
- No silent cloud fallback, declassification, historical backfill or bulk export. Protection covers queries, derived context and background requests, not just document embeddings.

Sections 1–9 define product/architecture; 10–11 define delivery/acceptance; 12–18 specify setup, execution, recall, contracts, recovery, rollout and implementation tasks.

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

## 9. Memory management view

Provide one memory management destination, accessible through both entry points:

- `/memories` opens memory management directly.
- `/settings` exposes the same destination and memory configuration, not a separate copy of the manager or its state.

Treat these as user navigation commands, not prompts submitted to the model. Both entry points use the same core commands, permissions and authoritative state. This specifies functional requirements only; visual structure and layout remain subject to the user's design.

### Browse and inspect

- List and search both explicit and whole-session memories, with pagination and filters for kind, workspace/global scope, source session, privacy and lifecycle/indexing status.
- Show text, tags, dates, privacy, embedding profile/status and provenance; allow opening permitted source excerpts or sessions.
- Show pending/failed indexing, blocked local-model setup, download/reindex progress and retry actions. Keep canonical text inspectable even when embeddings are unavailable.
- Management search is local by default (metadata/FTS), so opening or searching the view does not silently send its query or memory text to a cloud model. Protected content is visible only through an authorized management client; a cloud generation provider never receives it. Management authorization is separate from model/tool visibility.

### Edit and delete

- Edit explicit memory text/tags and permitted scope, saving a new revision and re-embedding changed text. Exclude obsolete text/vectors immediately; use optimistic revision checks to avoid overwriting concurrent changes.
- Allow correction of whole-session memory through a durable edited override linked to its source, without rewriting the exact transcript. Ingestion and reindexing must honor overrides and suppression markers so the original indexed text does not return. Identify edited excerpts as edited, not verbatim transcript evidence; explain that the original transcript remains until separately deleted.
- Delete individual records, selected records or a filtered set with a clear confirmation for bulk/destructive actions. Explain whether only memory, its derived records or source sessions are affected. Removing a session chunk leaves a suppression marker so reconciliation cannot silently recreate it; source-session deletion remains a separate explicit action.

### Settings and privacy actions

- Independently enable/disable explicit memory and whole-session memory. Disabling stops capture and retrieval for that kind but retains stored data; deletion is separate.
- Provide each kind's Local-only default plus per-record or selected-record privacy changes. Expose Local-only controls only with a trusted local generation configuration, as specified above. On cloud configurations, show existing privacy labels and a read-only explanation instead of a Local-only toggle.
- Making a shared record local-only immediately excludes it from shared retrieval and cancels/fences queued cloud jobs; propagate protection to retained derivatives, move it into the protected partition and rebuild with local embeddings if needed. Warn that prior cloud disclosure cannot be undone and already dispatched requests cannot be recalled.
- Making a protected record shared is explicit declassification, not an ordinary unconfirmed switch. Preview the affected content and derivatives, require user confirmation and retain protection on anything not explicitly authorized. Do not mark an already tainted conversation cloud-safe merely because a record changed privacy.
- Offer embedding-profile selection, managed local-model download/removal, indexing retry/rebuild and retention/backfill controls. Require disclosure before any action that newly sends existing text to cloud embedding. All operations enforce policy in Rust, not only through disabled frontend controls.

Expose frontend-neutral paginated list/get/update/delete, settings, privacy-change preview/confirm and job/status contracts through the existing core/Remote boundaries. These are user management operations, not agent tools with unrestricted administrative authority. Remote clients must have explicit management authorization before receiving protected content; merely selecting a local chat provider does not authorize an attached client. Avoid memory text in navigation parameters, ordinary telemetry or logs.

## 10. Delivery checkpoints

1. **Contracts and technical spike**: settle provider trust/egress metadata, settings, schemas, deletion semantics and exact local model/runtime compatibility. Verify existing cloud embedding APIs with supported credentials and review logging. Implement egress guards before enabling protected content in model requests. No UI design work.
2. **Store and explicit memory**: local canonical text, privacy partitions, scopes, FTS, lifecycle tools and policy enforcement. Add local model installation/inference and one verified cloud adapter; embedding jobs remain resumable. Validate explicit semantic retrieval behind a feature gate; do not release protected recall before the full session-safety checkpoint.
3. **Whole-session ingestion**: durable cursors, chunking/provenance, crash reconciliation, edit/regenerate and retention handling. Index new opted-in sessions first; backfill is separately authorized.
4. **Retrieval and session safety**: hybrid search, bounded append-only insertion, source citations and protected-history provider-switch enforcement across all request paths. Keep cross-session auto-retrieval gated until these protections pass.
5. **Product integration**: expose settings/status/management through core and Remote contracts, including the shared memory management destination accessible via `/settings` and `/memories`. Implement frontend controls against the user's supplied/requested design. Tune chunking and retrieval with a small representative corpus, then consider summaries/ANN indexes.

Meaningful commits should separate contracts/store, embedding execution, ingestion and retrieval integration. Do not ship the intermediate storage steps as a claim of end-to-end Local-only safety.

## 11. Focused acceptance checks

Keep tests small and behavior-driven:

- **Privacy/egress**: a transport spy sees no protected document/query/context in cloud embeddings, generation, retries or background calls; changing a toggle or switching providers cannot downgrade existing taint. Cover unknown “local” endpoints and cloud fallback rejection.
- **Capture integrity**: completed source groups are covered exactly once after reconciliation; interrupted tool exchanges, crashes, rollover and edit/regenerate do not drop coverage or return stale revisions.
- **Scope/lifecycle**: unrelated workspaces and protected IDs cannot be fetched by direct ID or vector search; superseded/deleted records and their derived indexes stop appearing immediately.
- **Management**: `/settings` and `/memories` reach the same authoritative manager without model dispatch; edits reject stale revisions, deleted/overridden chunks stay suppressed after reindex, and privacy changes fence queued cloud jobs and require explicit declassification. Unauthorized management clients cannot read protected text.
- **Embedding compatibility**: pin a tiny fixture per supported profile for query/document formatting, dimensions and preprocessing; a profile change cannot mix vector spaces. Managed inference works without an external local provider and offline after installation.
- **Retrieval quality**: a small fixed corpus includes exact preferences, paraphrased decisions, tool outcomes, conflicting old facts and overlapping chunks. Check relevant sources fit the context budget without rewriting the submitted prefix.

Open validation gates: exact Nemotron/Gemini model identifiers and availability; managed inference runtime/artifact compatibility; acceptable download/RAM/latency budgets by platform. These need evidence during implementation, not assumed support in this plan.

## 12. Setup and embedding-profile selection

### Defaults and configuration ownership

- On upgrade, both memory systems start disabled. No automatic backfill, model download or embedding traffic.
- Enable either or both kinds independently. Whole-session capture requires its own disclosure because it processes substantially more conversation text than explicit saves.
- Store profiles and installations at application/user level, outside committed project files. Reference existing provider credentials; do not copy API keys into memory records/configuration.
- Workspace configuration references profiles and stores enablement/scope/retention choices. User defaults may initialize new workspaces without changing existing workspace choices. Workspace files cannot grant cloud consent, endpoint trust or cross-workspace access by themselves.
- Each kind remembers its Local-only default, effective only under a trusted local generation configuration. Source taint always wins. Expose configured and effective state separately.
- Disabling a kind stops new capture, recall and undispatched embedding work for it. Retain records and park pending jobs until re-enabled/deleted. Management remains available.

Conceptual contract, to integrate into existing Rust/Remote schemas:

```text
MemorySettings
  revision
  explicit: { enabled, localOnly, sharedProfileId?, localProfileId? }
  wholeSession: { enabled, localOnly, sharedProfileId?, localProfileId? }
  retrieval: { automatic, maxInjectedTokens, defaultWorkspaceScope }
  retention: { sourcePolicyReference, memoryExpiry? }

EmbeddingProfile
  id, revision, displayName
  execution: cloud | managed-local
  providerConfigurationRef? / managedModelManifestRef?
  modelId, requestedOptions, resolvedCapabilities
  fingerprint, validationStatus, lastValidationError?
```

Initially offer one reusable shared profile and one managed local profile, inherited by both kinds. Profile references allow later per-kind assignments without a storage redesign; advanced multi-profile configuration need not complicate first setup. Shared records may use local embedding; protected records must.

### Cloud selection workflow

1. Choose an existing provider/account or configure a supported endpoint through existing provider settings.
2. Show curated embedding models plus provider discovery where reliable, not the entire chat-model catalog. Existing bridge name-based discovery/seeds are hints, not capability guarantees.
3. Allow custom model IDs for compatible endpoints, unvalidated until tested. Explain chat-only credentials and authentication failures.
4. Validate with a disclosed bundled non-sensitive probe, not real memories: query/document formatting, task types, counts, dimensions and finite values. Associate capabilities with the resolved profile revision.
5. Explain what text leaves the device, destination provider/endpoint and available cost/limit information. Require consent before real document/query embedding; backfill requires separate authorization. Do not invent price estimates when unavailable.
6. Mark ready only after validation. Incomplete setup leaves canonical text pending semantic indexing, with authorized local lexical search available.

Do not infer an embedding choice from the active chat model. No cross-provider fallback for rate limits, expired credentials or model unavailability: retry the authorized profile or ask the user to select another.

### Local selection workflow

1. Choose a pinned tested manifest from a small catalog. Show license, download/disk size, approximate RAM and supported devices; accept offline artifacts validated against the same manifest.
2. Obtain download approval, then install and verify model/runtime assets.
3. Complete preparation and fixture checks before declaring readiness. Show precise progress, errors, cancel and retry actions.
4. Reuse installation across workspaces. Load lazily; no dependency on Ollama/llama.cpp or a loaded chat model.

### Profile changes

Use immutable profile revisions. Changing encoder/endpoint/options creates a new index generation; refreshing credentials need not change identity if the encoder remains identical. Preview affected record counts and cloud egress before authorizing a rebuild. Protected records never enter a cloud migration batch.

Build in the background while the previous authorized encoder/index remains usable. If the old encoder disappears, use lexical retrieval, never new query vectors against old document vectors. Replay concurrent record mutations/deletions before atomic cutover; retire old generations after readers release them.

## 13. Managed local-model execution

### Runtime selection gate

Before choosing a production inference dependency, compare supported artifacts/runtimes for correctness, redistribution/license, package size, CPU behavior, acceleration and reproducible platform builds. GGUF, ONNX and other formats are candidates, not interchangeable guarantees. Pick one validated runtime path for v1.

The spike must produce a pinned manifest, query/document fixture and measurements of install size, peak RAM, cold load and warm throughput on reference hardware. Verify the requested Nemotron artifact first; if unavailable/incompatible, document the result and disclose an alternative rather than silently substituting under the same profile name.

### Installation/load states

| State | Contract |
| --- | --- |
| Not installed | No semantic inference; user may install/import |
| Checking requirements | Runtime/platform, license, disk and memory estimates |
| Downloading / paused | Temporary files, resumable only for matching artifact revision |
| Verifying | Sizes/hashes and complete artifact set; never load partial files |
| Preparing | Tokenizer/runtime and optional device cache; no repository-supplied executable code |
| Ready, unloaded | Valid installation, available offline |
| Loading / loaded | Supervised bounded worker serves requests |
| Failed | Typed error, retry or supported CPU fallback; never cloud fallback |
| Removing | Resolve leases, unload, remove selected assets and update dependent profiles |

Publish an installation atomically after verification and fixture validation. Verify runtime assets as well as weights. Bound extraction and reject path traversal/symlink escape. Partials/device caches have explicit cleanup rules and never count as ready installations.

### Worker and resource ownership

- Rust owns installation, authorization, queue admission and supervision. Use a private local worker channel, not an arbitrary publicly exposed inference port.
- Begin with one loaded encoder per worker and bounded queues. Prioritize interactive query embeddings over ingestion, with fair scheduling to avoid permanent ingestion starvation.
- Batch by model token/input limits. Separate query/document modes; apply tokenizer, pooling and normalization exactly as declared in the manifest.
- Cancellation removes undispatched work and fences late responses. Worker crashes produce retryable jobs with bounded restart/backoff; repeated failures surface an actionable error.
- Prefer supported acceleration; CPU fallback must preserve encoder semantics or create a separate compatible index generation. Use idle unload and bounded caches.
- Removal reports dependent profiles/workspaces and leaves canonical text intact. Their semantic jobs become blocked, not deleted.
- Installed inference needs no network. Advertise only validated platform/device combinations. Mobile clients may use an explicitly authorized desktop-hosted runtime; on-device mobile inference is a separate capability gate.

## 14. Recall execution and agent behavior

### Automatic recall algorithm

1. At an accepted user turn, capture settings revision, workspace, consumer trust, authorization and retained-context taint. Skip disabled kinds.
2. Build a bounded query from the current request and minimal permitted recent context. No cloud summarization of protected context; v1 needs no LLM query rewriting.
3. Search workspace explicit facts and session chunks, plus authorized user-global explicit preferences. Exclude unauthorized sources and current-turn duplicates before ranking.
4. Run FTS and matching-profile vector search. Initial candidate cap: 20 per permitted kind/profile lane with an overall bound. Fuse with reciprocal-rank fusion, not raw cross-model vector scores. Exact structured queries may use metadata/lexical paths alone.
5. Deduplicate overlapping spans and diversify session excerpts. Prefer active durable decisions/preferences appropriately, but do not treat them as verified current code state without evidence.
6. Expand neighbors only within the available budget. Cap injection at 2,000 tokens, reduced by remaining context capacity. Include IDs, dates, kind, edited/superseded indicators and sources, clearly delimited as untrusted data.
7. Recheck revisions, tombstones, policy generation and consumer trust immediately before insertion/dispatch. Append without rewriting the submitted prefix. Persist protected provenance before any request consumes it.

Repeat after rollover when a new working set needs recall; avoid reinserting the same turn's results. Do not run before every tool call. A retrieval deadline may yield permitted partial/no results; explicit tools remain available. A privacy violation blocks dispatch rather than degrading to an unsafe path.

### Deliberate recall

Give the agent concise guidance to search for earlier decisions, preferences, experiments, missing continuity or direct questions about past work. It can search even when automatic recall returns nothing. Results are snippets: use `memory_get` or permitted source history when details are needed.

Search accepts query, kind/scope/source filters and bounded limit. The runtime intersects filters with authority. Direct reads enforce the same checks even for guessed IDs. Save attaches source attribution automatically; the agent cannot declare protected-derived knowledge untainted or widen access through tool arguments.

If a protected query cannot use a shared cloud encoder, search shared documents lexically and protected documents with the local encoder. A separately built local shared index may be added later. Never compare local query vectors with cloud document vectors. Report partial semantic availability without logging query text.

### Evidence and persistent safety

Memory is evidence, not instruction or permission. Current user corrections/repository evidence can invalidate old facts. Surface conflicts or supersede explicitly. Retrieved commands/URLs never bypass approvals, sandbox or provenance rules.

Persist provenance, not a transient UI flag. Conservatively protect generated descendants of protected input, including workers, summaries, regeneration and rollover. Preserve flags in session exports/imports. Missing provenance must not be treated as cloud-safe when continuing protected history; require a fresh safe branch or explicit review. Fine-grained automatic declassification is out of scope for v1.

## 15. Persistence and API contracts

### Logical entities

| Entity | Responsibility |
| --- | --- |
| Records/revisions | Canonical text, kind/scope/privacy, current revision and lifecycle |
| Sources/dependencies | Stable session/branch/window/event spans, source revision and derivative links |
| Overrides/suppressions | User corrections and exclusions tied to source spans, independent of chunk IDs |
| Profiles/index generations | Encoder identity, validation and active/building generations |
| Embeddings/FTS | Revision-specific derived indexes partitioned by authority/privacy |
| Jobs/cursors | Leased retryable work and reconciled committed-source position |
| Policy generations/context provenance | Egress fences and durable protected dependencies, integrated with session persistence |

Use separate shared/protected database files by default, with a coordinator for cross-partition changes. Non-content profile/install metadata can be shared. Require private permissions, short transactions, versioned schemas and a consistently packaged FTS-capable SQLite build.

Suppress stable source spans/revisions, not just old chunk IDs: rechunking must not resurrect deleted text. Ambiguous override mappings after source edits stay excluded and require review instead of guessing. Keep minimal non-content tombstones; retained audit history must not hide deleted text. Garbage-collect obsolete text/vector revisions according to retention.

### User-management operations

| Operation | Contract |
| --- | --- |
| List/search/get | Cursor pagination, local search by default, authority-intersected filters |
| Update record | Expected revision, permitted fields, new revision/index status |
| Delete preview/confirm | Exact IDs/revisions and source/derivative/suppression effects |
| Privacy preview/confirm | Exact authorized set, dependency impact and egress disclosure |
| Get/update settings | Expected revision; configured/effective values and blocked reasons |
| List/validate profiles | Capabilities, credential references only, readiness/errors |
| Install/import/cancel/remove | Verified manifest, progress, resource checks and dependency effects |
| Reindex/backfill/retry/cancel | Authorized sources/profile generation, durable progress and disclosure |

Use request IDs/idempotency where operations may replay. Confirmation tokens bind initiating user/client, selected IDs/revisions, policy generation and expiry. Reject stale previews. Filtered bulk mutations apply to the previewed set, not newly matching records.

Return typed errors: revision conflict, scope denied, Local-only blocked, profile unready, consent required, model unavailable, quota exceeded and cancellation. Errors must not reveal protected text or cross-workspace record existence. Progress events contain bounded IDs/counts/state, not record bodies; reconnect retrieves a revisioned snapshot through existing protocol mechanisms.

Rust owns pagination, filtering and mutations. `frontend/shared` carries contracts/client delivery, not another retrieval engine or policy reducer. Negotiate Remote capabilities/version handling so older clients cannot accidentally accept unsupported privacy actions. Tauri/browser transports use the same authority checks.

## 16. Concurrency, failures and resource limits

### Cross-partition privacy fences

A privacy move across database files is not an ordinary single SQLite transaction. Commit a durable shared deny/fence first, stop undispatched cloud jobs, copy/verify the protected revision, then retire shared copies. A crash at any step keeps shared access denied; reconciliation resumes the move. Workers recheck policy generation before dispatch and before committing vectors, so late responses cannot republish moved/deleted records.

Earlier disclosure/in-flight cloud requests cannot be undone. Declassification uses an explicit confirmed preview and staged move; unrelated derivatives stay protected. Fence affected current contexts as well as indexes.

### Jobs and degraded operation

- Leased jobs use bounded retries/backoff with jitter; expired leases recover. Revision/profile keys deduplicate work. Never advance a source cursor past work that was not durably recorded.
- Rate limits/auth failures pause only the affected profile. Other profiles/local lexical management remain usable. Show indexing lag and last successful cursor.
- Disk-full pauses capture/indexing visibly; never evict authoritative sessions or claim success. Bound queues/caches and apply backpressure. Reconcile committed sources when resources recover.
- Shutdown checkpoints cursors/jobs, cancels work and unloads inference. Chat/rollover persistence never depends on embedding success.
- Follow existing daemon/runtime ownership; serialize store writers and enforce file/process locks. Multiple frontend attachments do not create competing memory engines/workers.
- Model removal/provider changes respect leases. Revision-fenced readers and immediate logical deletion precede physical cleanup.

### Initial engineering budgets

These are provisional measurement targets, not unverified product claims:

- At most 2,000 injected tokens and bounded retrieval candidates; no unbounded transcript scans in request assembly.
- Target local management/FTS p95 below 200 ms on a 10,000-chunk fixture on declared reference hardware. Benchmark exact vector search at 10,000/100,000 chunks before choosing ANN.
- Target warm local automatic recall below 500 ms on supported desktop reference hardware. Initial configurable total recall deadline: 1.5 seconds; cold load/cloud latency may exceed it, yielding permitted partial results rather than indefinitely blocking chat.
- Begin with embedding-worker concurrency one. Derive batch/RAM/idle limits from the model spike, not unsupported guesses.
- Ordinary logs/opt-in aggregate diagnostics contain counts, latency and typed failures, not raw memory, queries, vectors or source snippets. Audit existing API logging before embedding traffic is enabled.

## 17. Rollout, migration and release criteria

### First release

Include both kinds; one reusable shared profile and one protected local profile; one verified managed model; at least one verified cloud adapter; automatic/tool hybrid recall; management through both slash entry points; edit/delete/privacy operations; resumable indexing and protected-history provider-switch enforcement.

OpenAI, Gemini and OpenRouter belong in the supported architecture, but advertise specific models/adapters only after checks pass. Defer generated summaries, arbitrary custom local architectures, ANN infrastructure, automatic declassification, cloud memory synchronization and on-device mobile inference. Do not defer egress checks, coverage, suppression or direct-ID authorization.

### Upgrade/backfill/rollback

Add versioned memory storage without rewriting transcripts. Upgrade creates no embedding jobs. Opt-in starts at a committed source position; old history requires selected-scope backfill with counts and cloud disclosure. Never auto-import Foundation/external memories or instruction files.

Migrations are transactional within each store and resumable across stores; incompatible versions block use. Rollback disables dispatch while preserving text, provenance and suppression. Never fall back to an application path that ignores protected metadata. Recovery/export preserves privacy/deletion markers; protected export requires authorization.

### Release gates

- Validate local inference on every advertised platform, offline after installation and without a chat-provider daemon.
- Pass section 11 plus one full scenario: setup → save/capture → recall → edit → privacy move → delete → restart/reindex.
- Intercepting fake transport proves protected queries/documents/derived context do not reach cloud embeddings or generation/background paths. Cover provider switch, cancellation races and crash during privacy move.
- Both slash entry points reach the same manager without model dispatch. Test Remote capability/reconnect behavior and unauthorized management access.
- Evaluate a fixed corpus with paraphrases, exact identifiers, conflicting facts and long tool output. Record relevant-source recall, duplicate injection and budget compliance against FTS-only, rather than assuming vectors always improve results.
- Run relevant Rust/provider/frontend checks per checkpoint and `Scripts/check.sh` for integrated structural work. Record environment/pre-existing failures separately. Documentation-only planning needs no compile/test run.
- Implement visuals only against user-provided/requested design; no invented layout or `.ui` rendering framework.

## 18. Implementation work breakdown and handoff

Checkpoint names describe ownership, not mandatory file-per-entity structure. Commit meaningful increments and keep incomplete features gated.

| Checkpoint | Work | Completion evidence |
| --- | --- | --- |
| A. Compatibility | Pinned local manifest/runtime, cloud contract, provider trust metadata, credential/logging review | Fixture, license/platform report, measured resources, go/no-go |
| B. Policy/schema | `Harness/memory` types/policy, settings/model contracts, persistent taint and central egress checks | Scope/trust tests; protected dispatch disabled without guards |
| C. Store | SQLite/FTS, revisions, sources, suppressions, jobs/cursors, migrations, privacy journal | Crash/restart and deletion/override invariants; lexical management |
| D. Embedding | Rust bridge support as needed, existing TS adapter extensions, installer/worker, validation/rebuild | Cloud fixture, offline managed inference, cancellation/retry |
| E. Explicit tools | Catalog/schemas, save/search/get/forget, attribution, supersession, permissions | Authorized round trip; guessed IDs/scope widening denied |
| F. Session capture | Post-commit notifications/reconciliation, episode chunking, edit/regenerate/rollover | Source coverage without duplicates or resurrected deletions |
| G. Recall | Profile-aware hybrid search, budgeted append-only insertion, citations, switch enforcement | Fixed retrieval corpus and end-to-end privacy checks |
| H. Management | Shared Rust semantic state/actions, Remote contracts, settings and slash routing | Same manager, revision-safe operations, reconnect safety |
| I. Clients/release | User-designed components, supported client adapters, packaging and docs | Accessibility, platform verification and integrated release gates |

Integration points to re-inspect before coding:

- `Harness/core/provider_bridge.rs`, `RuntimeSource/src/bridge-protocol.ts`, `bridge.ts`, `embeddings.ts`, adapters/types: existing JS embedding operations are reusable; extend Rust client/protocol deliberately instead of duplicating transport.
- Harness service persistence/run/lifecycle/replay, session store and agent context archives: add post-commit notifications and reconciliation, never embed while holding persistence locks.
- Agent request assembly/history, detached/background helpers and workers: enforce append-only retrieval and the same egress policy on every path.
- Project/application settings, `Harness/model.rs` and service settings: expose configured/effective references without secrets.
- `UI/composer/state.rs`, `UI/application_session.rs`, shared settings and `Harness/service/commands.rs`: `/settings` already has interactive routing and a help fallback. Preserve it, add `/memories` consistently and route both to shared management, not a model prompt. Read/maintain `TUI_WHITEBOARD.md` before structural TUI changes.
- `src/remote/protocol.rs`, UI hosts/transports, `frontend/shared/remote` and desktop bridge: negotiate capabilities and preserve per-client authority/projection boundaries.
- `app/ui/ContentView.ui` is reserved, with no supplied layout. Read eventual memory specs before React Native implementation; preserve `web` compatibility until parity is verified.

Before each checkpoint, inspect current Git work/history and recall Foundation knowledge when that integration is available. Preserve unrelated work, add only meaningful behavioral tests and record durable decisions/results. Replace open validation gates with measured supported configurations as implementation proceeds; never describe planned behavior as shipped.

## Implementation checkpoint log

### Native store and local management foundation

Implemented `Harness/memory` with separate private shared/local-only SQLite databases, bundled FTS5, workspace/user-global scopes, revision-safe editing/deletion/privacy moves, source-idempotent saves, persistent source suppression/overrides, pending indexing jobs, immutable encoder profiles and revision-fenced vector storage/hybrid search. Attached rollback-journal transactions make store-only privacy moves atomic without enabling WAL.

`yeet memory help` exposes local JSON management: list/search/get/save/edit/delete/privacy/settings/profiles and consented cloud-profile probing with bundled non-sensitive text. Ordinary embedding transport no longer logs request text or returned vectors. Capture/indexing has no implicit network side effects. Profile options such as task-specific input modes still require later adapter work.

This is a backend checkpoint, not the completed product: no agent/automatic capture or recall, visual manager, Remote management contract or managed inference is enabled yet. Protected model reads intentionally fail closed for every provider until durable context provenance and every egress path are guarded. Workspace enablement is persisted but is not yet wired to agent lifecycle. Local administrative privacy operations are not chat-provider UI toggles.

Validated: six focused Rust memory tests, binary build, TypeScript build and six embedding transport tests. Strict library clippy currently reports two pre-existing collapsible-if warnings in `Harness/agent/tool_protocol.rs:374` and `Harness/tools/editing.rs:129`; no new-module warnings were reported. Foundation tools were not available in this environment, so durable implementation state is kept here and in Git.
