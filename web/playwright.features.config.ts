import { defineConfig } from '@playwright/test'

// Real, isolated server smoke: no provider completions or user workspace writes.
export default defineConfig({
  testDir: './tests', testMatch: 'real-remote-features.spec.ts',
  workers: 1, retries: 0, timeout: 180_000,
  outputDir: './test-results-real-features',
  expect: { timeout: 15_000 }, reporter: [['list']],
  use: { actionTimeout: 15_000, viewport: { width: 1440, height: 960 }, screenshot: 'only-on-failure', trace: 'retain-on-failure' },
})
