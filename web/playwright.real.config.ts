import { defineConfig } from '@playwright/test'

const backendPort = 17331

export default defineConfig({
  testDir: './tests',
  testMatch: 'real-*.spec.ts',
  timeout: 45_000,
  expect: { timeout: 12_000 },
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: `http://localhost:${backendPort}`,
    viewport: { width: 1440, height: 960 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: `sh -c 'npm run build && cd .. && cargo build --quiet && export YEET_CONFIG_DIR="$PWD/web/.e2e-config" && rm -rf "$YEET_CONFIG_DIR" && mkdir -p "$YEET_CONFIG_DIR" && target/debug/yeet remote auth key clear >/dev/null && target/debug/yeet remote auth passkey clear >/dev/null && export YEET_RUNTIME_DIR="$PWD/RuntimeSource" && exec target/debug/yeet remote --bind 127.0.0.1:${backendPort}'`,
    url: `http://127.0.0.1:${backendPort}/api/protocol`,
    timeout: 120_000,
    reuseExistingServer: false,
  },
})
