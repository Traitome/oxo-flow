import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  // #555: silent retries ate genuine failures — the reporter now lists
  // retried tests so a flake is visible in the job summary.
  reporter: process.env.CI ? [['html', { open: 'never' }], ['list']] : 'html',
  workers: process.env.CI ? 1 : undefined,
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
      // #555: in CI anything already on :3000 would make the e2e job
      // validate the WRONG process. Reuse is a local-dev convenience only.
      reuseExistingServer: !process.env.CI,
      // 60s was too tight: after a cold cache the spawn pays a partial
      // recompile of feature-sensitive deps (CI evidence: libsqlite3-sys/
      // reqwest/rustls rebuilt on first run in ~53s and the timer fired).
      // 240s covers cold builds without making genuinely dead servers
      // wait four minutes — the warm path finishes in seconds.
      timeout: 240000,
    },
  ],
});
