import assert from 'node:assert/strict';
import test from 'node:test';
import { OpenRouterProvider } from '../dist/providers/openrouter.js';

async function capture(overrides = {}) {
  let body;
  const provider = new OpenRouterProvider({
    apiKey: 'test-only',
    fetch: async (_url, init) => {
      body = JSON.parse(init.body);
      return new Response(JSON.stringify({
        id: 'test', choices: [{ message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' }],
      }), { headers: { 'content-type': 'application/json' } });
    },
  });
  await provider.complete({
    model: 'anthropic/claude-sonnet-4',
    messages: [
      { role: 'system', content: 'Stable instructions', cacheBreakpoint: true },
      { role: 'system', content: 'Changing guidance' },
      { role: 'user', content: 'Question', cacheBreakpoint: true },
    ],
    ...overrides,
  });
  return body;
}

const marked = body => body.messages.filter(m => Array.isArray(m.content)
  && m.content.some(block => block.cache_control));

test('preserves a selected system cache boundary before changing guidance', async () => {
  const body = await capture();
  assert.equal(marked(body).length, 2);
  assert.equal(body.messages[0].content[0].text, 'Stable instructions');
  assert.equal(body.messages[1].content, 'Changing guidance');
  const next = await capture({ messages: [
    { role: 'system', content: 'Stable instructions', cacheBreakpoint: true },
    { role: 'system', content: 'Different guidance' },
    { role: 'user', content: 'Question', cacheBreakpoint: true },
  ] });
  assert.deepEqual(next.messages[0], body.messages[0]);
});

test('system boundaries share the four-marker limit with history', async () => {
  const body = await capture({ system: 'Explicit instructions', messages: [
    { role: 'system', content: 'Stable instructions', cacheBreakpoint: true },
    ...Array.from({ length: 6 }, (_, i) => ({ role: 'user', content: `Turn ${i}`, cacheBreakpoint: true })),
  ] });
  assert.equal(marked(body).length, 4);
  assert.equal(body.messages[0].content, 'Explicit instructions');
  assert.equal(marked(body)[0].content[0].text, 'Stable instructions');
  assert.equal(marked(body).at(-1).content[0].text, 'Turn 5');
});

test('disabled and implicit-only caching retain plain system serialization', async () => {
  for (const overrides of [{ promptCache: false }, { model: 'openai/gpt-4o' }]) {
    const body = await capture(overrides);
    assert.equal(marked(body).length, 0);
    assert.equal(body.messages[0].content, 'Stable instructions\n\nChanging guidance');
  }
});
