# Provider reasoning and activity display

Yeet shows provider-emitted reasoning or summaries alongside real local tool
activity. A provider entering a hidden thinking block shows a waiting phase;
it does not create invented reasoning text. The TUI and web transcript keep
summaries independently expandable, and current tools take precedence over
older reasoning in the live status.

## Routes audited on 2026-09-30

| Yeet route | Wire protocol | Visible model information | Continuation and cache behavior |
| --- | --- | --- | --- |
| `codex-cli` (ChatGPT subscription) | Streaming Responses over OAuth | Reasoning summary parts; explicit reasoning phase; hosted-tool activity | Streaming-only, `store: false`; signed reasoning items retained; API-only request fields removed |
| `openai` (API) | Responses | Summary deltas and done-only summaries, compatible plaintext reasoning, hosted-tool activity | Reasoning items including encrypted state replayed only for the same provider/model; stable context cache affinity |
| `claude` (subscription), `anthropic`, `claude-api` | Messages over OAuth or API key | Visible thinking summaries and progress updates; hidden thinking phase; server-managed tool activity | Adaptive thinking plus summarized display for supported modern models; bounded manual budget for legacy models; signed/redacted blocks replayed unchanged; four cache breakpoints; one-hour cache write pricing shared by aliases |
| `gemini` (API), `gemini-web` (Code Assist) | GenerateContent, optionally Code Assist envelope | `thought: true` text is a model summary | `includeThoughts` requested for supported models; explicit opt-outs preserved; thought signatures retained for continuation; output usage includes thinking tokens and input usage includes tool-use prompts |
| `antigravity` | Gemini Interactions managed-agent API | Thought-step summaries, explicit reasoning phases, remote managed-tool activity | Interaction/environment IDs and function-name mapping preserved; provider options forwarded; no unsupported GenerateContent or generic reasoning parameters invented |
| `openrouter` | OpenAI-compatible chat | Native reasoning fields and structured reasoning details | Opaque reasoning details and native `reasoning_content` preserved; logical blocks kept separate; context/session affinity and content cache boundaries retained |
| `opencode`, `opencode-go` | Catalog-selected Responses, Messages, Google, or chat | Corresponding native protocol's reasoning/summary information | Uses the selected protocol rather than forcing every model through chat; native reasoning continuation survives tool turns |
| Custom OpenAI-compatible endpoints (including compatible gateways/local models) | Chat completions | `reasoning_content`, `reasoning`, `reasoning_text`, `thinking`, summary aliases, structured details | Native `reasoning_content` including an explicitly empty string is retained for replay; state is scoped to provider/model |
| Zed | Client, not a built-in Yeet provider | Determined by the underlying provider selected in Zed or Yeet | Yeet has no Zed-hosted adapter or undocumented Zed endpoint. The shared provider protocols above cover equivalent API/gateway routes; Zed external agents own their own model access |

## Display and formatting rules

- Model summaries remain authoritative when plaintext reasoning also arrives.
- Reasoning block starts seal the prior live segment. Changing to tool work or
  answer text clears old reasoning buffers while retaining completed entries.
- Empty or encrypted thinking creates a phase only, never a fabricated summary.
- Markdown headings and adjacent bold titles produce readable compact labels;
  expanded disclosures retain the complete readable summary.
- Summary parts retain paragraph boundaries, including done-only Responses
  events; repeated done events do not duplicate content.
- Compatible reasoning details are merged only within the same logical block.
- SSE parsing handles CRLF split across chunks and UTF-8 split across bytes.
- Provider-managed tools are presentation events, not local tool calls, and are
  explicitly identified as remote/provider-managed work.

## Verification

Regression coverage uses synthetic provider responses and streamed fixtures,
including continuation requests after tool calls, explicit opt-outs, signed
state, hidden reasoning, managed tools, summary boundaries, UTF-8/CRLF framing,
and desktop/mobile transcript transitions. These tests do not claim paid live
coverage for every account or model. Providers can omit summaries on simple
requests, redact them, or decline to expose them; the display reflects what
was actually returned.

## Provider references

- [Claude thinking configuration, display, streaming, and signed replay](https://platform.claude.com/docs/en/build-with-claude/thinking)
- [Gemini thought summaries and signatures](https://ai.google.dev/gemini-api/docs/thinking)
- [Antigravity managed-agent API](https://ai.google.dev/gemini-api/docs/antigravity-agent)
- [OpenRouter reasoning fields and replay](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)
- [OpenCode Go protocol-specific endpoints](https://opencode.ai/docs/go/)
- [DeepSeek thinking-mode continuation requirements](https://api-docs.deepseek.com/guides/thinking_mode/)
- [Zed model access and external-agent distinction](https://zed.dev/docs/ai/llm-providers)
