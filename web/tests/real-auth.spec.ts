import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { expect, test } from '@playwright/test'

const yeet = fileURLToPath(new URL('../../target/debug/yeet', import.meta.url))
const workspace = fileURLToPath(new URL('../.e2e-workspace/', import.meta.url))
const configDir = fileURLToPath(new URL('../.e2e-config/', import.meta.url))
const accessKey = 'yeet_e2e_remote_access_key_2026'

function authCommand(args: string[], input?: string): string {
  const result = spawnSync(yeet, ['remote', 'auth', ...args, '--workspace', workspace], {
    encoding: 'utf8',
    input,
    env: { ...process.env, YEET_CONFIG_DIR: configDir },
  })
  if (result.status !== 0) {
    throw new Error(`yeet remote auth ${args.join(' ')} failed: ${result.stderr || result.stdout}`)
  }
  return result.stdout
}

test.describe.serial('real Remote authentication', () => {
  test.afterEach(() => {
    authCommand(['key', 'clear'])
  })

  test('rejects invalid keys, authorizes a valid key, rejects a wrong WebSocket Origin, and invalidates stale sessions', async ({ page }) => {
    authCommand(['key', 'set'], `${accessKey}\n`)
    await page.goto('/')

    await expect(page.getByRole('heading', { name: 'Authorization required' })).toBeVisible()
    const unauthorizedSocket = await page.evaluate(async () => {
      const socket = new WebSocket(`${location.origin.replace(/^http/, 'ws')}/api/ws`, 'yeet.remote.v1')
      return await new Promise<string>((resolve) => {
        const timer = window.setTimeout(() => resolve('timeout'), 3000)
        socket.addEventListener('open', () => { window.clearTimeout(timer); resolve('opened') })
        socket.addEventListener('error', () => { window.clearTimeout(timer); resolve('rejected') })
      })
    })
    expect(unauthorizedSocket).toBe('rejected')

    await page.getByLabel('Access key').fill('definitely-wrong')
    await page.getByRole('button', { name: 'Authorize' }).click()
    await expect(page.getByRole('alert')).toContainText('invalid access key')

    await page.getByLabel('Access key').fill(accessKey)
    await page.getByRole('button', { name: 'Authorize' }).click()
    await expect(page.locator('.desktop-status').first()).toContainText('connected')

    const status = await page.request.get('/api/auth/status')
    await expect(status.json()).resolves.toMatchObject({ required: true, authenticated: true, key: true })

    const wrongOrigin = await page.request.get('/api/ws', {
      headers: {
        connection: 'Upgrade',
        upgrade: 'websocket',
        origin: 'https://not-the-yeet-origin.invalid',
        'sec-websocket-version': '13',
        'sec-websocket-key': 'dGhlIHNhbXBsZSBub25jZQ==',
        'sec-websocket-protocol': 'yeet.remote.v1',
      },
    })
    expect(wrongOrigin.status()).toBe(403)

    await page.reload()
    await expect(page.locator('.desktop-status').first()).toContainText('connected')

    // Reloading Remote auth invalidates all issued sessions. This exercises the
    // browser behavior of a no-longer-valid session without waiting 12 hours.
    authCommand(['key', 'set'], `${accessKey}\n`)
    await page.reload()
    await expect(page.getByRole('heading', { name: 'Authorization required' })).toBeVisible()
  })

  test('enrolls and authenticates a real passkey with a virtual platform authenticator', async ({ page, context }) => {
    authCommand(['passkey', 'clear'])
    const cdp = await context.newCDPSession(page)
    await cdp.send('WebAuthn.enable')
    const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
      options: {
        protocol: 'ctap2',
        transport: 'internal',
        hasResidentKey: true,
        hasUserVerification: true,
        isUserVerified: true,
        automaticPresenceSimulation: true,
      },
    })

    try {
      const output = authCommand(['passkey', 'add'])
      const enrollmentUrl = output.match(/https?:\/\/\S+/)?.[0]
      expect(enrollmentUrl).toBeTruthy()

      await page.goto(enrollmentUrl!)
      await expect(page.getByRole('heading', { name: 'Register a passkey' })).toBeVisible()
      await page.getByRole('button', { name: 'Register passkey' }).click()
      await expect(page.locator('.desktop-status').first()).toContainText('connected')

      await context.clearCookies()
      const duplicateOutput = authCommand(['passkey', 'add'])
      const duplicateEnrollmentUrl = duplicateOutput.match(/https?:\/\/\S+/)?.[0]
      expect(duplicateEnrollmentUrl).toBeTruthy()

      await page.goto(duplicateEnrollmentUrl!)
      await expect(page.getByRole('heading', { name: 'Register a passkey' })).toBeVisible()
      await page.getByRole('button', { name: 'Register passkey' }).click()
      await expect(page.locator('.desktop-status').first()).toContainText('connected')

      // A duplicate enrollment on the same authenticator falls back to the
      // already-registered passkey and consumes the one-time enrollment URL.
      await page.goto(duplicateEnrollmentUrl!)
      await expect(page.getByText('invalid or has expired')).toBeVisible()

      await context.clearCookies()
      await page.goto('/')
      await expect(page.getByRole('heading', { name: 'Authorization required' })).toBeVisible()
      await expect(page.getByRole('button', { name: 'Use a passkey' })).toBeVisible()
      await page.getByRole('button', { name: 'Use a passkey' }).click()
      await expect(page.locator('.desktop-status').first()).toContainText('connected')

      // Enrollment tokens are one-time. Reusing the consumed URL must fail
      // closed with the same browser treatment as an expired enrollment.
      await page.goto(enrollmentUrl!)
      await expect(page.getByText('invalid or has expired')).toBeVisible()
    } finally {
      await cdp.send('WebAuthn.removeVirtualAuthenticator', { authenticatorId })
      await cdp.send('WebAuthn.disable')
      authCommand(['passkey', 'clear'])
    }
  })
})
