import * as readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const providers = new Set(['openai', 'anthropic', 'gemini', 'openrouter']);
const auth = new Map();
const mcpServers = new Map();
const send = (value) => process.stdout.write(JSON.stringify(value) + '\n');
const statusFor = (provider) => auth.get(provider) ?? {
  provider,
  authenticated: false,
  method: 'none',
  configDir: '/tmp/fake-yeet'
};
rl.on('line', (line) => {
  const cmd = JSON.parse(line);
  const base = { v: 1, id: cmd.id };
  switch (cmd.op) {
    case 'ping': send({ ...base, type: 'pong' }); break;
    case 'list-providers': send({ ...base, type: 'providers', providers: [...providers] }); break;
    case 'list-models': send({ ...base, type: 'models', provider: cmd.provider, models: [`${cmd.provider}-test-model`] }); break;
    case 'config-path': send({ ...base, type: 'config-path', path: '/tmp/fake-yeet' }); break;
    case 'auth-status': send({ ...base, type: 'auth-status', status: statusFor(cmd.provider) }); break;
    case 'auth-set-api-key': {
      const status = { provider: cmd.provider, authenticated: true, method: 'api-key', configDir: '/tmp/fake-yeet' };
      auth.set(cmd.provider, status);
      send({ ...base, type: 'auth-status', status });
      break;
    }
    case 'auth-login-browser': {
      const status = { provider: cmd.provider, authenticated: true, method: 'browser', configDir: '/tmp/fake-yeet' };
      auth.set(cmd.provider, status);
      send({ ...base, type: 'auth-status', status });
      break;
    }
    case 'auth-logout': {
      auth.delete(cmd.provider);
      send({ ...base, type: 'auth-status', status: statusFor(cmd.provider) });
      break;
    }
    case 'register-provider':
      providers.add(cmd.provider.id);
      send({ ...base, type: 'registered', provider: cmd.provider.id });
      break;
    case 'unregister-provider': {
      const removed = providers.delete(cmd.provider);
      send({ ...base, type: 'unregistered', provider: cmd.provider, removed });
      break;
    }
    case 'complete': {
      const provider = cmd.request.model.split('/')[0];
      send({
        ...base,
        type: 'result',
        result: {
          provider,
          model: cmd.request.model,
          id: 'fake-1',
          text: 'hello from bridge',
          toolCalls: [],
          finishReason: 'stop',
          usage: { inputTokens: 3, outputTokens: 4, totalTokens: 7 },
          raw: { ok: true }
        }
      });
      break;
    }
    case 'stream': {
      const provider = cmd.request.model.split('/')[0];
      send({ ...base, type: 'event', event: { type: 'start', provider, model: cmd.request.model, id: 'fake-s' } });
      send({ ...base, type: 'event', event: { type: 'text-delta', delta: 'hello ' } });
      send({ ...base, type: 'event', event: { type: 'text-delta', delta: 'stream' } });
      send({ ...base, type: 'event', event: { type: 'finish', finishReason: 'stop', usage: { outputTokens: 2 } } });
      send({ ...base, type: 'done' });
      break;
    }
    case 'skill-list':
      send({ ...base, type: 'skills', skills: [{ name: 'review-code', description: 'Review code', root: '/tmp/fake-yeet/skills/review-code', entrypoint: '/tmp/fake-yeet/skills/review-code/SKILL.md' }] });
      break;
    case 'skill-load':
      send({ ...base, type: 'skill', skill: { name: cmd.skill, description: 'Review code', root: '/tmp/fake-yeet/skills/review-code', entrypoint: '/tmp/fake-yeet/skills/review-code/SKILL.md', instructions: 'Inspect the diff.', files: ['SKILL.md', 'references/checklist.md'] } });
      break;
    case 'skill-read':
      send({ ...base, type: 'skill-file', content: '# Checklist\nCheck tests.' });
      break;
    case 'mcp-list-servers':
      send({ ...base, type: 'mcp-servers', servers: [...mcpServers.values()].map(server => ({ ...server, connected: false })) });
      break;
    case 'mcp-set-server':
      mcpServers.set(cmd.server.name, cmd.server);
      send({ ...base, type: 'mcp-server', server: cmd.server });
      break;
    case 'mcp-remove-server':
      send({ ...base, type: 'mcp-removed', removed: mcpServers.delete(cmd.server) });
      break;
    case 'mcp-list-tools':
      send({ ...base, type: 'mcp-tools', tools: [{ server: cmd.server ?? 'demo', name: 'search', qualifiedName: `${cmd.server ?? 'demo'}/search`, description: 'Search', inputSchema: { type: 'object' } }] });
      break;
    case 'mcp-call-tool':
      send({ ...base, type: 'mcp-tool-result', toolResult: { content: [{ type: 'text', text: `called ${cmd.tool}` }], isError: false } });
      break;
    case 'mcp-list-resources':
      send({ ...base, type: 'mcp-resources', resources: [{ server: cmd.server ?? 'demo', uri: 'file:///demo.txt', name: 'demo.txt', mimeType: 'text/plain' }] });
      break;
    case 'mcp-read-resource':
      send({ ...base, type: 'mcp-resource-result', resourceResult: { contents: [{ uri: cmd.uri, mimeType: 'text/plain', text: 'demo' }] } });
      break;
    case 'mcp-list-prompts':
      send({ ...base, type: 'mcp-prompts', prompts: [{ server: cmd.server ?? 'demo', name: 'summarize', qualifiedName: `${cmd.server ?? 'demo'}/summarize`, description: 'Summarize' }] });
      break;
    case 'mcp-get-prompt':
      send({ ...base, type: 'mcp-prompt-result', promptResult: { description: 'Summarize', messages: [{ role: 'user', content: { type: 'text', text: 'Summarize this' } }] } });
      break;
    case 'mcp-disconnect': send({ ...base, type: 'done' }); break;
    case 'cancel': send({ ...base, type: 'cancelled', target: cmd.target }); break;
    case 'shutdown':
      send({ ...base, type: 'done' });
      setTimeout(() => process.exit(0), 1);
      break;
  }
});
