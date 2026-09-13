import { expect, test } from '@playwright/test'

test('negotiates the real semantic Remote protocol without legacy frame polling', async ({ page }) => {
  const requests: string[] = []
  const sentFrames: string[] = []
  const receivedFrames: string[] = []
  const socketEvents: string[] = []
  const browserMessages: string[] = []
  let remoteSocketUrl = ''

  page.on('request', (request) => requests.push(request.url()))
  page.on('console', (message) => browserMessages.push(`${message.type()}: ${message.text()}`))
  page.on('pageerror', (error) => browserMessages.push(`pageerror: ${error.message}`))

  await page.route('https://static.cloudflareinsights.com/**', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  })
  page.on('websocket', (socket) => {
    if (!socket.url().includes('/api/ws')) return
    remoteSocketUrl = socket.url()
    socket.on('framesent', (event) => {
      if (typeof event.payload === 'string') sentFrames.push(event.payload)
    })
    socket.on('framereceived', (event) => {
      if (typeof event.payload === 'string') receivedFrames.push(event.payload)
    })
    socket.on('close', () => socketEvents.push('closed'))
    socket.on('socketerror', (error) => socketEvents.push(`error:${error}`))
  })

  const protocol = await page.request.get('/api/protocol')
  expect(protocol.ok()).toBeTruthy()
  await expect(protocol.json()).resolves.toMatchObject({ version: 1, websocket: '/api/ws' })

  await page.goto('/')
  expect((await page.request.get('/api/frame')).status()).toBe(404)
  expect((await page.request.post('/api/input', { data: {} })).status()).toBe(404)
  await expect(page.locator('.desktop-status').first()).toContainText('connected')
  await expect(page.getByTestId('composer')).toBeVisible()

  await expect.poll(() => remoteSocketUrl).toContain('/api/ws')
  await expect.poll(() => sentFrames.some((frame) => {
    try {
      const message = JSON.parse(frame) as Record<string, unknown>
      return message.type === 'hello' && message.min_version === 1 && message.max_version === 1
    } catch {
      return false
    }
  })).toBe(true)

  await expect.poll(() => sentFrames.some((frame) => {
    try {
      const message = JSON.parse(frame) as Record<string, unknown>
      return message.type === 'command'
        && (message.command as Record<string, unknown> | undefined)?.type === 'request_sessions'
    } catch {
      return false
    }
  })).toBe(true)
  await expect.poll(() => receivedFrames.some((frame) => {
    try { return (JSON.parse(frame) as Record<string, unknown>).type === 'ack' }
    catch { return false }
  })).toBe(true)
  await expect.poll(() => receivedFrames.some((frame) => {
    try {
      const message = JSON.parse(frame) as Record<string, unknown>
      const container = message.type === 'snapshot'
        ? message.state as Record<string, unknown> | undefined
        : message.type === 'state_update'
          ? message.patch as Record<string, unknown> | undefined
          : undefined
      const workspaces = container?.known_workspaces
      return Array.isArray(workspaces)
        && workspaces.length > 0
        && workspaces.some((workspace) => (workspace as Record<string, unknown>).is_current === true)
    } catch {
      return false
    }
  })).toBe(true)

  expect(requests.some((url) => url.includes('/api/frame'))).toBe(false)
  expect(requests.some((url) => url.includes('/api/input'))).toBe(false)

  const firstResume = await page.evaluate(() => sessionStorage.getItem('yeet.remote.resume.v1'))
  expect(firstResume).toBeTruthy()
  const firstClientId = JSON.parse(firstResume || '{}').clientId as string | undefined
  expect(firstClientId).toBeTruthy()

  await page.reload()
  await expect(page.locator('.desktop-status').first()).toContainText('connected')
  const secondResume = await page.evaluate(() => sessionStorage.getItem('yeet.remote.resume.v1'))
  expect(JSON.parse(secondResume || '{}').clientId).toBe(firstClientId)

  await expect.poll(() => sentFrames.filter((frame) => {
    try { return (JSON.parse(frame) as Record<string, unknown>).type === 'hello' }
    catch { return false }
  }).length).toBeGreaterThanOrEqual(2)
  const helloFrames = sentFrames
    .map((frame) => {
      try { return JSON.parse(frame) as Record<string, unknown> }
      catch { return null }
    })
    .filter((message): message is Record<string, unknown> => message?.type === 'hello')
  expect(helloFrames.at(-1)).toMatchObject({
    client_id: firstClientId,
    last_sequence: null,
    last_revision: null,
  })

  const index = await page.request.get('/')
  const csp = index.headers()['content-security-policy']
  const indexHtml = await index.text()
  expect(csp).toContain("script-src 'self' https://static.cloudflareinsights.com/beacon.min.js")
  expect(csp).toContain("connect-src 'self' ws: wss: https://cloudflareinsights.com")
  expect(index.headers()['cache-control']).toBe('no-store')
  expect(indexHtml).toContain('https://static.cloudflareinsights.com/beacon.min.js')
  expect(indexHtml).toContain('data-cf-beacon')
  expect(indexHtml).not.toContain('/api/frame')
  expect(receivedFrames.length).toBeGreaterThan(0)
  expect(socketEvents.some((event) => event.startsWith('error:'))).toBe(false)
  expect(browserMessages).toEqual([])
})


test('handles fatal Remote handshake errors without WebSocket console exceptions', async ({ page }) => {
  const browserMessages: string[] = []
  const socketErrors: string[] = []
  let socketClosed = false

  page.on('console', (message) => browserMessages.push(`${message.type()}: ${message.text()}`))
  page.on('pageerror', (error) => browserMessages.push(`pageerror: ${error.message}`))
  await page.route('https://static.cloudflareinsights.com/**', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/javascript', body: '' })
  })
  await page.route('https://cloudflareinsights.com/**', async (route) => {
    await route.fulfill({ status: 204, body: '' })
  })
  page.on('websocket', (socket) => {
    if (!socket.url().includes('/api/ws')) return
    socket.on('close', () => { socketClosed = true })
    socket.on('socketerror', (error) => socketErrors.push(String(error)))
  })
  await page.addInitScript(() => {
    sessionStorage.setItem('yeet.remote.resume.v1', JSON.stringify({
      clientId: 'fatal-handshake-test',
      workspace: '/__yeet_remote_missing_workspace__',
      sessionId: null,
    }))
  })

  await page.goto('/')
  await expect(page.locator('.desktop-status').first()).toContainText('failed')
  await expect.poll(() => socketClosed).toBe(true)

  expect(socketErrors).toEqual([])
  expect(browserMessages).toEqual([])
})


test('rejects incompatible protocol negotiation on a real WebSocket', async ({ page }) => {
  await page.goto('/')
  const result = await page.evaluate(() => new Promise<Record<string, unknown>>((resolve, reject) => {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${location.host}/api/ws`, 'yeet.remote.v1')
    const timeout = window.setTimeout(() => {
      socket.close()
      reject(new Error('timed out waiting for protocol mismatch'))
    }, 5_000)
    socket.addEventListener('open', () => {
      socket.send(JSON.stringify({ type: 'hello', min_version: 2, max_version: 2 }))
    })
    socket.addEventListener('message', (event) => {
      if (typeof event.data !== 'string') return
      const message = JSON.parse(event.data) as Record<string, unknown>
      if (message.type !== 'error') return
      window.clearTimeout(timeout)
      socket.close()
      resolve(message)
    })
    socket.addEventListener('error', () => {
      window.clearTimeout(timeout)
      reject(new Error('WebSocket failed before protocol mismatch response'))
    })
  }))

  expect(result).toMatchObject({
    type: 'error',
    version: 1,
    code: 'protocol_mismatch',
    fatal: true,
    supported_min_version: 1,
    supported_max_version: 1,
  })
})


test('rejects a malformed first protocol frame on a real WebSocket', async ({ page }) => {
  await page.goto('/')
  const result = await page.evaluate(() => new Promise<Record<string, unknown>>((resolve, reject) => {
    const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const socket = new WebSocket(`${protocol}//${location.host}/api/ws`, 'yeet.remote.v1')
    const timeout = window.setTimeout(() => {
      socket.close()
      reject(new Error('timed out waiting for malformed-message response'))
    }, 5_000)
    socket.addEventListener('open', () => socket.send('{broken-json'))
    socket.addEventListener('message', (event) => {
      if (typeof event.data !== 'string') return
      const message = JSON.parse(event.data) as Record<string, unknown>
      if (message.type !== 'error') return
      window.clearTimeout(timeout)
      socket.close()
      resolve(message)
    })
    socket.addEventListener('error', () => {
      window.clearTimeout(timeout)
      reject(new Error('WebSocket failed before malformed-message response'))
    })
  }))

  expect(result).toMatchObject({
    type: 'error',
    version: 1,
    code: 'malformed_message',
    fatal: true,
    supported_min_version: 1,
    supported_max_version: 1,
  })
})
