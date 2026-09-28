import { expect, test, type Page } from '@playwright/test'
import { installMockRemote } from './mockRemote'

type Hooks = {
  __yeetSent: Array<Record<string, unknown>>
}

async function commands(page: Page) {
  return page.evaluate(() => {
    const sent = (window as unknown as Hooks).__yeetSent
    return sent
      .filter((item) => item.type === 'command')
      .map((item) => item.command as Record<string, unknown>)
  })
}

test.beforeEach(async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
})

test('renders the iOS remote shell without horizontal overflow', async ({ page }) => {
  await expect(page.locator('.remote-topbar')).toBeVisible()
  await expect(page.locator('.composer-shell')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Message' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Open sidebar' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Quick settings' })).toBeVisible()

  const topbar = await page.locator('.remote-topbar').boundingBox()
  expect(topbar).not.toBeNull()
  expect(Math.round(topbar?.height ?? 0)).toBe(56)

  const overflow = await page.evaluate(() => ({
    body: document.body.scrollWidth - document.body.clientWidth,
    root: document.documentElement.scrollWidth - document.documentElement.clientWidth,
  }))
  expect(overflow.body).toBeLessThanOrEqual(1)
  expect(overflow.root).toBeLessThanOrEqual(1)
})

test('uses an overlay sidebar on touch and a docked sidebar on desktop', async ({ page }) => {
  await page.getByRole('button', { name: 'Open sidebar' }).click()
  const sidebar = page.locator('.remote-sidebar')
  await expect(sidebar).toHaveClass(/is-open/)
  await expect(sidebar.getByText('Sessions')).toBeVisible()
  await expect(sidebar.getByText('Remote WebUI')).toBeVisible()

  const sidebarBox = await sidebar.boundingBox()
  expect(sidebarBox).not.toBeNull()
  expect(sidebarBox?.width ?? 0).toBeGreaterThan(250)
  expect(sidebarBox?.width ?? 999).toBeLessThanOrEqual(300)

  const desktopDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1000px) and (hover: hover) and (pointer: fine)').matches
  )
  const viewport = page.viewportSize()
  await page.mouse.click((viewport?.width ?? 402) - 8, 120)

  if (desktopDocked) {
    await expect(sidebar).toHaveClass(/is-open/)
    await expect(sidebar).toHaveAttribute('role', 'complementary')
    const main = page.locator('.main-viewport')
    await expect.poll(async () => {
      const [sidebarBounds, mainBounds] = await Promise.all([sidebar.boundingBox(), main.boundingBox()])
      if (!sidebarBounds || !mainBounds) return Number.POSITIVE_INFINITY
      return Math.abs(mainBounds.x - (sidebarBounds.x + sidebarBounds.width))
    }).toBeLessThanOrEqual(1)
  } else {
    await expect(sidebar).not.toHaveClass(/is-open/)
  }

  await page.getByRole('button', { name: 'Quick settings' }).click()
  const controls = page.locator('.quick-panel')
  await expect(controls).toHaveClass(/is-open/)
  await expect(controls.getByText('Controls')).toBeVisible()
  await expect(controls.getByText('Model')).toBeVisible()
  await expect(controls.getByText('Reasoning')).toBeVisible()
  await expect(controls.getByText('Context')).toBeVisible()

  const controlsBox = await controls.boundingBox()
  expect(controlsBox).not.toBeNull()
  const inspectorDocked = await page.evaluate(() =>
    matchMedia('(min-width: 1180px) and (hover: hover) and (pointer: fine)').matches
  )
  if (inspectorDocked) {
    const main = page.locator('.main-viewport')
    await expect.poll(async () => (await controls.boundingBox())?.width ?? 0).toBeGreaterThanOrEqual(290)
    await expect.poll(async () => {
      const [controlsBounds, mainBounds] = await Promise.all([controls.boundingBox(), main.boundingBox()])
      if (!controlsBounds || !mainBounds) return Number.POSITIVE_INFINITY
      return Math.abs(controlsBounds.x - (mainBounds.x + mainBounds.width))
    }).toBeLessThanOrEqual(1)
    const settledControls = await controls.boundingBox()
    expect(settledControls?.width ?? 999).toBeLessThanOrEqual(340)
  } else {
    expect(controlsBox?.width ?? 0).toBeGreaterThan(250)
    expect(controlsBox?.width ?? 999).toBeLessThanOrEqual(300)
  }
})

test('collapses the composer toolbar and keeps the affordance usable', async ({ page }) => {
  const toolbar = page.locator('.composer-toolbar')
  await expect(toolbar).toHaveClass(/is-expanded/)
  await expect(toolbar.getByText('gpt-5.6-sol')).toBeVisible()

  await page.getByRole('button', { name: 'Collapse toolbar' }).click()
  await expect(toolbar).toHaveClass(/is-collapsed/)
  await expect(page.getByRole('button', { name: 'Expand toolbar' })).toBeVisible()

  await page.getByRole('button', { name: 'Expand toolbar' }).click()
  await expect(toolbar).toHaveClass(/is-expanded/)
})

test('opens model sheet and emits model, goal, submit and interrupt commands', async ({ page }) => {
  await page.locator('.composer-toolbar .toolbar-chip').first().click()
  const sheet = page.getByRole('dialog', { name: 'Choose model' })
  await expect(sheet).toBeVisible()
  await sheet.locator('.model-row').filter({ hasText: 'gpt-5.6-luna' }).click()

  await expect.poll(async () => (await commands(page)).some((command) =>
    command.type === 'select_model' && command.model === 'gpt-5.6-luna'
  )).toBe(true)

  await page.getByRole('button', { name: 'Goal', exact: true }).click()
  await expect.poll(async () => (await commands(page)).some((command) =>
    command.type === 'set_goal' && command.enabled === true
  )).toBe(true)

  const message = page.getByRole('textbox', { name: 'Message' })
  await message.fill('React remote smoke test')
  await page.getByRole('button', { name: 'Send' }).click()
  await expect.poll(async () => (await commands(page)).some((command) =>
    command.type === 'submit' && command.text === 'React remote smoke test'
  )).toBe(true)

  await page.evaluate(() => {
    ;(window as unknown as { __yeetEmit: (message: Record<string, unknown>) => void }).__yeetEmit({
      type: 'assistant_delta',
      version: 1,
      sequence: 2,
      revision: 2,
      entry_id: 'a1',
      delta: '',
      content: 'Streaming',
      reset: true,
    })
  })
  await expect(page.getByRole('button', { name: 'Stop' })).toBeVisible()
  await page.getByRole('button', { name: 'Stop' }).click()

  await expect.poll(async () => (await commands(page)).some((command) =>
    command.type === 'interrupt'
  )).toBe(true)
})


test('renders completed tools as compact trace disclosures instead of cards', async ({ page }) => {
  const group = page.locator('.activity-group').first()
  await group.locator('.activity-group__header').click()
  await expect(group.locator('.activity-group__header')).toHaveAttribute('aria-expanded', 'true')

  const row = page.locator('.trace-disclosure__row').filter({ hasText: 'Read file' }).first()

  await expect(row).toBeVisible()
  await expect(row).toContainText('App.tsx')
  await expect(row).toContainText('✓')
  await expect(row).toContainText(/82\s*ms/)

  const geometry = await row.evaluate((element) => {
    const rowStyle = getComputedStyle(element)
    const disclosure = element.closest('.trace-disclosure')
    const disclosureStyle = disclosure ? getComputedStyle(disclosure) : null
    return {
      height: element.getBoundingClientRect().height,
      rowBackground: rowStyle.backgroundColor,
      rowBorder: rowStyle.borderTopWidth,
      rowRadius: rowStyle.borderRadius,
      disclosureBackground: disclosureStyle?.backgroundColor ?? null,
      disclosureBorder: disclosureStyle?.borderTopWidth ?? null,
      disclosureRadius: disclosureStyle?.borderRadius ?? null,
    }
  })

  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  if (touchFirst) expect(geometry.height).toBeGreaterThanOrEqual(44)
  else expect(geometry.height).toBeLessThanOrEqual(24)
  expect(geometry.rowBackground).toBe('rgba(0, 0, 0, 0)')
  expect(geometry.rowBorder).toBe('0px')
  expect(geometry.rowRadius).toBe('0px')
  expect(geometry.disclosureBackground).toBe('rgba(0, 0, 0, 0)')
  expect(geometry.disclosureBorder).toBe('0px')
  expect(geometry.disclosureRadius).toBe('0px')

  await row.click()
  await expect(row).toHaveAttribute('aria-expanded', 'true')
})


test('matches iOS chat bubble actions and composer control geometry', async ({ page }) => {
  const userBubble = page.locator('.user-bubble').first()
  await expect(userBubble).toBeVisible()

  const bubbleStyle = await userBubble.evaluate((element) => {
    const style = getComputedStyle(element)
    return {
      topLeft: style.borderTopLeftRadius,
      topRight: style.borderTopRightRadius,
      bottomRight: style.borderBottomRightRadius,
      bottomLeft: style.borderBottomLeftRadius,
      paddingTop: style.paddingTop,
      paddingLeft: style.paddingLeft,
    }
  })
  expect(bubbleStyle.topLeft).toBe('15px')
  expect(bubbleStyle.topRight).toBe('15px')
  expect(bubbleStyle.bottomRight).toBe('15px')
  expect(bubbleStyle.bottomLeft).toBe('15px')
  expect(bubbleStyle.paddingTop).toBe('10px')
  expect(bubbleStyle.paddingLeft).toBe('12px')

  const copyButton = page.getByRole('button', { name: 'Copy message' }).first()
  const editButton = page.getByRole('button', { name: 'Edit message' })
  const regenerateButton = page.getByRole('button', { name: 'Regenerate response' })
  await expect(copyButton).toBeVisible()
  await expect(editButton).toBeVisible()
  await expect(regenerateButton).toBeVisible()

  const actionBox = await editButton.boundingBox()
  const touchFirst = await page.evaluate(() => matchMedia('(hover: none) and (pointer: coarse)').matches)
  if (touchFirst) {
    expect(Math.round(actionBox?.width ?? 0)).toBeGreaterThanOrEqual(44)
    expect(Math.round(actionBox?.height ?? 0)).toBeGreaterThanOrEqual(44)
  } else {
    expect(Math.round(actionBox?.width ?? 0)).toBe(32)
    expect(Math.round(actionBox?.height ?? 0)).toBe(32)
  }

  const addButton = page.getByRole('button', { name: 'Add attachment' })
  const sendButton = page.getByRole('button', { name: 'Send' })
  const input = page.locator('.composer-input')
  await expect(addButton).toBeVisible()
  await expect(sendButton).toBeVisible()

  const addBox = await addButton.boundingBox()
  const sendBox = await sendButton.boundingBox()
  expect(Math.round(addBox?.width ?? 0)).toBe(48)
  expect(Math.round(addBox?.height ?? 0)).toBe(48)
  expect(Math.round(sendBox?.width ?? 0)).toBe(48)
  expect(Math.round(sendBox?.height ?? 0)).toBe(48)

  const visualCircles = await Promise.all([addButton, sendButton].map((button) =>
    button.evaluate((element) => {
      const style = getComputedStyle(element, '::before')
      return { width: style.width, height: style.height, top: style.top, left: style.left }
    }),
  ))
  for (const circle of visualCircles) {
    expect(circle.width).toBe('44px')
    expect(circle.height).toBe('44px')
    expect(circle.top).toBe('2px')
    expect(circle.left).toBe('2px')
  }

  const inputStyle = await input.evaluate((element) => {
    const style = getComputedStyle(element)
    return { radius: style.borderRadius, minHeight: style.minHeight }
  })
  expect(inputStyle.radius).toBe('15px')
  expect(inputStyle.minHeight).toBe('44px')

  await editButton.click()
  await expect(page.getByText('Editing last message')).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue('Inspect the **remote UI** and keep it responsive.')
})
