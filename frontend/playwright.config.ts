import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: 'html',
  use: {
    // Tests use the Rust server directly (port 3000) for both UI and API tests.
    // The frontend must be pre-built: npm run build
    baseURL: 'http://localhost:3000',
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: [
    {
      command: 'cargo run -p oxo-flow-web -- --port 3000',
      cwd: '../',
      env: { OXO_FLOW_DISABLE_RATE_LIMIT: '1' },
      port: 3000,
      reuseExistingServer: true,
      timeout: 60000,
    },
  ],
});
