import { defineConfig } from '../../web/node_modules/@playwright/test/index.mjs'
import { fileURLToPath } from 'node:url'

export default defineConfig({
  testDir: './tests',
  testMatch: '**/*.spec.ts',
  fullyParallel: false,
  workers: 1,
  timeout: 30000,
  use: { baseURL: 'http://127.0.0.1:4187', browserName: 'chromium', viewport: { width: 1440, height: 960 }, trace: 'retain-on-failure' },
  webServer: {
    command: 'node tests/serve-export.mjs',
    cwd: fileURLToPath(new URL('.', import.meta.url)),
    port: 4187,
    reuseExistingServer: false,
  },
})
