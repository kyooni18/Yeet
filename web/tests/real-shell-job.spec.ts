import { expect, test } from '@playwright/test'

test('real Remote reports a stale shell job rather than inventing a live process', async ({ page }) => {
  await page.route('https://static.cloudflareinsights.com/**', route => route.fulfill({status:200, contentType:'application/javascript', body:''}))
  await page.goto('/')
  await expect(page.getByRole('textbox', {name:'Message'})).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', '00000000-0000-0000-0000-000000000000')
    document.body.append(viewer)
  })
  await expect(page.locator('yeet-shell-job')).toContainText('Unknown or unrecoverable shell job', {timeout:15000})
})
