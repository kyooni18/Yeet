import test from 'node:test';
import assert from 'node:assert/strict';
import { AnthropicProvider, OpenAIChatProvider, OpenAIProvider, GeminiProvider } from '../dist/index.js';
import { withReasoningPolicy } from '../dist/request-policy.js';
import { parseSSE } from '../dist/sse.js';

const json = (body) => new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } });
const sse = (events) => new Response(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(''), { headers: { 'content-type': 'text/event-stream' } });
const messages = [{ role: 'user', content: 'Inspect the file' }];
const collect = async (stream) => { const events = []; for await (const event of stream) events.push(event); return events; };

test('native thinking policy enables visible summaries without overwriting opt-outs or auxiliary calls', () => {
  for (const provider of ['anthropic', 'claude', 'claude-api', 'opencode', 'opencode-go']) {
    const request = { model: `${provider}/claude-sonnet-4-6`, messages, metadata: { reasoningLevel: 'high' } };
    const prepared = withReasoningPolicy(request);
    assert.deepEqual(prepared.providerOptions.thinking, { type: 'adaptive', display: 'summarized' });
    assert.equal(prepared.providerOptions.output_config.effort, 'high');
    assert.equal(prepared.providerOptions.reasoning, undefined);
    const explicit = withReasoningPolicy({ ...request, providerOptions: { thinking: { type: 'disabled' }, output_config: { effort: 'low' } } });
    assert.deepEqual(explicit.providerOptions, { thinking: { type: 'disabled' }, output_config: { effort: 'low' } });
  }
  const legacy = withReasoningPolicy({ model: 'claude-api/claude-sonnet-4-5', messages, maxTokens: 4096, metadata: { reasoningLevel: 'high' } });
  assert.deepEqual(legacy.providerOptions.thinking, { type: 'enabled', budget_tokens: 2048 });
  assert.equal(withReasoningPolicy({ model: 'anthropic/claude-sonnet-4-5', messages, toolChoice: 'required' }).providerOptions, undefined);
  assert.equal(withReasoningPolicy({ model: 'anthropic/claude-sonnet-4-5', messages, maxTokens: 1024 }).providerOptions, undefined);
  const openai = withReasoningPolicy({ model: 'codex-cli/gpt-6-sol', messages, providerOptions: { reasoning: { effort: 'high' } } });
  assert.deepEqual(openai.providerOptions.reasoning, { effort: 'high', summary: 'auto' });
  assert.equal(withReasoningPolicy({ model: 'antigravity/antigravity-preview-09-2026', messages, metadata: { reasoningLevel: 'high' } }).providerOptions, undefined);
});

test('Claude signed thinking and native chat reasoning survive a streamed tool round trip', async () => {
  let claudeBody;
  const claude = new AnthropicProvider({ id: 'claude-api', apiKey: 'test', fetch: async (_url, init) => {
    claudeBody = JSON.parse(init.body);
    return sse([
      { type: 'message_start', message: { id: 'm', model: 'claude-sonnet-4-6', usage: {} } },
      { type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: 'Checking ', signature: '' } },
      { type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'the file' } },
      { type: 'content_block_delta', index: 0, delta: { type: 'signature_delta', signature: 'signed-state' } },
      { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: {} },
    ]);
  } });
  const events = await collect(claude.stream({ model: 'claude-sonnet-4-6', messages, temperature: 0.2, providerOptions: { thinking: { type: 'adaptive', display: 'summarized' } } }));
  assert.equal(claudeBody.temperature, undefined);
  assert.ok(events.some((event) => event.type === 'reasoning-start'));
  assert.equal(events.filter((event) => event.type === 'reasoning-summary-delta').map((event) => event.delta).join(''), 'Checking the file');
  assert.deepEqual(events.at(-1).providerState.data.thinkingBlocks, [{ type: 'thinking', thinking: 'Checking the file', signature: 'signed-state' }]);
  await collect(claude.stream({ model: 'claude-sonnet-4-6', messages: [...messages, { role: 'assistant', content: '', providerState: events.at(-1).providerState }, { role: 'user', content: 'Continue' }] }));
  assert.equal(claudeBody.messages[1].content[0].signature, 'signed-state');

  let chatBody;
  const chat = new OpenAIChatProvider({ id: 'opencode-go', baseUrl: 'https://example.invalid/v1', fetch: async (_url, init) => {
    chatBody = JSON.parse(init.body);
    return sse([{ choices: [{ delta: { reasoning_content: 'Inspect ' } }] }, { choices: [{ delta: { reasoning_content: 'inputs', content: 'done' }, finish_reason: 'stop' }] }]);
  } });
  const chatEvents = await collect(chat.stream({ model: 'deepseek-test', messages }));
  const state = chatEvents.at(-1).providerState;
  assert.equal(state.data.reasoningContent, 'Inspect inputs');
  await collect(chat.stream({ model: 'deepseek-test', messages: [...messages, { role: 'assistant', content: '', toolCalls: [{ id: 'call', name: 'read_file', arguments: {} }], providerState: state }, { role: 'tool', toolCallId: 'call', content: 'ok' }] }));
  assert.equal(chatBody.messages[1].reasoning_content, 'Inspect inputs');
  await collect(chat.stream({ model: 'other-model', messages: [...messages, { role: 'assistant', content: 'done', providerState: state }] }));
  assert.equal(chatBody.messages[1].reasoning_content, undefined);
});

test('provider streams expose hidden reasoning phases, managed operations, and summarized Gemini thoughts', async () => {
  const openai = new OpenAIProvider({ apiKey: 'test', fetch: async () => sse([
    { type: 'response.created', response: { id: 'r' } },
    { type: 'response.output_item.added', item: { type: 'reasoning', id: 'reason' } },
    { type: 'response.web_search_call.in_progress' },
    { type: 'response.completed', response: { id: 'r', status: 'completed', output: [], usage: {} } },
  ]) });
  const events = await collect(openai.stream({ model: 'gpt-test', messages }));
  assert.ok(events.some((event) => event.type === 'reasoning-start'));
  assert.ok(events.some((event) => event.type === 'activity' && event.title === 'Searching web'));
  assert.equal(events.some((event) => event.type === 'reasoning-summary-delta'), false);
  const doneOnly = new OpenAIProvider({ apiKey: 'test', fetch: async () => sse([
    { type: 'response.reasoning_summary_text.done', item_id: 'r', summary_index: 0, text: 'Completed summary' },
    { type: 'response.reasoning_summary_text.done', item_id: 'r', summary_index: 0, text: 'Completed summary' },
    { type: 'response.completed', response: { id: 'r', status: 'completed', output: [], usage: {} } },
  ]) });
  assert.equal((await collect(doneOnly.stream({ model: 'gpt-test', messages }))).filter((event) => event.type === 'reasoning-summary-delta').map((event) => event.delta).join(''), 'Completed summary');
  const gemini = new GeminiProvider({ apiKey: 'test', fetch: async () => sse([{ candidates: [{ content: { parts: [{ thought: true, text: 'Comparing options' }, { text: 'Answer' }] }, finishReason: 'STOP' }] }]) });
  assert.deepEqual((await collect(gemini.stream({ model: 'gemini-test', messages }))).filter((event) => event.type === 'reasoning-summary-delta'), [{ type: 'reasoning-summary-delta', delta: 'Comparing options' }]);
  const broken = new AnthropicProvider({ apiKey: 'test', fetch: async () => sse([{ type: 'error', error: { type: 'overloaded_error', message: 'Retry later' } }]) });
  await assert.rejects(() => collect(broken.stream({ model: 'claude-test', messages })), (error) => error.message === 'Retry later' && error.retryable);
});

test('SSE survives CRLF and Unicode split at every byte boundary', async () => {
  const source = 'event: reasoning\r\ndata: 한글\r\n\r\ndata: next\r\n\r\n';
  const bytes = new TextEncoder().encode(source);
  const response = new Response(new ReadableStream({ start(controller) { for (const byte of bytes) controller.enqueue(Uint8Array.of(byte)); controller.close(); } }));
  assert.deepEqual(await collect(parseSSE(response)), [{ event: 'reasoning', data: '한글' }, { data: 'next' }]);
  assert.deepEqual(await collect(parseSSE(new Response('data: first\r\rdata:   indented'))), [{ data: 'first' }, { data: '  indented' }]);
});
