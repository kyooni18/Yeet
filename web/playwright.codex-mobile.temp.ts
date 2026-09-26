import { defineConfig, devices } from '@playwright/test'

const executablePath = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
const chrome = { browserName: 'chromium' as const, launchOptions: { executablePath } }

export default defineConfig({
  testDir: './tests',
  testIgnore: '**/real-*.spec.ts',
  fullyParallel: false,
  timeout: 30_000,
  expect: { timeout: 5_000 },
  workers: 1,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4174',
    trace: 'off',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'pnpm vite --host 127.0.0.1 --port 4174',
    port: 4174,
    reuseExistingServer: true,
  },
  projects: [
    {
      name: 'mobile-small',
      use: { ...chrome, viewport: { width: 320, height: 568 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    {
      name: 'mobile-portrait',
      use: { ...chrome, ...devices['iPhone 16 Pro'], browserName: 'chromium', launchOptions: { executablePath } },
    },
    {
      name: 'ipad-portrait',
      use: { ...chrome, viewport: { width: 820, height: 1180 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    {
      name: 'ipad-landscape',
      use: { ...chrome, viewport: { width: 1180, height: 820 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
    },
    {
      name: 'desktop',
      use: { ...chrome, viewport: { width: 1440, height: 960 } },
    },
  ],
})
