import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'
import { realpathSync } from 'node:fs'

const devtoolsUrl = '/@fs' + realpathSync(new URL('../node_modules/@mun/web/dist/devtools.js', import.meta.url))

test('typing and unrelated status updates stay independent of a long transcript', async ({ page }, info) => {
  await installMockRemote(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const conversation = Array.from({ length: 180 }, (_, index) => [
      { id: `user-${index}`, kind: { type: 'user', content: `Review item ${index}` } },
      { id: `tool-${index}`, kind: { type: 'toolCall', toolCall: { id: `tool-${index}`, name: 'search_workspace', arguments: '{"query":"layout"}', status: 'completed', result: 'One match in src/layout.ts' } } },
      { id: `answer-${index}`, kind: { type: 'assistant', content: `Item ${index} reviewed.` } },
    ]).flat()
    ;(window as any).__yeetEmit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation })
  })
  await expect(page.locator('.activity-group')).toHaveCount(180)
  // Import the same host store used by the app; deliver projection-only events
  // without timing the test fixture's child-process/network overhead.
  if (info.project.use.browserName !== 'webkit') {
    const cdp = await page.context().newCDPSession(page)
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 })
  }
  const measurements = await page.evaluate(async devtoolsUrl => {
    const devtools = await import(devtoolsUrl)
    devtools.setMunDevtoolsEnabled(true)
    devtools.resetMunDevtools()
    const { remoteStore } = await import('/src/store/remoteStore.ts' as string)
    const composer = remoteStore.getSnapshot().composer
    const times: number[] = []
    for (let i = 0; i < 8; i++) {
      const start = performance.now()
      ;(window as any).__yeetEmit({ type: 'ui_composer', version: 1, composer_revision: 100 + i, view: { ...composer } })
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
      times.push(performance.now() - start)
    }
    return { times, boundaries: devtools.getMunDevtoolsSnapshot().boundaries.map((item: any) => ({ name: item.name, renders: item.renderCount })) }
  }, devtoolsUrl)
  console.log('composer-only update ms:', measurements)
  await info.attach('reaction-time.json', { body: JSON.stringify(measurements), contentType: 'application/json' })
  expect(measurements.boundaries.filter(item => ['Thread', 'WorkspacePanel', 'Sidebar', 'App', 'TopBar'].includes(item.name)).every(item => item.renders === 0)).toBe(true)
  const editor = page.getByRole('textbox', { name: 'Message', exact: true })
  await editor.fill('Keep basic typing responsive')
  await expect(editor).toHaveValue('Keep basic typing responsive')
})
