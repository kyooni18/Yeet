import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  testIgnore: '**/real-*.spec.ts',
  fullyParallel: false,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : 4,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:4174',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'pnpm vite --host 127.0.0.1 --port 4174',
    port: 4174,
    reuseExistingServer: !process.env.CI,
  },
  projects: [
    {
      name: 'mobile-small',
      use: { browserName: 'webkit', viewport: { width: 320, height: 568 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    {
      name: 'mobile-portrait',
      use: { browserName: 'webkit', ...devices['iPhone 16 Pro'] },
    },
    {
      name: 'mobile-landscape',
      use: { browserName: 'webkit', viewport: { width: 852, height: 393 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true },
    },
    {
      name: 'ipad-portrait',
      use: { browserName: 'webkit', viewport: { width: 820, height: 1180 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    {
      name: 'ipad-landscape',
      use: { browserName: 'webkit', viewport: { width: 1180, height: 820 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    { name: 'laptop', use: { browserName: 'chromium', viewport: { width: 1366, height: 768 } } },
    { name: 'desktop', use: { viewport: { width: 1440, height: 960 } } },
    { name: 'ultrawide', use: { browserName: 'chromium', viewport: { width: 2560, height: 1080 } } },
  ],
})
