import * as readline from 'node:readline';

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const send = (value) => process.stdout.write(`${JSON.stringify(value)}\n`);
let pendingToolCall;

rl.on('line', (line) => {
  const message = JSON.parse(line);
  if (message.method === 'server/discover') {
    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2026-07-28', capabilities: { elicitation: { form: {} } }, serverInfo: { name: 'native-approval', version: '1' } } });
  } else if (message.method === 'tools/list') {
    send({ jsonrpc: '2.0', id: message.id, result: { tools: [{ name: 'open', inputSchema: { type: 'object', properties: {} } }] } });
  } else if (message.method === 'tools/call') {
    pendingToolCall = message;
    const modern = process.env.NATIVE_APPROVAL_SHAPE === 'modern';
    const approvalMeta = modern
      ? {
          codex_approval_kind: process.env.NATIVE_APPROVAL_KIND ?? 'mcp_tool_call',
          connector_id: 'computer-use',
          connector_name: 'Computer Use',
          persist: ['session', 'always'],
          riskLevel: 'low',
          tool_name: 'get_app_state',
          tool_params: { app: 'org.blenderfoundation.blender' },
          tool_params_display: [{ name: 'app', display_name: 'App', value: 'Blender' }],
        }
      : {
          codex_approval_kind: process.env.NATIVE_APPROVAL_KIND ?? 'mcp_tool_call',
          tool_params: {
            app: { bundleId: 'org.blenderfoundation.blender', name: 'Blender' },
            tool: 'open',
            operation: 'Open Blender',
          },
        };
    send({
      jsonrpc: '2.0',
      id: 9001,
      method: 'elicitation/create',
      params: {
        message: 'Open Blender for this MCP operation',
        ...(modern
          ? { _meta: approvalMeta, mode: 'form', requestedSchema: { type: 'object', properties: {} } }
          : { meta: approvalMeta }),
      },
    });
  } else if (message.id === 9001 && pendingToolCall) {
    send({ jsonrpc: '2.0', id: pendingToolCall.id, result: { content: [{ type: 'text', text: JSON.stringify(message.result ?? message.error ?? null) }] } });
    pendingToolCall = undefined;
  }
});
