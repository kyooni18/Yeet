import { expect, test } from '@playwright/test'
import { installMockRemote } from './mockRemote'

test('transient output rejection retries without treating the job as failed', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', 'busy-job')
    document.body.append(viewer)
  })
  const outputs = () => page.evaluate(() => ((window as any).__yeetSent as any[])
    .filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === 'output').map(item => item.command))
  await expect.poll(async () => (await outputs()).length).toBe(1)
  const request = (await outputs())[0]
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'busy-job',requestId:request.request_id,action:'output',error:'Too many pending shell job requests; try again',retryable:true}}), request)
  await expect(page.locator('yeet-shell-job')).toContainText('Too many pending')
  await page.waitForTimeout(250)
  expect(await outputs()).toHaveLength(1)
  await expect.poll(async () => (await outputs()).length, {timeout:5000}).toBe(2)
  const retry = (await outputs())[1]
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'busy-job',requestId:request.request_id,action:'output',result:{events:[],nextOffset:0,totalBytes:0,finished:true,truncated:false,status:'completed'}}}), retry)
  await expect(page.locator('yeet-shell-job')).toContainText('Completed')
})

test('truncated recording remains interactive until the job finishes', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', 'truncated-job')
    document.body.append(viewer)
  })
  const last = (action: string) => page.evaluate(action => ((window as any).__yeetSent as any[])
    .filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === action).at(-1)?.command ?? null, action)
  await expect.poll(() => last('output')).not.toBeNull()
  const first = await last('output')
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'truncated-job',requestId:request.request_id,action:'output',result:{events:[],nextOffset:0,totalBytes:0,finished:false,truncated:true,status:'running'}}}), first)
  await expect(page.locator('yeet-shell-job')).toContainText('Running; output truncated')
  await expect.poll(async () => (await last('output'))?.request_id).not.toBe(first.request_id)
  await page.locator('yeet-shell-job .xterm-helper-textarea').focus()
  await page.keyboard.type('x')
  await expect.poll(() => last('input')).toMatchObject({data:'x'})
  const pending = await last('output')
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'truncated-job',requestId:request.request_id,action:'output',result:{events:[],nextOffset:0,totalBytes:0,finished:true,truncated:true,status:'completed'}}}), pending)
  await expect(page.locator('yeet-shell-job')).toContainText('Completed; output truncated')
})

test('shell viewer pages output and sends interactive controls over Remote', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    // Exercise the real Rust projection rather than a partial hand-written
    // rich record that no production server would send.
    ;(window as any).__yeetEmit({ type: 'conversation_reset', version: 1, sequence: 2, revision: 2, conversation: [
      { id: 'interactive-job', kind: { type: 'toolCall', toolCall: {
        id: 'interactive-job', name: 'run_shell', arguments: '{"command":"npm run watch"}', status: 'completed',
        rawResult: JSON.stringify({ jobId: 'job-test', status: 'running', outputHandedOff: true }),
      } } },
    ] })
  })
  const header = page.locator('.activity-group__header')
  await expect(header).toContainText('npm run watch')
  await header.click()
  await expect(page.locator('.activity-group yeet-shell-job')).toBeVisible()
  const command = async (action: string) => page.evaluate(action => {
    const sent = (window as any).__yeetSent as Array<{ type: string; command?: any }>
    return sent.findLast(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === action)?.command ?? null
  }, action)
  await expect.poll(() => command('output')).not.toBeNull()
  const request = await command('output')
  await page.evaluate((request) => {
    (window as any).__yeetEmit({ type: 'shell_job', version: 1, payload: {
      jobId: 'job-test', requestId: request.request_id, action: 'output', result: {
        events: [{ offset: 0, nextOffset: 6, stream: 'pty', dataBase64: btoa('hello\r') }],
        nextOffset: 6, totalBytes: 6, finished: false, truncated: false, status: 'running',
      },
    } })
  }, request)
  await expect(page.locator('yeet-shell-job')).toContainText('hello')
  await expect(page.locator('yeet-shell-job')).toContainText('Running')
  // A normal application refresh must not wipe xterm's imperative DOM or
  // reset its cursor. This used to intermittently leave an empty custom element.
  await page.evaluate(() => (window as any).__yeetEmit({ type: 'state_update', version: 1, sequence: 3, revision: 3, patch: { credit_usage: 1 } }))
  await expect(page.locator('yeet-shell-job .xterm-rows')).toContainText('hello')
  await expect.poll(() => command('output')).toMatchObject({ offset: 6 })
  await page.locator('yeet-shell-job').getByRole('button', { name: 'Interrupt interactive shell' }).click()
  await expect.poll(() => command('interrupt')).not.toBeNull()
  await page.locator('yeet-shell-job').getByRole('button', { name: 'Stop job' }).click()
  await expect.poll(() => command('stop')).not.toBeNull()
  await expect.poll(() => command('resize')).not.toBeNull()
  await page.locator('yeet-shell-job .xterm-helper-textarea').focus()
  await page.keyboard.type('hi')
  await expect.poll(() => command('input')).toMatchObject({data: 'h'})
  const firstInput = await command('input')
  await page.evaluate(request => (window as any).__yeetEmit({ type:'shell_job', version:1, payload: { jobId:'job-test', requestId:request.request_id, action:'input', result:{delivered:true} } }), firstInput)
  await expect.poll(() => command('input')).toMatchObject({data: 'i'})
  const finalOutput = await command('output')
  await page.evaluate(request => (window as any).__yeetEmit({ type:'shell_job', version:1, payload: { jobId:'job-test', requestId:request.request_id, action:'output', result:{events:[],nextOffset:6,totalBytes:6,finished:true,truncated:false,status:'failed'} } }), finalOutput)
  await expect(page.locator('yeet-shell-job')).toContainText('Failed')
})


test('shell viewer resumes from its byte cursor after a transport reconnect', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', 'resume-job')
    document.body.append(viewer)
  })
  await page.locator('yeet-shell-job').scrollIntoViewIfNeeded()
  const lastOutput = () => page.evaluate(() => ((window as any).__yeetSent as any[]).filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === 'output').at(-1)?.command ?? null)
  await expect.poll(lastOutput).not.toBeNull()
  const first = await lastOutput()
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'resume-job',requestId:request.request_id,action:'output',result:{events:[{offset:0,nextOffset:3,stream:'pty',dataBase64:btoa('one')}],nextOffset:3,totalBytes:3,finished:false,truncated:false}}}), first)
  await expect.poll(lastOutput).toMatchObject({offset:3})
  const pending = await lastOutput()
  await page.evaluate(() => (window as any).__yeetDisconnect())
  await expect.poll(async () => {
    const request = await lastOutput()
    return request?.offset === 3 && request?.request_id !== pending.request_id
  }, {timeout:10000}).toBe(true)
  const resumed = await lastOutput()
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'resume-job',requestId:request.request_id,action:'output',result:{events:[{offset:3,nextOffset:6,stream:'pty',dataBase64:btoa('two')}],nextOffset:6,totalBytes:6,finished:true,truncated:false}}}), resumed)
  await expect(page.locator('yeet-shell-job')).toContainText('onetwo')
  await expect(page.locator('yeet-shell-job')).toContainText('Job finished')
})

test('terminal renders ANSI color and carriage-return updates across pages', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', 'ansi-job')
    document.body.append(viewer)
  })
  await page.locator('yeet-shell-job').scrollIntoViewIfNeeded()
  const lastOutput = () => page.evaluate(() => ((window as any).__yeetSent as any[]).filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === 'output').at(-1)?.command ?? null)
  await expect.poll(lastOutput).not.toBeNull()
  const first = await lastOutput()
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'ansi-job',requestId:request.request_id,action:'output',result:{events:[{offset:0,nextOffset:7,stream:'pty',dataBase64:btoa('\x1b[31mab')}],nextOffset:7,totalBytes:7,finished:false,truncated:false,status:'running'}}}), first)
  await expect.poll(lastOutput).toMatchObject({offset:7})
  const second = await lastOutput()
  await page.evaluate(request => (window as any).__yeetEmit({type:'shell_job',version:1,payload:{jobId:'ansi-job',requestId:request.request_id,action:'output',result:{events:[{offset:7,nextOffset:15,stream:'pty',dataBase64:btoa('\rOK\x1b[0m!')}],nextOffset:15,totalBytes:15,finished:true,truncated:false,status:'completed'}}}), second)
  await expect(page.locator('yeet-shell-job .xterm-rows')).toContainText('OK!')
  await expect(page.locator('yeet-shell-job .xterm-rows')).not.toContainText('ab')
  await expect(page.locator('yeet-shell-job .xterm-fg-1')).toContainText('OK')
  await expect(page.locator('yeet-shell-job')).toContainText('Completed')
})

test('does not replay unacknowledged terminal input after reconnect', async ({ page }) => {
  await installMockRemote(page)
  await page.goto('/')
  await expect(page.getByText('Interface ready')).toBeVisible()
  await page.evaluate(() => {
    const viewer = document.createElement('yeet-shell-job')
    viewer.setAttribute('job-id', 'input-job')
    document.body.append(viewer)
  })
  await page.locator('yeet-shell-job').scrollIntoViewIfNeeded()
  await page.locator('yeet-shell-job .xterm-helper-textarea').focus()
  await page.keyboard.type('ab')
  const inputs = () => page.evaluate(() => ((window as any).__yeetSent as any[]).filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === 'input').map(item => item.command.data))
  await expect.poll(inputs).toEqual(['a'])
  await page.evaluate(() => (window as any).__yeetDisconnect())
  // Wait for the output read to be reissued by the reconnected transport.
  await expect.poll(() => page.evaluate(() => ((window as any).__yeetSent as any[]).filter(item => item.type === 'command' && item.command?.type === 'shell_job' && item.command.action === 'output').length), {timeout:10000}).toBeGreaterThan(1)
  expect(await inputs()).toEqual(['a'])
})
