import { test, expect } from '@playwright/test'
import { installMockRemote } from './mockRemote'

for (const appearance of ['light', 'dark'] as const) {
  test(`minimal conversation · ${appearance}`, async ({ page }, testInfo) => {
    const errors: string[] = []
    page.on("pageerror", error => errors.push(error.message))
    await page.route("https://static.cloudflareinsights.com/**", route => route.fulfill({ body: "", contentType: "text/javascript" }))
    await installMockRemote(page)
    await page.emulateMedia({ colorScheme: appearance, reducedMotion: 'reduce' })
    await page.goto('/')
    await expect(page.getByText('Interface ready')).toBeVisible()
    const reasoning = page.locator('.reasoning-trace')
    await expect(reasoning).toContainText('Checked state and responsive layout')
    await expect(reasoning.locator('button, svg')).toHaveCount(0)
    await expect(reasoning).not.toContainText('I checked the semantic state')
    const editor = page.getByRole('textbox', { name: 'Message', exact: true })
    await expect(editor).toBeVisible()
    const geometry = await page.evaluate(() => ({
      width: innerWidth, scroll: document.documentElement.scrollWidth,
      editor: document.querySelector('textarea')?.getBoundingClientRect().toJSON(),
    }))
    expect(geometry.scroll).toBeLessThanOrEqual(geometry.width)
    await page.screenshot({ path: testInfo.outputPath(`conversation-${appearance}.png`) })
    const header = page.locator('.activity-group__header').first()
    await header.click()
    await expect(header).toHaveAttribute('aria-expanded', 'true')
    await expect(page.locator('.activity-group__events').first()).toBeVisible()
    for (const view of ['Files', 'Diff', 'Agents']) {
      if (!await page.locator('#sidebar').isVisible()) await page.getByRole('button', { name: 'Toggle sidebar' }).click()
      await page.getByRole('button', { name: new RegExp(`^${view}(?:\\s|$)`) }).click()
      await expect(editor).toHaveCount(0)
      if (view === 'Files') await expect(page.locator('#main')).toContainText('Files')
      if (view === 'Diff') await expect(page.locator('#main')).toContainText('Review changes')
      if (view === 'Agents') await expect(page.getByRole('button', { name: 'Create', exact: true })).toBeVisible()
      await page.screenshot({ path: testInfo.outputPath(`${view.toLowerCase()}-${appearance}.png`) })
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    }
    expect(errors).toEqual([])
  })
}
