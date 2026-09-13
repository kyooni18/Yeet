import type { Page } from '@playwright/test'

export async function installMockRemote(page: Page): Promise<void> {
  await page.route('**/api/auth/status', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({ required: false, authenticated: true, key: false, passkey: false }),
    })
  })

  await page.route('**/api/protocol', async (route) => {
    await route.fulfill({
      contentType: 'application/json',
      body: JSON.stringify({
        version: 1,
        minVersion: 1,
        maxVersion: 1,
        websocket: '/api/ws',
      }),
    })
  })

  await page.addInitScript(() => {
    const initialState = {
      conversation_revision: 1,
      conversation: [
        { id: 'u1', kind: { type: 'user', content: 'Inspect the **remote UI** and keep it responsive.' } },
        {
          id: 'a1',
          kind: {
            type: 'assistant',
            content: '## Interface ready\n\nThe semantic browser frontend is connected.\n\n```ts\nconst transport = "websocket"\nconsole.log(transport)\n```',
          },
        },
        {
          id: 'r1',
          kind: {
            type: 'reasoning',
            content: 'I checked the semantic state and selected a responsive layout.',
            summary: 'Checked state and responsive layout',
          },
        },
        {
          id: 'act1',
          kind: {
            type: 'activity',
            activity: { phase: 'running', title: 'Inspecting workspace', detail: 'web/src', run_id: 'run-1' },
          },
        },
        {
          id: 'tool1',
          kind: {
            type: 'toolCall',
            toolCall: {
              id: 'tool1',
              name: 'read_file',
              arguments: '{"path":"web/src/App.vue"}',
              status: 'completed',
              durationMs: 82,
              result: 'App.vue loaded successfully',
            },
          },
        },
        { id: 'skill1', kind: { type: 'skill', name: 'frontend-review', content: 'Responsive checks enabled.', status: 'loaded' } },
        { id: 'mcp1', kind: { type: 'mcp', server: 'Yeet-KY', name: 'list_files', content: 'web/src', isError: false } },
        { id: 'sys1', kind: { type: 'system', content: 'Remote protocol v1 established.' } },
      ],
      active_assistant_entry_id: null,
      active_assistant_text: '',
      active_activity_entry_id: 'act1',
      active_reasoning_entry_id: null,
      active_reasoning_text: '',
      active_reasoning_summary: '',
      is_streaming: false,
      infinity_mode: false,
      error_message: null,
      active_model: 'gpt-5.6-sol',
      active_reasoning_level: 'high',
      active_model_context_length: 200000,
      current_context_tokens: 42000,
      token_usage: { input_tokens: 42000, output_tokens: 2600 },
      credit_usage: 0,
      pending_shell_permission: null,
      pending_native_app_permission: null,
      available_models: ['gpt-5.6-sol', 'gpt-5.6-luna'],
      is_loading_models: false,
      saved_sessions: [
        { id: 'session-a', title: 'Remote WebUI', updated_at: new Date().toISOString(), model: 'gpt-5.6-sol', message_count: 8 },
        { id: 'session-b', title: 'Protocol review', updated_at: new Date(Date.now() - 3600000).toISOString(), model: 'gpt-5.6-luna', message_count: 4 },
      ],
      known_workspaces: [
        { id: '/Users/test/Code/Rust/Yeet', path: '/Users/test/Code/Rust/Yeet', display_name: 'Yeet', updated_at: new Date().toISOString(), session_count: 2, is_current: true },
        { id: '/Users/test/Code/Rust/AnotherProject', path: '/Users/test/Code/Rust/AnotherProject', display_name: 'AnotherProject', updated_at: new Date(Date.now() - 7200000).toISOString(), session_count: 1, is_current: false },
      ],
      current_session_id: 'session-a',
      active_run_id: 'run-1',
      available_capabilities: [
        { id: 'mcp:yeet', kind: 'mcp', name: 'Yeet MCP', description: 'Workspace execution tools', enabled: true },
        { id: 'skill:review', kind: 'skill', name: 'Review', description: 'Review frontend changes', enabled: false },
      ],
      is_loading_capabilities: false,
      auth_providers: [
        {
          provider: 'openai', authenticated: true, method: 'oauth', expires_at: null, error: null,
          usage: {
            provider: 'openai', available: true, source: 'api', fetchedAt: new Date().toISOString(), plan: 'team', message: null,
            windows: [{ id: 'five-hour', label: '5 hour', usedPercent: 31, remainingPercent: 69, resetsAt: null }],
          },
        },
      ],
      auth_notice: null,
      auth_working: false,
      provider_configurations: [{ id: 'local', base_url: 'http://127.0.0.1:11434/v1', require_api_key: false, header_count: 0 }],
      providers_notice: null,
      providers_working: false,
      openai_flex: false,
      foundation_memory_enabled: true,
      foundation_memory_server: 'foundation',
      foundation_memory_connected: true,
      settings_notice: null,
      settings_working: false,
      sandbox_settings: {
        preset: 'balanced', execution_mode: 'sandboxed', auto_approve: false, workspace_mode: 'all', workspace_paths: [], scratch_writable: true,
        network_allow: [{ host: 'api.openai.com', port: 443 }],
        environment: [{ key: 'RUST_LOG', value: 'info' }],
        secret_ids: ['OPENAI_API_KEY'],
        limits: { wall_time_seconds: 300, max_stdout_bytes: 1048576, max_stderr_bytes: 1048576, max_memory_bytes: 2147483648, max_processes: 32 },
      },
      sandbox_notice: null,
      sandbox_working: false,
      debate: null,
    }

    type Message = Record<string, unknown>
    const sockets: MockSocket[] = []
    const sent: Message[] = []
    const generatedClientId = `pw-${crypto.randomUUID()}`
    let forceFreshHandshake = false

    class MockSocket extends EventTarget {
      static readonly CONNECTING = 0
      static readonly OPEN = 1
      static readonly CLOSING = 2
      static readonly CLOSED = 3
      readonly CONNECTING = 0
      readonly OPEN = 1
      readonly CLOSING = 2
      readonly CLOSED = 3
      readyState = MockSocket.CONNECTING
      binaryType: BinaryType = 'blob'
      bufferedAmount = 0
      extensions = ''
      protocol = 'yeet.remote.v1'
      url: string
      onopen: ((this: WebSocket, ev: Event) => unknown) | null = null
      onclose: ((this: WebSocket, ev: CloseEvent) => unknown) | null = null
      onerror: ((this: WebSocket, ev: Event) => unknown) | null = null
      onmessage: ((this: WebSocket, ev: MessageEvent) => unknown) | null = null

      constructor(url: string | URL) {
        super()
        this.url = String(url)
        sockets.push(this)
        setTimeout(() => {
          this.readyState = MockSocket.OPEN
          this.dispatchEvent(new Event('open'))
        }, 0)
      }

      send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
        const message = JSON.parse(String(data)) as Message
        sent.push(message)
        if (message.type === 'hello') {
          queueMicrotask(() => {
            const cursor = typeof message.last_sequence === 'number' ? message.last_sequence : null
            const revision = typeof message.last_revision === 'number' ? message.last_revision : 1
            const requestedClientId = typeof message.client_id === 'string' ? message.client_id : null
            const resumed = !forceFreshHandshake && cursor != null && requestedClientId != null
            const clientId = requestedClientId ?? generatedClientId
            const sessionId = typeof message.session_id === 'string' ? message.session_id : 'session-a'
            const workspace = typeof message.workspace === 'string' && message.workspace
              ? message.workspace
              : '/Users/test/Code/Rust/Yeet'
            forceFreshHandshake = false
            this.emit({ type: 'welcome', version: 1, client_id: clientId, workspace, session_id: sessionId, sequence: cursor ?? 0, revision, resumed })
            if (!resumed) {
              const state = {
                ...initialState,
                known_workspaces: initialState.known_workspaces.map((item) => ({ ...item, is_current: item.path === workspace })),
              }
              this.emit({ type: 'snapshot', version: 1, sequence: 1, revision: 1, state })
            }
          })
        }
      }

      close(code = 1000, reason = '') {
        if (this.readyState === MockSocket.CLOSED) return
        this.readyState = MockSocket.CLOSED
        this.dispatchEvent(new CloseEvent('close', { code, reason, wasClean: true }))
      }

      emit(message: Message) {
        this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(message) }))
      }
    }

    Object.defineProperty(window, 'WebSocket', { configurable: true, writable: true, value: MockSocket })
    Object.assign(window, {
      __yeetSent: sent,
      __yeetEmit: (message: Message) => sockets.at(-1)?.emit(message),
      __yeetDisconnect: () => sockets.at(-1)?.close(1006, 'test disconnect'),
      __yeetRestart: () => {
        forceFreshHandshake = true
        sockets.at(-1)?.close(1012, 'backend restart')
      },
    })
  })
}
