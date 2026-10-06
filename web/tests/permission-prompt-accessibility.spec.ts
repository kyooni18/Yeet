import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type TestHooks = {
  __yeetEmit: (message: Record<string, unknown>) => void
  __yeetSent: Record<string, unknown>[]
}

async function emit(page: Page, message: Record<string, unknown>) {
  await page.evaluate((payload) => (window as unknown as TestHooks).__yeetEmit(payload), message)
}

async function sentCommands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as TestHooks).__yeetSent
    return sent.filter((item) => item.type === 'command').map((item) => item.command as Record<string, unknown>)
  })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('permission alertdialog takes keyboard focus and restores the prior control after resolution', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.focus()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-focus',
        kind: 'shell',
        command: 'cargo test --all-targets',
        operation: 'execute',
        reason: 'Run verification',
      },
    },
  })

  const prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  await expect(prompt).toBeVisible()
  const deny = prompt.getByRole('button', { name: 'Deny' })
  await expect(deny).toBeFocused()
  await expect(prompt).toHaveAttribute('aria-describedby', 'permission-reason permission-detail')
  await expect(prompt).toContainText('Run verification')
  await expect(prompt).toContainText('cargo test --all-targets')

  await page.keyboard.press('Tab')
  const allow = prompt.getByRole('button', { name: 'Allow' })
  await expect(allow).toBeFocused()
  await page.keyboard.press('Shift+Tab')
  await expect(deny).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(allow).toBeFocused()
  await page.keyboard.press('Enter')
  await expect.poll(() => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'resolve_permission', request_id: 'shell-focus', granted: true }),
  ]))

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: { pending_shell_permission: null },
  })

  await expect(prompt).not.toBeAttached()
  await expect(composer).toBeFocused()
})

test('a replacement permission is re-announced from the dialog root without losing the original return target', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.focus()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-replaced',
        kind: 'shell',
        command: 'cargo check',
        operation: 'execute',
        reason: 'Check build',
      },
    },
  })

  let prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  await expect(prompt.getByRole('button', { name: 'Deny' })).toBeFocused()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: {
      pending_shell_permission: null,
      pending_native_app_permission: {
        id: 'native-replacement',
        server: 'Computer Use',
        tool: 'click',
        appName: 'Safari',
        operation: 'interact',
        reason: 'Use browser',
      },
    },
  })

  prompt = page.getByRole('alertdialog', { name: 'Native app permission requested' })
  const nativeDeny = prompt.getByRole('button', { name: 'Deny' })
  await expect(nativeDeny).toBeFocused()
  await expect(prompt).toContainText('Safari · click')
  await page.keyboard.press('Enter')
  await expect.poll(() => sentCommands(page)).toEqual(expect.arrayContaining([
    expect.objectContaining({ type: 'resolve_permission', request_id: 'native-replacement', granted: false }),
  ]))

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 4,
    revision: 4,
    patch: { pending_native_app_permission: null },
  })

  await expect(prompt).not.toBeAttached()
  await expect(composer).toBeFocused()
})

test('permission resolution does not steal focus back after the user deliberately moves elsewhere', async ({ page }) => {
  const composer = page.getByRole('textbox', { name: 'Message' })
  await composer.focus()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-leave',
        kind: 'shell',
        command: 'cargo test',
        operation: 'execute',
        reason: 'Run tests',
      },
    },
  })

  const prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  await expect(prompt.getByRole('button', { name: 'Deny' })).toBeFocused()
  const transcript = page.getByTestId('transcript')
  await transcript.focus()
  await expect(transcript).toBeFocused()

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 3,
    revision: 3,
    patch: { pending_shell_permission: null },
  })

  await expect(prompt).not.toBeAttached()
  await expect(transcript).toBeFocused()
})

test('narrow permission prompts keep oversized commands inspectable and actions touch sized', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 1000) >= 900)
  const longCommand = `echo ${'segment '.repeat(500)} --final-sentinel`

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-narrow-layout',
        kind: 'shell',
        command: longCommand,
        operation: 'execute',
        reason: 'Review the entire command before allowing execution',
      },
    },
  })

  const prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  const command = prompt.locator('#permission-detail')
  await expect(command).toContainText('--final-sentinel')
  const promptBox = await prompt.boundingBox()
  const viewport = page.viewportSize()
  expect(promptBox).not.toBeNull()
  expect(promptBox!.y).toBeGreaterThanOrEqual(0)
  expect(promptBox!.y + promptBox!.height).toBeLessThanOrEqual((viewport?.height ?? 0) + 1)

  const commandStyle = await command.evaluate((element) => {
    const style = getComputedStyle(element)
    return {
      overflowX: style.overflowX,
      overflowY: style.overflowY,
      textOverflow: style.textOverflow,
      whiteSpace: style.whiteSpace,
      scrollWidth: element.scrollWidth,
      clientWidth: element.clientWidth,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
    }
  })
  expect(commandStyle.overflowX).not.toBe('hidden')
  expect(commandStyle.textOverflow).not.toBe('ellipsis')
  expect(commandStyle.whiteSpace).not.toBe('nowrap')
  expect(commandStyle.clientWidth).toBeGreaterThan(0)
  expect(commandStyle.scrollHeight).toBeGreaterThan(commandStyle.clientHeight)
  expect(['auto', 'scroll']).toContain(commandStyle.overflowY)

  for (const name of ['Deny', 'Allow']) {
    const box = await prompt.getByRole('button', { name }).boundingBox()
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(44)
    expect(box?.width ?? 0).toBeGreaterThanOrEqual(44)
  }

  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual((page.viewportSize()?.width ?? 0) + 1)
})


test('pending permission remains inspectable but cannot be answered while Remote is offline', async ({ page }) => {
  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-offline',
        kind: 'shell',
        command: 'cargo test --workspace',
        operation: 'execute',
        reason: 'Run verification before reconnect',
      },
    },
  })

  const prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  const deny = prompt.getByRole('button', { name: 'Deny' })
  const allow = prompt.getByRole('button', { name: 'Allow' })
  await expect(prompt).toBeVisible()
  await expect(deny).toBeEnabled()
  await expect(allow).toBeEnabled()

  await page.evaluate(() => window.dispatchEvent(new Event('offline')))

  await expect(prompt).toBeVisible()
  await expect(prompt).toContainText('cargo test --workspace')
  await expect(prompt).toContainText('Reconnect to respond')
  await expect(deny).toBeDisabled()
  await expect(allow).toBeDisabled()
  await expect(prompt).toBeFocused()
})


test('wide permission actions adapt touch target size to pointer type', async ({ page }) => {
  const viewport = page.viewportSize()
  test.skip(!viewport || viewport.width < 900)

  await emit(page, {
    type: 'state_update',
    version: 1,
    sequence: 2,
    revision: 2,
    patch: {
      pending_shell_permission: {
        id: 'shell-wide-pointer',
        kind: 'shell',
        command: 'cargo check',
        operation: 'execute',
        reason: 'Verify the current change',
      },
    },
  })

  const prompt = page.getByRole('alertdialog', { name: 'Shell permission requested' })
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  for (const name of ['Deny', 'Allow']) {
    const box = await prompt.getByRole('button', { name }).boundingBox()
    expect(box).not.toBeNull()
    if (touchFirst) {
      expect(box!.width).toBeGreaterThanOrEqual(44)
      expect(box!.height).toBeGreaterThanOrEqual(44)
    } else {
      expect(box!.height).toBeLessThan(44)
    }
  }
})
